use crate::model::{BackupEntry, TransactionRecord};
use anyhow::{anyhow, Context, Result};
use chrono::Utc;
use rusqlite::backup::Backup;
use rusqlite::Connection;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub struct ImportTransaction {
    pub root: PathBuf,
    pub record: TransactionRecord,
}

impl ImportTransaction {
    pub fn begin(codex_home: &Path, sqlite_home: &Path, source_codex_home: &Path) -> Result<Self> {
        let id = format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%SZ"),
            Uuid::new_v4().simple()
        );
        let root = codex_home.join("migration_transactions").join(&id);
        fs::create_dir_all(root.join("backups"))?;
        let record = TransactionRecord {
            id,
            created_at: Utc::now().to_rfc3339(),
            codex_home: codex_home.to_string_lossy().into_owned(),
            sqlite_home: sqlite_home.to_string_lossy().into_owned(),
            source_codex_home: source_codex_home.to_string_lossy().into_owned(),
            backups: Vec::new(),
            created_files: Vec::new(),
            replaced_files: Vec::new(),
            completed: false,
        };
        let mut transaction = Self { root, record };
        transaction.persist()?;
        Ok(transaction)
    }

    pub fn backup_sqlite_family(&mut self, state_db: &Path) -> Result<()> {
        let name = format!(
            "{}-{}",
            self.record.backups.len(),
            state_db
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("state.sqlite")
        );
        let backup_path = self.root.join("backups").join(name);
        let source = Connection::open(state_db)?;
        let mut destination = Connection::open(&backup_path)?;
        let backup = Backup::new(&source, &mut destination)?;
        backup.run_to_completion(32, std::time::Duration::from_millis(20), None)?;
        drop(backup);
        drop(destination);
        self.record.backups.push(BackupEntry {
            original: state_db.to_string_lossy().into_owned(),
            backup: backup_path.to_string_lossy().into_owned(),
        });
        let text = state_db.to_string_lossy();
        for sidecar in [
            PathBuf::from(format!("{text}-wal")),
            PathBuf::from(format!("{text}-shm")),
        ] {
            self.record
                .created_files
                .push(sidecar.to_string_lossy().into_owned());
        }
        self.persist()
    }

    pub fn note_created(&mut self, path: &Path) -> Result<()> {
        self.record
            .created_files
            .push(path.to_string_lossy().into_owned());
        self.persist()
    }

    pub fn backup_replaced(&mut self, path: &Path) -> Result<()> {
        self.backup_file(path, true)
    }

    pub fn complete(&mut self) -> Result<()> {
        self.record.completed = true;
        self.persist()
    }

    pub fn rollback(&self) -> Result<()> {
        rollback_record(&self.record)
    }

    fn backup_file(&mut self, path: &Path, replaced: bool) -> Result<()> {
        let name = format!(
            "{}-{}",
            self.record.backups.len() + self.record.replaced_files.len(),
            path.file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("backup")
        );
        let backup = self.root.join("backups").join(name);
        fs::copy(path, &backup)
            .with_context(|| format!("backup {} to {}", path.display(), backup.display()))?;
        let entry = BackupEntry {
            original: path.to_string_lossy().into_owned(),
            backup: backup.to_string_lossy().into_owned(),
        };
        if replaced {
            self.record.replaced_files.push(entry);
        } else {
            self.record.backups.push(entry);
        }
        self.persist()
    }

    fn persist(&mut self) -> Result<()> {
        let final_path = self.root.join("transaction.json");
        let temporary = self.root.join("transaction.json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(&self.record)?)?;
        fs::rename(temporary, final_path)?;
        Ok(())
    }
}

pub fn rollback_by_id(codex_home: &Path, id: &str) -> Result<()> {
    let path = transaction_directory(codex_home, id)?.join("transaction.json");
    let bytes =
        fs::read(&path).with_context(|| format!("read transaction record {}", path.display()))?;
    let record: TransactionRecord = serde_json::from_slice(&bytes)?;
    rollback_record(&record)
}

pub fn delete_by_ids(codex_home: &Path, ids: &[String]) -> Result<usize> {
    let mut deleted = 0;
    for id in ids {
        let directory = transaction_directory(codex_home, id)?;
        if directory.is_dir() {
            fs::remove_dir_all(&directory)
                .with_context(|| format!("delete rollback data {}", directory.display()))?;
            deleted += 1;
        }
    }
    Ok(deleted)
}

fn transaction_directory(codex_home: &Path, id: &str) -> Result<PathBuf> {
    if id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || Path::new(id).components().count() != 1
    {
        return Err(anyhow!("invalid transaction id"));
    }
    Ok(codex_home.join("migration_transactions").join(id))
}

fn rollback_record(record: &TransactionRecord) -> Result<()> {
    for path in record.created_files.iter().rev() {
        let path = Path::new(path);
        if path.is_file() {
            fs::remove_file(path)?;
        }
    }
    for entry in record
        .replaced_files
        .iter()
        .rev()
        .chain(record.backups.iter().rev())
    {
        let original = Path::new(&entry.original);
        let backup = Path::new(&entry.backup);
        if !backup.exists() {
            return Err(anyhow!("missing rollback backup {}", backup.display()));
        }
        if let Some(parent) = original.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(backup, original)?;
    }
    for entry in &record.backups {
        let original = Path::new(&entry.original);
        if original.extension().and_then(|value| value.to_str()) == Some("sqlite") {
            let connection = Connection::open(original)?;
            connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn deletes_only_selected_transaction_directories() {
        let home = TempDir::new().unwrap();
        let root = home.path().join("migration_transactions");
        fs::create_dir_all(root.join("first").join("backups")).unwrap();
        fs::create_dir_all(root.join("second").join("backups")).unwrap();

        let deleted = delete_by_ids(home.path(), &["first".to_owned()]).unwrap();

        assert_eq!(deleted, 1);
        assert!(!root.join("first").exists());
        assert!(root.join("second").exists());
        assert!(delete_by_ids(home.path(), &["../second".to_owned()]).is_err());
        assert!(root.join("second").exists());
    }
}
