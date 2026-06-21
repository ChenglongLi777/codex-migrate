use crate::model::ThreadRecord;
use anyhow::{anyhow, Context, Result};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, OptionalExtension};
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

pub fn check_write_lock(path: &Path) -> Result<()> {
    let connection = Connection::open(path)
        .with_context(|| format!("open state DB for lock check {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(2))?;
    connection
        .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
        .map_err(|error| {
            anyhow!(
                "Codex state database is busy; close Codex app and active CLI sessions: {error}"
            )
        })
}

pub fn integrity_check(path: &Path) -> Result<String> {
    let connection = open_readable(path)?;
    connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(Into::into)
}

pub fn count_threads(path: &Path) -> Result<usize> {
    let connection = open_readable(path)?;
    if !table_exists(&connection, "threads")? {
        return Ok(0);
    }
    let count: i64 = connection.query_row("SELECT count(*) FROM threads", [], |row| row.get(0))?;
    Ok(count as usize)
}

pub fn thread_rollout_path(path: &Path, thread_id: &str) -> Result<Option<String>> {
    let connection = open_readable(path)?;
    if !table_exists(&connection, "threads")? {
        return Ok(None);
    }
    connection
        .query_row(
            "SELECT rollout_path FROM threads WHERE id = ?1",
            [thread_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
}

pub fn validate_visible_thread(path: &Path, thread_id: &str, expected_cwd: &Path) -> Result<()> {
    let connection = open_readable(path)?;
    let columns = table_columns(&connection, "threads")?;
    let preview_expression = if columns.contains("preview") {
        "preview"
    } else {
        "first_user_message"
    };
    let sql = format!("SELECT cwd, rollout_path, {preview_expression} FROM threads WHERE id = ?1");
    let (cwd, rollout_path, preview): (String, String, String) = connection
        .query_row(&sql, [thread_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .with_context(|| format!("thread {thread_id} is missing from the state database"))?;
    if Path::new(&cwd) != expected_cwd {
        return Err(anyhow!(
            "thread {thread_id} has cwd {cwd}, expected {}",
            expected_cwd.display()
        ));
    }
    if !Path::new(&rollout_path).is_file() {
        return Err(anyhow!(
            "thread {thread_id} rollout is missing: {rollout_path}"
        ));
    }
    if preview.trim().is_empty() {
        return Err(anyhow!(
            "thread {thread_id} has an empty preview and will be hidden by Codex"
        ));
    }
    Ok(())
}

pub fn upsert_thread(
    path: &Path,
    thread: &ThreadRecord,
    rollout_path: &Path,
    cwd: &Path,
) -> Result<()> {
    let mut connection =
        Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    if !table_exists(&connection, "threads")? {
        return Err(anyhow!("unsupported state database: missing threads table"));
    }
    let columns = table_columns(&connection, "threads")?;
    let required = [
        "id",
        "rollout_path",
        "created_at",
        "updated_at",
        "source",
        "model_provider",
        "cwd",
        "title",
        "sandbox_policy",
        "approval_mode",
    ];
    for column in required {
        if !columns.contains(column) {
            return Err(anyhow!(
                "unsupported threads schema: missing required column {column}"
            ));
        }
    }

    let transaction = connection.transaction()?;
    let exists: bool = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM threads WHERE id = ?1)",
        [&thread.id],
        |row| row.get(0),
    )?;
    if exists {
        update_thread(&transaction, &columns, thread, rollout_path, cwd)?;
    } else {
        insert_thread(&transaction, &columns, thread, rollout_path, cwd)?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn promote_thread(path: &Path, thread_id: &str, updated_at: i64) -> Result<()> {
    let connection = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let columns = table_columns(&connection, "threads")?;
    let mut assignments = Vec::new();
    let mut values = Vec::new();
    let mut add = |name: &str, value: Value| {
        if columns.contains(name) {
            assignments.push(format!("{name}=?{}", values.len() + 1));
            values.push(value);
        }
    };
    add("updated_at", Value::Integer(updated_at));
    add(
        "updated_at_ms",
        Value::Integer(updated_at.saturating_mul(1000)),
    );
    add("recency_at", Value::Integer(updated_at));
    add(
        "recency_at_ms",
        Value::Integer(updated_at.saturating_mul(1000)),
    );
    values.push(Value::Text(thread_id.to_owned()));
    let sql = format!(
        "UPDATE threads SET {} WHERE id=?{}",
        assignments.join(", "),
        values.len()
    );
    let changed = connection.execute(&sql, params_from_iter(values))?;
    if changed != 1 {
        return Err(anyhow!(
            "cannot promote missing thread {thread_id} in {}",
            path.display()
        ));
    }
    Ok(())
}

fn update_thread(
    connection: &Connection,
    columns: &HashSet<String>,
    thread: &ThreadRecord,
    rollout_path: &Path,
    cwd: &Path,
) -> Result<()> {
    let mut assignments = Vec::new();
    let mut values = Vec::new();
    let mut add = |name: &str, value: Value| {
        if columns.contains(name) {
            assignments.push(format!("{name}=?{}", values.len() + 1));
            values.push(value);
        }
    };
    add(
        "rollout_path",
        Value::Text(rollout_path.to_string_lossy().into_owned()),
    );
    add("cwd", Value::Text(cwd.to_string_lossy().into_owned()));
    add("title", Value::Text(thread.title.clone()));
    add("created_at", Value::Integer(thread.created_at));
    add("updated_at", Value::Integer(thread.updated_at));
    add("source", Value::Text(thread.source.clone()));
    add(
        "thread_source",
        Value::Text(
            thread
                .thread_source
                .clone()
                .unwrap_or_else(|| "user".to_owned()),
        ),
    );
    add("model_provider", Value::Text(thread.model_provider.clone()));
    add("archived", Value::Integer(thread.archived as i64));
    add(
        "archived_at",
        if thread.archived {
            Value::Integer(thread.updated_at)
        } else {
            Value::Null
        },
    );
    add("cli_version", Value::Text(thread.cli_version.clone()));
    add(
        "first_user_message",
        Value::Text(thread.first_user_message.clone()),
    );
    add("preview", Value::Text(preview_text(thread)));
    add(
        "model",
        thread.model.clone().map(Value::Text).unwrap_or(Value::Null),
    );
    add(
        "reasoning_effort",
        thread
            .reasoning_effort
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
    );
    add(
        "created_at_ms",
        Value::Integer(thread.created_at.saturating_mul(1000)),
    );
    add(
        "updated_at_ms",
        Value::Integer(thread.updated_at.saturating_mul(1000)),
    );
    add("recency_at", Value::Integer(thread.updated_at));
    add(
        "recency_at_ms",
        Value::Integer(thread.updated_at.saturating_mul(1000)),
    );
    values.push(Value::Text(thread.id.clone()));
    let sql = format!(
        "UPDATE threads SET {} WHERE id=?{}",
        assignments.join(", "),
        values.len()
    );
    connection.execute(&sql, params_from_iter(values))?;
    Ok(())
}

fn insert_thread(
    connection: &Connection,
    columns: &HashSet<String>,
    thread: &ThreadRecord,
    rollout_path: &Path,
    cwd: &Path,
) -> Result<()> {
    let mut names = Vec::new();
    let mut values = Vec::new();
    let mut add = |name: &str, value: Value| {
        if columns.contains(name) {
            names.push(name.to_owned());
            values.push(value);
        }
    };
    add("id", Value::Text(thread.id.clone()));
    add(
        "rollout_path",
        Value::Text(rollout_path.to_string_lossy().into_owned()),
    );
    add("created_at", Value::Integer(thread.created_at));
    add("updated_at", Value::Integer(thread.updated_at));
    add("source", Value::Text(thread.source.clone()));
    add(
        "thread_source",
        Value::Text(
            thread
                .thread_source
                .clone()
                .unwrap_or_else(|| "user".to_owned()),
        ),
    );
    add("model_provider", Value::Text(thread.model_provider.clone()));
    add("cwd", Value::Text(cwd.to_string_lossy().into_owned()));
    add("title", Value::Text(thread.title.clone()));
    add(
        "sandbox_policy",
        Value::Text(
            thread
                .sandbox_policy
                .clone()
                .unwrap_or_else(default_sandbox_policy),
        ),
    );
    add(
        "approval_mode",
        Value::Text(
            thread
                .approval_mode
                .clone()
                .unwrap_or_else(|| "on-request".to_owned()),
        ),
    );
    add("tokens_used", Value::Integer(0));
    add("has_user_event", Value::Integer(1));
    add("archived", Value::Integer(thread.archived as i64));
    add(
        "archived_at",
        if thread.archived {
            Value::Integer(thread.updated_at)
        } else {
            Value::Null
        },
    );
    add("cli_version", Value::Text(thread.cli_version.clone()));
    add(
        "first_user_message",
        Value::Text(thread.first_user_message.clone()),
    );
    add("preview", Value::Text(preview_text(thread)));
    add("memory_mode", Value::Text("enabled".to_owned()));
    add(
        "model",
        thread.model.clone().map(Value::Text).unwrap_or(Value::Null),
    );
    add(
        "reasoning_effort",
        thread
            .reasoning_effort
            .clone()
            .map(Value::Text)
            .unwrap_or(Value::Null),
    );
    add(
        "created_at_ms",
        Value::Integer(thread.created_at.saturating_mul(1000)),
    );
    add(
        "updated_at_ms",
        Value::Integer(thread.updated_at.saturating_mul(1000)),
    );
    add("recency_at", Value::Integer(thread.updated_at));
    add(
        "recency_at_ms",
        Value::Integer(thread.updated_at.saturating_mul(1000)),
    );

    let placeholders = (1..=names.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO threads ({}) VALUES ({})",
        names.join(", "),
        placeholders
    );
    connection.execute(&sql, params_from_iter(values))?;
    Ok(())
}

fn table_columns(connection: &Connection, table: &str) -> Result<HashSet<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<rusqlite::Result<HashSet<_>>>()
        .map_err(Into::into)
}

fn table_exists(connection: &Connection, table: &str) -> Result<bool> {
    let exists: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
            [table],
            |row| row.get(0),
        )
        .optional()?;
    Ok(exists.is_some())
}

fn default_sandbox_policy() -> String {
    r#"{"type":"workspace-write","writable_roots":[],"network_access":false}"#.to_owned()
}

fn preview_text(thread: &ThreadRecord) -> String {
    if thread.first_user_message.trim().is_empty() {
        thread.title.clone()
    } else {
        thread.first_user_message.clone()
    }
}

pub(crate) fn open_readable(path: &Path) -> Result<Connection> {
    let wal = std::path::PathBuf::from(format!("{}-wal", path.to_string_lossy()));
    if !wal.exists() {
        // WAL-mode databases without a live WAL/SHM pair cannot be queried through
        // SQLite's strict read-only mode. A normal connection safely recreates the
        // transient sidecars without changing application rows.
        return Connection::open(path).map_err(Into::into);
    }
    Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(Into::into)
}
