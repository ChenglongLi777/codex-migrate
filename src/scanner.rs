use crate::model::{ScannedThread, ThreadRecord};
use crate::sqlite_adapter;
use anyhow::{anyhow, Context, Result};
use rusqlite::{Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use walkdir::WalkDir;

#[derive(Debug, Default)]
struct DbMetadata {
    title: String,
    created_at: i64,
    updated_at: i64,
    cwd: String,
    source: String,
    thread_source: Option<String>,
    model_provider: String,
    cli_version: String,
    first_user_message: String,
    sandbox_policy: Option<String>,
    approval_mode: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<String>,
}

pub fn scan_codex_home(home: &Path, state_db: Option<&Path>) -> Result<Vec<ScannedThread>> {
    let db_metadata = state_db
        .map(load_db_metadata)
        .transpose()?
        .unwrap_or_default();
    let mut result = Vec::new();
    scan_directory(&home.join("sessions"), false, &db_metadata, &mut result)?;
    scan_directory(
        &home.join("archived_sessions"),
        true,
        &db_metadata,
        &mut result,
    )?;
    result.sort_by(|left, right| {
        left.record
            .updated_at
            .cmp(&right.record.updated_at)
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    Ok(result)
}

fn scan_directory(
    root: &Path,
    archived: bool,
    db_metadata: &HashMap<String, DbMetadata>,
    output: &mut Vec<ScannedThread>,
) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    for entry in WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            continue;
        }
        output.push(scan_rollout(path, root, archived, db_metadata)?);
    }
    Ok(())
}

fn scan_rollout(
    path: &Path,
    root: &Path,
    archived: bool,
    db_metadata: &HashMap<String, DbMetadata>,
) -> Result<ScannedThread> {
    let content = fs::read(path).with_context(|| format!("read rollout {}", path.display()))?;
    if content.is_empty() {
        return Err(anyhow!("empty rollout: {}", path.display()));
    }
    let reader = BufReader::new(content.as_slice());
    let mut id = None;
    let mut cwd = None;
    let mut timestamp = None;
    let mut source = None;
    let mut thread_source = None;
    let mut provider = None;
    let mut cli_version = None;
    let mut title = None;
    let mut first_user_message = None;

    for (index, line) in reader.lines().enumerate() {
        let line =
            line.with_context(|| format!("read line {} in {}", index + 1, path.display()))?;
        let value: Value = serde_json::from_str(&line)
            .with_context(|| format!("invalid JSON line {} in {}", index + 1, path.display()))?;
        match value.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                let payload = &value["payload"];
                id = payload.get("id").and_then(Value::as_str).map(str::to_owned);
                cwd = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                timestamp = payload
                    .get("timestamp")
                    .and_then(Value::as_str)
                    .and_then(parse_timestamp);
                source = payload
                    .get("source")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                thread_source = payload
                    .get("thread_source")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                provider = payload
                    .get("model_provider")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                cli_version = payload
                    .get("cli_version")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            Some("event_msg") => {
                let payload = &value["payload"];
                if payload.get("type").and_then(Value::as_str) == Some("thread_name_updated") {
                    title = payload
                        .get("thread_name")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if first_user_message.is_none()
                    && payload.get("type").and_then(Value::as_str) == Some("user_message")
                {
                    first_user_message = payload
                        .get("message")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
            }
            Some("response_item") if first_user_message.is_none() => {
                let payload = &value["payload"];
                if payload.get("type").and_then(Value::as_str) == Some("message")
                    && payload.get("role").and_then(Value::as_str) == Some("user")
                {
                    first_user_message = extract_message_text(payload);
                }
            }
            _ => {}
        }
    }

    let id = id
        .or_else(|| id_from_filename(path))
        .ok_or_else(|| anyhow!("cannot determine thread id for {}", path.display()))?;
    let metadata = db_metadata.get(&id);
    let sha256 = hex::encode(Sha256::digest(&content));
    let relative = path
        .strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    let archive_path = if archived {
        format!("archived_sessions/{relative}")
    } else {
        format!("sessions/{relative}")
    };

    let record = ThreadRecord {
        id,
        title: metadata
            .map(|value| value.title.clone())
            .filter(|value| !value.is_empty())
            .or_else(|| title.filter(|value| !value.is_empty()))
            .or_else(|| first_user_message.as_deref().map(summarize_title))
            .unwrap_or_default(),
        created_at: metadata
            .map(|value| value.created_at)
            .or(timestamp)
            .unwrap_or_default(),
        updated_at: metadata
            .map(|value| value.updated_at)
            .or(timestamp)
            .unwrap_or_default(),
        cwd: cwd
            .or_else(|| metadata.map(|value| value.cwd.clone()))
            .unwrap_or_default(),
        source: source
            .or_else(|| metadata.map(|value| value.source.clone()))
            .unwrap_or_else(|| "unknown".to_owned()),
        thread_source: thread_source
            .or_else(|| metadata.and_then(|value| value.thread_source.clone())),
        model_provider: provider
            .or_else(|| metadata.map(|value| value.model_provider.clone()))
            .unwrap_or_else(|| "openai".to_owned()),
        cli_version: cli_version
            .or_else(|| metadata.map(|value| value.cli_version.clone()))
            .unwrap_or_default(),
        archived,
        archive_path,
        sha256,
        byte_len: content.len() as u64,
        first_user_message: metadata
            .map(|value| value.first_user_message.clone())
            .filter(|value| !value.is_empty())
            .or(first_user_message)
            .unwrap_or_default(),
        sandbox_policy: metadata.and_then(|value| value.sandbox_policy.clone()),
        approval_mode: metadata.and_then(|value| value.approval_mode.clone()),
        model: metadata.and_then(|value| value.model.clone()),
        reasoning_effort: metadata.and_then(|value| value.reasoning_effort.clone()),
    };
    Ok(ScannedThread {
        record,
        source_path: path.to_path_buf(),
        content,
    })
}

fn load_db_metadata(path: &Path) -> Result<HashMap<String, DbMetadata>> {
    let connection = sqlite_adapter::open_readable(path)
        .with_context(|| format!("open state DB {}", path.display()))?;
    let has_threads: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='threads'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if has_threads.is_none() {
        return Ok(HashMap::new());
    }
    let columns = table_columns(&connection, "threads")?;
    let optional = |name: &str, fallback: &str| {
        if columns.iter().any(|column| column == name) {
            name.to_owned()
        } else {
            fallback.to_owned()
        }
    };
    let sql = format!(
        "SELECT id, title, created_at, updated_at, cwd, source, model_provider, \
         {}, {}, {}, {}, {}, {}, {} FROM threads",
        optional("cli_version", "''"),
        optional("first_user_message", "''"),
        optional("sandbox_policy", "NULL"),
        optional("approval_mode", "NULL"),
        optional("model", "NULL"),
        optional("reasoning_effort", "NULL"),
        optional("thread_source", "NULL"),
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            DbMetadata {
                title: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                cwd: row.get(4)?,
                source: row.get(5)?,
                model_provider: row.get(6)?,
                cli_version: row.get(7)?,
                first_user_message: row.get(8)?,
                sandbox_policy: row.get(9)?,
                approval_mode: row.get(10)?,
                model: row.get(11)?,
                reasoning_effort: row.get(12)?,
                thread_source: row.get(13)?,
            },
        ))
    })?;
    let mut result = HashMap::new();
    for row in rows {
        let (id, metadata) = row?;
        result.insert(id, metadata);
    }
    Ok(result)
}

fn table_columns(connection: &Connection, table: &str) -> Result<Vec<String>> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(Into::into)
}

fn parse_timestamp(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.timestamp())
}

fn id_from_filename(path: &Path) -> Option<String> {
    let stem = path.file_stem()?.to_str()?;
    let suffix = stem.rsplit('-').take(5).collect::<Vec<_>>();
    if suffix.len() != 5 {
        return None;
    }
    Some(suffix.into_iter().rev().collect::<Vec<_>>().join("-"))
}

fn extract_message_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?.as_array()?;
    for item in content {
        if let Some(text) = item.get("text").and_then(Value::as_str) {
            return Some(text.to_owned());
        }
    }
    None
}

fn summarize_title(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(80).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn rejects_malformed_jsonl() {
        let home = TempDir::new().unwrap();
        let sessions = home.path().join("sessions");
        fs::create_dir_all(&sessions).unwrap();
        fs::write(
            sessions.join("rollout-2026-06-19T12-00-00-11111111-2222-3333-4444-555555555555.jsonl"),
            b"{not-json}\n",
        )
        .unwrap();
        assert!(scan_codex_home(home.path(), None).is_err());
    }

    #[test]
    fn scans_archived_unicode_session() {
        let home = TempDir::new().unwrap();
        let archived = home.path().join("archived_sessions");
        fs::create_dir_all(&archived).unwrap();
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let line = serde_json::json!({
            "timestamp": "2026-06-19T04:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": id,
                "timestamp": "2026-06-19T04:00:00Z",
                "cwd": "/项目/论文",
                "source": "vscode",
                "model_provider": "openai",
                "cli_version": "0.142.0"
            }
        });
        fs::write(
            archived.join(format!("rollout-2026-06-19T12-00-00-{id}.jsonl")),
            format!("{line}\n"),
        )
        .unwrap();
        let threads = scan_codex_home(home.path(), None).unwrap();
        assert_eq!(threads.len(), 1);
        assert!(threads[0].record.archived);
        assert_eq!(threads[0].record.cwd, "/项目/论文");
    }
}
