use crate::model::PlannedThread;
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

pub fn path(home: &Path) -> PathBuf {
    home.join("session_index.jsonl")
}

pub fn load(home: &Path) -> Result<BTreeMap<String, Value>> {
    let index_path = path(home);
    if !index_path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text = fs::read_to_string(&index_path)
        .with_context(|| format!("read {}", index_path.display()))?;
    let mut entries = BTreeMap::new();
    for (line_number, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line).with_context(|| {
            format!("parse line {} in {}", line_number + 1, index_path.display())
        })?;
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            entries.insert(id.to_owned(), value);
        }
    }
    Ok(entries)
}

pub fn sync_imported(
    target_home: &Path,
    source_home: &Path,
    planned: &[PlannedThread],
) -> Result<BTreeMap<String, i64>> {
    let mut target = load(target_home)?;
    let source = load(source_home)?;
    let base_epoch = Utc::now().timestamp();
    let mut promoted = BTreeMap::new();

    for (offset, item) in planned.iter().enumerate() {
        let epoch = base_epoch.saturating_sub(offset as i64);
        let updated_at = chrono::DateTime::from_timestamp(epoch, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        let existing_name = target
            .get(&item.thread.id)
            .and_then(|entry| entry.get("thread_name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let source_name = source
            .get(&item.thread.id)
            .and_then(|entry| entry.get("thread_name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        let thread_name = existing_name
            .or(source_name)
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| {
                if item.thread.title.trim().is_empty() {
                    item.thread.id.clone()
                } else {
                    item.thread.title.clone()
                }
            });
        let mut entry = target
            .remove(&item.thread.id)
            .or_else(|| source.get(&item.thread.id).cloned())
            .unwrap_or_else(|| Value::Object(Map::new()));
        if !entry.is_object() {
            entry = Value::Object(Map::new());
        }
        let object = entry
            .as_object_mut()
            .expect("entry was converted to object");
        object.insert("id".to_owned(), Value::String(item.thread.id.clone()));
        object.insert("thread_name".to_owned(), Value::String(thread_name));
        object.insert("updated_at".to_owned(), Value::String(updated_at));
        target.insert(item.thread.id.clone(), entry);
        promoted.insert(item.thread.id.clone(), epoch);
    }

    write(target_home, &target)?;
    Ok(promoted)
}

pub fn verify_contains(home: &Path, expected_ids: impl IntoIterator<Item = String>) -> Result<()> {
    let entries = load(home)?;
    let expected = expected_ids.into_iter().collect::<BTreeSet<_>>();
    let actual = entries.keys().cloned().collect::<BTreeSet<_>>();
    let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!(
            "threads are missing from session_index.jsonl: {}",
            missing.join(", ")
        );
    }
    Ok(())
}

fn write(home: &Path, entries: &BTreeMap<String, Value>) -> Result<()> {
    fs::create_dir_all(home)?;
    let index_path = path(home);
    let temporary = index_path.with_extension("jsonl.tmp");
    let mut output = String::new();
    for entry in entries.values() {
        output.push_str(&serde_json::to_string(entry)?);
        output.push('\n');
    }
    fs::write(&temporary, output)?;
    fs::rename(&temporary, &index_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{MergeAction, ThreadRecord};
    use tempfile::TempDir;

    fn planned(id: &str, title: &str) -> PlannedThread {
        PlannedThread {
            thread: ThreadRecord {
                id: id.to_owned(),
                title: title.to_owned(),
                created_at: 1,
                updated_at: 2,
                cwd: "/old".to_owned(),
                source: "vscode".to_owned(),
                thread_source: Some("user".to_owned()),
                model_provider: "openai".to_owned(),
                cli_version: String::new(),
                archived: false,
                archive_path: "sessions/test.jsonl".to_owned(),
                sha256: String::new(),
                byte_len: 0,
                first_user_message: title.to_owned(),
                sandbox_policy: None,
                approval_mode: None,
                model: None,
                reasoning_effort: None,
            },
            source_path: String::new(),
            mapped_cwd: "/new".to_owned(),
            history_only: false,
            target_path: String::new(),
            action: MergeAction::Import,
            reason: String::new(),
        }
    }

    #[test]
    fn preserves_source_sidebar_name_and_promotes_timestamp() {
        let source = TempDir::new().unwrap();
        let target = TempDir::new().unwrap();
        fs::write(
            path(source.path()),
            "{\"id\":\"thread-1\",\"thread_name\":\"短名称\",\"updated_at\":\"2020-01-01T00:00:00Z\"}\n",
        )
        .unwrap();
        sync_imported(
            target.path(),
            source.path(),
            &[planned("thread-1", "很长的标题")],
        )
        .unwrap();
        let entries = load(target.path()).unwrap();
        assert_eq!(entries["thread-1"]["thread_name"], "短名称");
        assert_ne!(entries["thread-1"]["updated_at"], "2020-01-01T00:00:00Z");
    }
}
