use assert_cmd::Command;
use rusqlite::Connection;
use serde_json::json;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn exports_scans_and_dry_runs_directory_backup() {
    let source = TempDir::new().unwrap();
    create_codex_home(source.path(), "11111111-2222-3333-4444-555555555555");
    fs::write(source.path().join("auth.json"), "{}").unwrap();
    fs::create_dir_all(source.path().join("skills/example")).unwrap();
    fs::write(source.path().join("skills/example/SKILL.md"), "secret").unwrap();

    let destination = TempDir::new().unwrap();
    Command::cargo_bin("codex-migrate")
        .unwrap()
        .args([
            "export",
            source.path().to_str().unwrap(),
            "--output-parent",
            destination.path().to_str().unwrap(),
        ])
        .assert()
        .success();

    let backup = destination.path().join("Codex");
    assert!(backup.join("sessions").is_dir());
    assert!(backup.join("session_index.jsonl").is_file());
    assert!(!backup.join("auth.json").exists());
    assert!(!backup.join("skills").exists());
    assert!(!backup.join("state_5.sqlite").exists());

    Command::cargo_bin("codex-migrate")
        .unwrap()
        .args(["scan", destination.path().to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"thread_count\": 1"))
        .stdout(predicates::str::contains("\"Test thread\""));

    let target = TempDir::new().unwrap();
    create_empty_state_db(target.path());
    let mapped_root = target.path().join("projects");
    let mapping = format!("/old/project={}", mapped_root.display());
    Command::cargo_bin("codex-migrate")
        .unwrap()
        .args([
            "import",
            backup.to_str().unwrap(),
            "--codex-home",
            target.path().to_str().unwrap(),
            "--dry-run",
            "--map",
            &mapping,
        ])
        .assert()
        .success()
        .stdout(predicates::str::contains("\"action\": \"import\""))
        .stdout(predicates::str::contains(
            mapped_root.join("demo").to_string_lossy().as_ref(),
        ));
}

#[test]
fn scan_without_sqlite_uses_rollout_title_and_groups_by_project() {
    let source = TempDir::new().unwrap();
    create_rollout(
        source.path(),
        "11111111-1111-1111-1111-111111111111",
        "/old/project/demo",
        "First session",
        false,
    );
    create_rollout(
        source.path(),
        "22222222-2222-2222-2222-222222222222",
        "/old/project/demo",
        "Second session",
        true,
    );

    let catalog = codex_migrate::operations::scan_source(source.path()).unwrap();
    assert_eq!(catalog.thread_count, 2);
    assert_eq!(catalog.projects.len(), 1);
    assert_eq!(catalog.projects[0].sessions.len(), 2);
    assert!(catalog.projects[0]
        .sessions
        .iter()
        .any(|session| session.thread.title == "First session"));
    assert!(catalog.projects[0]
        .sessions
        .iter()
        .any(|session| session.thread.archived));
}

#[test]
fn doctor_detects_stale_database_path() {
    let home = TempDir::new().unwrap();
    create_codex_home(home.path(), "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee");
    let db = Connection::open(home.path().join("state_5.sqlite")).unwrap();
    db.execute(
        "UPDATE threads SET rollout_path='/missing/rollout.jsonl'",
        [],
    )
    .unwrap();

    Command::cargo_bin("codex-migrate")
        .unwrap()
        .args(["doctor", "--codex-home", home.path().to_str().unwrap()])
        .assert()
        .failure();
}

#[test]
fn imports_selected_thread_with_sqlite_fallback_and_rolls_back() {
    let source = TempDir::new().unwrap();
    let selected_id = "99999999-2222-3333-4444-555555555555";
    let excluded_id = "88888888-2222-3333-4444-555555555555";
    create_codex_home(source.path(), selected_id);
    create_rollout(
        source.path(),
        excluded_id,
        "/old/project/demo",
        "Excluded session",
        false,
    );

    let target = TempDir::new().unwrap();
    create_empty_state_db(target.path());
    let mapped_root = target.path().join("projects");
    let mapping = format!("/old/project={}", mapped_root.display());
    let output = Command::cargo_bin("codex-migrate")
        .unwrap()
        .env("PATH", "")
        .args([
            "import",
            source.path().to_str().unwrap(),
            "--codex-home",
            target.path().to_str().unwrap(),
            "--thread",
            selected_id,
            "--map",
            &mapping,
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    let transaction_id = stdout
        .lines()
        .find_map(|line| line.strip_prefix("import completed; transaction id: "))
        .unwrap();

    let db = Connection::open(target.path().join("state_5.sqlite")).unwrap();
    let (cwd, rollout): (String, String) = db
        .query_row(
            "SELECT cwd, rollout_path FROM threads WHERE id=?1",
            [selected_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(cwd, mapped_root.join("demo").to_string_lossy());
    assert!(Path::new(&rollout).is_file());
    let excluded_count: i64 = db
        .query_row(
            "SELECT count(*) FROM threads WHERE id=?1",
            [excluded_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(excluded_count, 0);
    drop(db);

    let repeated = Command::cargo_bin("codex-migrate")
        .unwrap()
        .env("PATH", "")
        .args([
            "import",
            source.path().to_str().unwrap(),
            "--codex-home",
            target.path().to_str().unwrap(),
            "--thread",
            selected_id,
            "--dry-run",
            "--map",
            &mapping,
        ])
        .output()
        .unwrap();
    assert!(repeated.status.success());
    assert!(String::from_utf8_lossy(&repeated.stdout).contains("\"action\": \"skip_identical\""));

    Command::cargo_bin("codex-migrate")
        .unwrap()
        .env("PATH", "")
        .args([
            "rollback",
            transaction_id,
            "--codex-home",
            target.path().to_str().unwrap(),
        ])
        .assert()
        .success();
    let db = Connection::open(target.path().join("state_5.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM threads WHERE id=?1",
            [selected_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn history_only_uses_one_stable_project_directory() {
    let source = TempDir::new().unwrap();
    create_rollout(
        source.path(),
        "33333333-3333-3333-3333-333333333333",
        "C:/missing/project",
        "History one",
        false,
    );
    create_rollout(
        source.path(),
        "44444444-4444-4444-4444-444444444444",
        "C:/missing/project",
        "History two",
        false,
    );
    let target = TempDir::new().unwrap();
    create_empty_state_db(target.path());

    let output = Command::cargo_bin("codex-migrate")
        .unwrap()
        .args([
            "import",
            source.path().to_str().unwrap(),
            "--codex-home",
            target.path().to_str().unwrap(),
            "--dry-run",
            "--history-only",
            "C:/missing/project",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let plan: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let threads = plan["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[0]["mapped_cwd"], threads[1]["mapped_cwd"]);
    assert!(threads[0]["mapped_cwd"]
        .as_str()
        .unwrap()
        .contains("migration_history"));
}

#[test]
fn repeated_import_repairs_visibility_and_registers_project() {
    let source = TempDir::new().unwrap();
    let id = "55555555-5555-5555-5555-555555555555";
    create_codex_home(source.path(), id);
    let target = TempDir::new().unwrap();
    create_empty_state_db(target.path());
    let mapped_root = target.path().join("projects");
    let mapped_project = mapped_root.join("demo");
    let mapping = format!("/old/project={}", mapped_root.display());

    for attempt in 0..2 {
        Command::cargo_bin("codex-migrate")
            .unwrap()
            .env("PATH", "")
            .args([
                "import",
                source.path().to_str().unwrap(),
                "--codex-home",
                target.path().to_str().unwrap(),
                "--thread",
                id,
                "--map",
                &mapping,
            ])
            .assert()
            .success();
        let db = Connection::open(target.path().join("state_5.sqlite")).unwrap();
        let (preview, thread_source, cwd): (String, String, String) = db
            .query_row(
                "SELECT preview, thread_source, cwd FROM threads WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(preview, "hello");
        assert_eq!(thread_source, "user");
        assert_eq!(cwd, mapped_project.to_string_lossy());
        drop(db);
        let state: serde_json::Value = serde_json::from_slice(
            &fs::read(target.path().join(".codex-global-state.json")).unwrap(),
        )
        .unwrap();
        let current_projects = state["electron-saved-workspace-roots"].as_array().unwrap();
        assert!(current_projects
            .iter()
            .any(|value| value.as_str() == Some(mapped_project.to_string_lossy().as_ref())));
        let current_order = state["project-order"].as_array().unwrap();
        assert!(current_order
            .iter()
            .any(|value| value.as_str() == Some(mapped_project.to_string_lossy().as_ref())));
        let session_index = fs::read_to_string(target.path().join("session_index.jsonl")).unwrap();
        let imported_index_entry = session_index
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|entry| entry["id"] == id)
            .unwrap();
        assert_eq!(imported_index_entry["thread_name"], "Test thread");
        assert!(imported_index_entry["updated_at"]
            .as_str()
            .is_some_and(|value| value.starts_with("20")));
        let rollout = fs::read_to_string(
            target
                .path()
                .join("sessions/2026/06/19")
                .join(format!("rollout-2026-06-19T12-00-00-{id}.jsonl")),
        )
        .unwrap();
        assert!(rollout.contains(&format!("\"cwd\":\"{}\"", mapped_project.to_string_lossy())));
        assert!(!rollout.contains("\"cwd\":\"/old/project/demo\""));
        let projects = state["electron-persisted-atom-state"]["electron-saved-workspace-roots"]
            .as_array()
            .unwrap();
        assert!(projects
            .iter()
            .any(|value| value.as_str() == Some(mapped_project.to_string_lossy().as_ref())));
        if attempt == 0 {
            let db = Connection::open(target.path().join("state_5.sqlite")).unwrap();
            db.execute(
                "UPDATE threads SET preview='', thread_source=NULL WHERE id=?1",
                [id],
            )
            .unwrap();
        }
    }
}

fn create_codex_home(home: &Path, id: &str) {
    create_empty_state_db(home);
    let rollout = create_rollout(home, id, "/old/project/demo", "Test thread", false);
    let db = Connection::open(home.join("state_5.sqlite")).unwrap();
    db.execute(
        "INSERT INTO threads (
            id, rollout_path, created_at, updated_at, source, model_provider,
            cwd, title, sandbox_policy, approval_mode, cli_version,
            first_user_message, archived
         ) VALUES (?1, ?2, 1781841600, 1781841602, 'vscode', 'openai',
                   '/old/project/demo', 'Test thread', '{}', 'on-request',
                   '0.142.0', 'hello', 0)",
        rusqlite::params![id, rollout.to_string_lossy()],
    )
    .unwrap();
}

fn create_rollout(
    home: &Path,
    id: &str,
    cwd: &str,
    title: &str,
    archived: bool,
) -> std::path::PathBuf {
    let rollout = if archived {
        home.join("archived_sessions")
            .join(format!("rollout-2026-06-19T12-00-00-{id}.jsonl"))
    } else {
        home.join("sessions")
            .join("2026")
            .join("06")
            .join("19")
            .join(format!("rollout-2026-06-19T12-00-00-{id}.jsonl"))
    };
    fs::create_dir_all(rollout.parent().unwrap()).unwrap();
    let lines = [
        json!({
            "timestamp": "2026-06-19T04:00:00Z",
            "type": "session_meta",
            "payload": {
                "id": id,
                "timestamp": "2026-06-19T04:00:00Z",
                "cwd": cwd,
                "source": "vscode",
                "model_provider": "openai",
                "cli_version": "0.142.0"
            }
        }),
        json!({
            "timestamp": "2026-06-19T04:00:01Z",
            "type": "event_msg",
            "payload": {"type": "user_message", "message": "hello"}
        }),
        json!({
            "timestamp": "2026-06-19T04:00:02Z",
            "type": "event_msg",
            "payload": {
                "type": "thread_name_updated",
                "thread_id": id,
                "thread_name": title
            }
        }),
    ];
    let content = lines
        .iter()
        .map(serde_json::Value::to_string)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    fs::write(&rollout, content).unwrap();
    rollout
}

fn create_empty_state_db(home: &Path) {
    fs::create_dir_all(home).unwrap();
    let db = Connection::open(home.join("state_5.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
            version INTEGER PRIMARY KEY,
            description TEXT NOT NULL,
            installed_on TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            success INTEGER NOT NULL,
            checksum BLOB NOT NULL DEFAULT X'',
            execution_time INTEGER NOT NULL DEFAULT 0
         );
         INSERT OR IGNORE INTO _sqlx_migrations(version, description, success)
         VALUES (1, 'threads', 1);
         CREATE TABLE IF NOT EXISTS threads (
            id TEXT PRIMARY KEY,
            rollout_path TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            source TEXT NOT NULL,
            model_provider TEXT NOT NULL,
            cwd TEXT NOT NULL,
            title TEXT NOT NULL,
            sandbox_policy TEXT NOT NULL,
            approval_mode TEXT NOT NULL,
            tokens_used INTEGER NOT NULL DEFAULT 0,
            has_user_event INTEGER NOT NULL DEFAULT 0,
            archived INTEGER NOT NULL DEFAULT 0,
            archived_at INTEGER,
            cli_version TEXT NOT NULL DEFAULT '',
            first_user_message TEXT NOT NULL DEFAULT '',
            memory_mode TEXT NOT NULL DEFAULT 'enabled',
            model TEXT,
            reasoning_effort TEXT,
            created_at_ms INTEGER,
            updated_at_ms INTEGER
            ,thread_source TEXT
            ,preview TEXT NOT NULL DEFAULT ''
            ,recency_at INTEGER NOT NULL DEFAULT 0
            ,recency_at_ms INTEGER NOT NULL DEFAULT 0
         );",
    )
    .unwrap();
}
