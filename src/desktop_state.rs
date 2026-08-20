use crate::path_mapper::normalize;
use anyhow::{Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

const LOCAL_PROJECT_MIGRATIONS: [&str; 2] = [
    "2026-07-13-local-projects",
    "2026-07-14-repair-metadata-local-projects",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedThreadProject {
    pub thread_id: String,
    pub source_cwd: String,
    pub target_cwd: String,
}

#[derive(Debug)]
struct ProjectRegistration {
    source_cwd: String,
    target_cwd: String,
    thread_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct RegisteredProject {
    id: String,
    name: String,
    target_cwd: String,
    thread_ids: BTreeSet<String>,
}

pub fn state_path(codex_home: &Path) -> PathBuf {
    codex_home.join(".codex-global-state.json")
}

pub fn local_project_id(root_path: &str) -> String {
    let digest = hex::encode(Sha256::digest(root_path.as_bytes()));
    format!("local-{}", &digest[..32])
}

pub fn register_projects(
    target_path: &Path,
    source_path: &Path,
    threads: &[ImportedThreadProject],
) -> Result<()> {
    if threads.is_empty() {
        return Ok(());
    }
    let source = read_object(source_path)?;
    let mut target = read_object(target_path)?;
    let projects = group_projects(threads);
    let registered = register_modern_projects(&source, &mut target, projects);

    register_project_state(&mut target, &registered);
    register_legacy_arrays(
        object_entry(&mut target, "electron-persisted-atom-state"),
        &registered,
    );
    reset_local_project_migrations(&mut target);

    if let Some(parent) = target_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = target_path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(&Value::Object(target))?)?;
    fs::rename(&temporary, target_path)?;
    Ok(())
}

fn read_object(path: &Path) -> Result<Map<String, Value>> {
    if !path.is_file() {
        return Ok(Map::new());
    }
    let value = serde_json::from_slice::<Value>(&fs::read(path)?)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(value.as_object().cloned().unwrap_or_default())
}

fn group_projects(threads: &[ImportedThreadProject]) -> Vec<ProjectRegistration> {
    let mut projects = BTreeMap::<String, ProjectRegistration>::new();
    for thread in threads {
        let key = normalize(&thread.target_cwd);
        let project = projects.entry(key).or_insert_with(|| ProjectRegistration {
            source_cwd: thread.source_cwd.clone(),
            target_cwd: thread.target_cwd.clone(),
            thread_ids: BTreeSet::new(),
        });
        project.thread_ids.insert(thread.thread_id.clone());
    }
    projects.into_values().collect()
}

fn register_modern_projects(
    source: &Map<String, Value>,
    target: &mut Map<String, Value>,
    projects: Vec<ProjectRegistration>,
) -> Vec<RegisteredProject> {
    let now = chrono::Utc::now().timestamp_millis();
    let local_projects = object_entry(target, "local-projects");
    let mut registered = Vec::new();
    for project in projects {
        let id = find_existing_project_id(local_projects, &project.target_cwd)
            .unwrap_or_else(|| local_project_id(&project.target_cwd));
        let existing = local_projects
            .get(&id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let name = existing
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty())
            .map(str::to_owned)
            .or_else(|| source_project_name(source, &project))
            .unwrap_or_else(|| project_name(&project.target_cwd));
        let created_at = existing
            .get("createdAt")
            .and_then(Value::as_i64)
            .or_else(|| source_project_timestamp(source, &project, "createdAt"))
            .unwrap_or(now);
        let updated_at = existing
            .get("updatedAt")
            .and_then(Value::as_i64)
            .or_else(|| source_project_timestamp(source, &project, "updatedAt"))
            .unwrap_or(now);
        let mut value = existing;
        value.insert("id".to_owned(), Value::String(id.clone()));
        value.insert("name".to_owned(), Value::String(name.clone()));
        value.insert(
            "rootPaths".to_owned(),
            Value::Array(vec![Value::String(project.target_cwd.clone())]),
        );
        value.insert("createdAt".to_owned(), Value::Number(created_at.into()));
        value.insert("updatedAt".to_owned(), Value::Number(updated_at.into()));
        local_projects.insert(id.clone(), Value::Object(value));
        registered.push(RegisteredProject {
            id,
            name,
            target_cwd: project.target_cwd,
            thread_ids: project.thread_ids,
        });
    }
    registered
}

fn source_project_name(
    source: &Map<String, Value>,
    project: &ProjectRegistration,
) -> Option<String> {
    source_project(source, project)
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            source
                .get("electron-workspace-root-labels")
                .and_then(Value::as_object)
                .and_then(|labels| {
                    labels.iter().find_map(|(root, label)| {
                        paths_equal(root, &project.source_cwd)
                            .then(|| label.as_str())
                            .flatten()
                            .filter(|label| !label.trim().is_empty())
                            .map(str::to_owned)
                    })
                })
        })
}

fn source_project_timestamp(
    source: &Map<String, Value>,
    project: &ProjectRegistration,
    key: &str,
) -> Option<i64> {
    source_project(source, project)
        .and_then(|value| value.get(key))
        .and_then(Value::as_i64)
}

fn source_project<'a>(
    source: &'a Map<String, Value>,
    project: &ProjectRegistration,
) -> Option<&'a Map<String, Value>> {
    let projects = source.get("local-projects")?.as_object()?;
    let assigned_id = source
        .get("thread-project-assignments")
        .and_then(Value::as_object)
        .and_then(|assignments| {
            project.thread_ids.iter().find_map(|thread_id| {
                assignments
                    .get(thread_id)
                    .and_then(Value::as_object)
                    .and_then(|assignment| assignment.get("projectId"))
                    .and_then(Value::as_str)
            })
        });
    assigned_id
        .and_then(|id| projects.get(id))
        .and_then(Value::as_object)
        .or_else(|| {
            projects.values().find_map(|value| {
                let object = value.as_object()?;
                object
                    .get("rootPaths")?
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|root| paths_equal(root, &project.source_cwd))
                    .then_some(object)
            })
        })
}

fn find_existing_project_id(projects: &Map<String, Value>, target_cwd: &str) -> Option<String> {
    projects.iter().find_map(|(id, value)| {
        let roots = value.get("rootPaths")?.as_array()?;
        (roots.len() == 1
            && roots
                .first()
                .and_then(Value::as_str)
                .is_some_and(|root| paths_equal(root, target_cwd)))
        .then(|| id.clone())
    })
}

fn register_project_state(target: &mut Map<String, Value>, projects: &[RegisteredProject]) {
    register_legacy_arrays(target, projects);
    let labels = object_entry(target, "electron-workspace-root-labels");
    for project in projects {
        labels.insert(
            project.target_cwd.clone(),
            Value::String(project.name.clone()),
        );
    }
    let assignments = object_entry(target, "thread-project-assignments");
    for project in projects {
        for thread_id in &project.thread_ids {
            assignments.insert(
                thread_id.clone(),
                json!({
                    "projectKind": "local",
                    "projectId": project.id,
                    "cwd": project.target_cwd,
                    "pendingCoreUpdate": false
                }),
            );
        }
    }
    let assigned = projects
        .iter()
        .flat_map(|project| project.thread_ids.iter().cloned())
        .collect::<BTreeSet<_>>();
    if let Some(projectless) = target
        .get_mut("projectless-thread-ids")
        .and_then(Value::as_array_mut)
    {
        projectless.retain(|value| {
            value
                .as_str()
                .is_none_or(|thread_id| !assigned.contains(thread_id))
        });
    }
}

fn register_legacy_arrays(object: &mut Map<String, Value>, projects: &[RegisteredProject]) {
    let saved_roots = array_entry(object, "electron-saved-workspace-roots");
    for project in projects {
        if !saved_roots.iter().any(|value| {
            value
                .as_str()
                .is_some_and(|root| paths_equal(root, &project.target_cwd))
        }) {
            saved_roots.push(Value::String(project.target_cwd.clone()));
        }
    }
    let order = array_entry(object, "project-order");
    for project in projects {
        let mut found = false;
        order.retain_mut(|value| {
            let matches = value.as_str().is_some_and(|candidate| {
                candidate == project.id || paths_equal(candidate, &project.target_cwd)
            });
            if matches {
                if found {
                    return false;
                }
                *value = Value::String(project.id.clone());
                found = true;
            }
            true
        });
        if !found {
            order.push(Value::String(project.id.clone()));
        }
    }
}

fn reset_local_project_migrations(target: &mut Map<String, Value>) {
    if let Some(completed) = target
        .get_mut("electron-completed-local-data-migration-ids")
        .and_then(Value::as_array_mut)
    {
        completed.retain(|value| {
            value
                .as_str()
                .is_none_or(|id| !LOCAL_PROJECT_MIGRATIONS.contains(&id))
        });
    }
}

fn paths_equal(left: &str, right: &str) -> bool {
    let left = normalize(left);
    let right = normalize(right);
    let windows_path =
        |value: &str| value.as_bytes().get(1) == Some(&b':') || value.starts_with("//");
    if windows_path(&left) || windows_path(&right) {
        left.eq_ignore_ascii_case(&right)
    } else {
        left == right
    }
}

fn project_name(path: &str) -> String {
    normalize(path)
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("Project")
        .to_owned()
}

fn object_entry<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value
        .as_object_mut()
        .expect("value was converted to object")
}

fn array_entry<'a>(object: &'a mut Map<String, Value>, key: &str) -> &'a mut Vec<Value> {
    let value = object
        .entry(key.to_owned())
        .or_insert_with(|| Value::Array(Vec::new()));
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value.as_array_mut().expect("value was converted to array")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registers_modern_project_and_assignment_without_removing_existing_state() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.json");
        let target = directory.path().join("target.json");
        fs::write(
            &source,
            serde_json::to_vec(&json!({
                "local-projects": {"old": {
                    "id": "old", "name": "Original name",
                    "rootPaths": ["D:/old/demo"], "createdAt": 10, "updatedAt": 20
                }},
                "thread-project-assignments": {"thread-1": {"projectId": "old"}}
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            &target,
            serde_json::to_vec(&json!({
                "local-projects": {"existing": {
                    "id": "existing", "name": "Existing",
                    "rootPaths": ["C:/existing"], "createdAt": 1, "updatedAt": 2
                }},
                "project-order": ["existing", "E:\\new\\demo"],
                "projectless-thread-ids": ["thread-1", "other"],
                "electron-completed-local-data-migration-ids": [
                    "unrelated", "2026-07-13-local-projects",
                    "2026-07-14-repair-metadata-local-projects"
                ],
                "electron-persisted-atom-state": {"other": true}
            }))
            .unwrap(),
        )
        .unwrap();
        let thread = ImportedThreadProject {
            thread_id: "thread-1".to_owned(),
            source_cwd: "D:/old/demo".to_owned(),
            target_cwd: "E:/new/demo".to_owned(),
        };

        register_projects(&target, &source, &[thread]).unwrap();

        let value: Value = serde_json::from_slice(&fs::read(target).unwrap()).unwrap();
        let id = local_project_id("E:/new/demo");
        assert_eq!(value["local-projects"][&id]["name"], "Original name");
        assert_eq!(value["local-projects"][&id]["rootPaths"][0], "E:/new/demo");
        assert_eq!(
            value["thread-project-assignments"]["thread-1"]["projectId"],
            id
        );
        assert_eq!(value["project-order"], json!(["existing", id]));
        assert_eq!(value["projectless-thread-ids"], json!(["other"]));
        assert_eq!(
            value["electron-completed-local-data-migration-ids"],
            json!(["unrelated"])
        );
        assert_eq!(value["electron-persisted-atom-state"]["other"], true);
    }

    #[test]
    fn reuses_existing_project_with_equivalent_windows_path() {
        let projects = serde_json::from_value::<Map<String, Value>>(json!({
            "existing-id": {"rootPaths": ["E:\\new\\demo"]}
        }))
        .unwrap();
        assert_eq!(
            find_existing_project_id(&projects, "e:/new/demo"),
            Some("existing-id".to_owned())
        );
    }

    #[test]
    fn matches_codex_desktop_path_hash_ids() {
        assert_eq!(
            local_project_id(r"E:\Projects\demo"),
            "local-521c61092fcb4b7b9f95b32470fce007"
        );
    }
}
