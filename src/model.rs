use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    Macos,
    Windows,
    Linux,
    Wsl,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThreadRecord {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub cwd: String,
    pub source: String,
    pub thread_source: Option<String>,
    pub model_provider: String,
    pub cli_version: String,
    pub archived: bool,
    pub archive_path: String,
    pub sha256: String,
    pub byte_len: u64,
    pub first_user_message: String,
    pub sandbox_policy: Option<String>,
    pub approval_mode: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScannedThread {
    pub record: ThreadRecord,
    pub source_path: PathBuf,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceCatalog {
    pub source_codex_home: String,
    pub source_platform: PlatformKind,
    pub source_codex_version: Option<String>,
    pub projects: Vec<SourceProject>,
    pub thread_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceProject {
    pub original_cwd: String,
    pub suggested_target: Option<String>,
    pub sessions: Vec<SourceSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceSession {
    pub thread: ThreadRecord,
    pub source_path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImportOptions {
    pub selected_thread_ids: BTreeSet<String>,
    pub mappings: BTreeMap<String, String>,
    pub history_only_projects: BTreeSet<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionRecord {
    pub id: String,
    pub created_at: String,
    pub codex_home: String,
    pub sqlite_home: String,
    pub source_codex_home: String,
    pub backups: Vec<BackupEntry>,
    pub created_files: Vec<String>,
    pub replaced_files: Vec<BackupEntry>,
    pub completed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupEntry {
    pub original: String,
    pub backup: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MergeAction {
    Import,
    SkipIdentical,
    ReplaceWithLonger,
    KeepTargetLonger,
    Conflict,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedThread {
    pub thread: ThreadRecord,
    pub source_path: String,
    pub mapped_cwd: String,
    pub history_only: bool,
    pub target_path: String,
    pub action: MergeAction,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportPlan {
    pub source_codex_home: String,
    pub codex_home: String,
    pub threads: Vec<PlannedThread>,
    pub conflicts: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticReport {
    pub codex_home: String,
    pub sqlite_home: String,
    pub codex_executable: Option<String>,
    pub codex_version: Option<String>,
    pub platform: PlatformKind,
    pub wsl: bool,
    pub state_db: Option<String>,
    pub schema_version: Option<i64>,
    pub active_rollouts: usize,
    pub archived_rollouts: usize,
    pub database_threads: usize,
    pub integrity: Option<String>,
    pub issues: Vec<String>,
}
