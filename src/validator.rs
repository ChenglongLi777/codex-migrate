use crate::discovery::Environment;
use crate::model::DiagnosticReport;
use crate::{scanner, session_index, sqlite_adapter};
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

pub fn diagnose(environment: &Environment) -> Result<DiagnosticReport> {
    let threads =
        scanner::scan_codex_home(&environment.codex_home, environment.state_db.as_deref())?;
    let active_rollouts = threads
        .iter()
        .filter(|thread| !thread.record.archived)
        .count();
    let archived_rollouts = threads
        .iter()
        .filter(|thread| thread.record.archived)
        .count();
    let database_threads = environment
        .state_db
        .as_deref()
        .map(sqlite_adapter::count_threads)
        .transpose()?
        .unwrap_or(0);
    let integrity = environment
        .state_db
        .as_deref()
        .map(sqlite_adapter::integrity_check)
        .transpose()?;
    let mut issues = Vec::new();
    if !environment.codex_home.exists() {
        issues.push("CODEX_HOME does not exist".to_owned());
    }
    if environment.state_db.is_none() {
        issues.push("state database was not found".to_owned());
    }
    if integrity.as_deref().is_some_and(|value| value != "ok") {
        issues.push(format!(
            "SQLite integrity check failed: {}",
            integrity.as_deref().unwrap_or_default()
        ));
    }
    if let Some(state_db) = environment.state_db.as_deref() {
        for thread in &threads {
            let indexed = sqlite_adapter::thread_rollout_path(state_db, &thread.record.id)?;
            if indexed
                .as_deref()
                .is_none_or(|path| Path::new(path) != thread.source_path)
            {
                issues.push(format!(
                    "thread {} rollout path is missing or stale",
                    thread.record.id
                ));
            }
        }
    }
    let index = session_index::load(&environment.codex_home)?;
    for thread in &threads {
        if !index.contains_key(&thread.record.id) {
            issues.push(format!(
                "thread {} is missing from session_index.jsonl",
                thread.record.id
            ));
        }
    }
    Ok(DiagnosticReport {
        codex_home: environment.codex_home.to_string_lossy().into_owned(),
        sqlite_home: environment.sqlite_home.to_string_lossy().into_owned(),
        codex_executable: environment
            .codex_executable
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        codex_version: environment.codex_version.clone(),
        platform: environment.platform.clone(),
        wsl: environment.wsl,
        state_db: environment
            .state_db
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        schema_version: environment.schema_version,
        active_rollouts,
        archived_rollouts,
        database_threads,
        integrity,
        issues,
    })
}

pub fn run_codex_doctor(environment: &Environment) -> Result<Option<serde_json::Value>> {
    let Some(codex) = environment.codex_executable.as_deref() else {
        return Ok(None);
    };
    let output = Command::new(codex)
        .args(["doctor", "--json"])
        .env("CODEX_HOME", &environment.codex_home)
        .output()
        .with_context(|| format!("run {} doctor", codex.display()))?;
    // doctor returns non-zero when unrelated checks such as network fail.
    let value = serde_json::from_slice(&output.stdout).ok();
    Ok(value)
}

pub fn verify_expected_threads(
    environment: &Environment,
    expected_ids: impl IntoIterator<Item = String>,
) -> Result<()> {
    let expected = expected_ids.into_iter().collect::<HashSet<_>>();
    let threads =
        scanner::scan_codex_home(&environment.codex_home, environment.state_db.as_deref())?;
    let actual = threads
        .into_iter()
        .map(|thread| thread.record.id)
        .collect::<HashSet<_>>();
    let missing = expected.difference(&actual).cloned().collect::<Vec<_>>();
    if !missing.is_empty() {
        anyhow::bail!("imported rollout files are missing: {}", missing.join(", "));
    }
    let report = diagnose(environment)?;
    let relevant = report
        .issues
        .into_iter()
        .filter(|issue| expected.iter().any(|id| issue.contains(id)))
        .collect::<Vec<_>>();
    if !relevant.is_empty() {
        anyhow::bail!("import validation failed: {}", relevant.join("; "));
    }
    Ok(())
}
