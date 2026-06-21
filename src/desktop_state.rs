use anyhow::{Context, Result};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

pub fn state_path(codex_home: &Path) -> PathBuf {
    codex_home.join(".codex-global-state.json")
}

pub fn register_projects(path: &Path, projects: &BTreeSet<String>) -> Result<()> {
    if projects.is_empty() {
        return Ok(());
    }
    let mut root = if path.is_file() {
        serde_json::from_slice::<Value>(&fs::read(path)?)
            .with_context(|| format!("parse {}", path.display()))?
    } else {
        Value::Object(Map::new())
    };
    if !root.is_object() {
        root = Value::Object(Map::new());
    }
    let root_object = root.as_object_mut().expect("root was converted to object");
    register_project_arrays(root_object, projects);

    let state = root_object
        .entry("electron-persisted-atom-state")
        .or_insert_with(|| Value::Object(Map::new()));
    if !state.is_object() {
        *state = Value::Object(Map::new());
    }
    let state_object = state
        .as_object_mut()
        .expect("state was converted to object");
    register_project_arrays(state_object, projects);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(&root)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

fn register_project_arrays(object: &mut Map<String, Value>, projects: &BTreeSet<String>) {
    for key in ["electron-saved-workspace-roots", "project-order"] {
        let values = object
            .entry(key)
            .or_insert_with(|| Value::Array(Vec::new()));
        if !values.is_array() {
            *values = Value::Array(Vec::new());
        }
        let array = values.as_array_mut().expect("value was converted to array");
        for project in projects {
            if !array.iter().any(|value| value.as_str() == Some(project)) {
                array.push(Value::String(project.clone()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_projects_without_removing_existing_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(".codex-global-state.json");
        fs::write(
            &path,
            br#"{"electron-persisted-atom-state":{"other":true,"project-order":["/existing"]}}"#,
        )
        .unwrap();
        register_projects(
            &path,
            &["/new".to_owned(), "/existing".to_owned()]
                .into_iter()
                .collect(),
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(value["project-order"].as_array().unwrap().len(), 2);
        assert_eq!(
            value["electron-saved-workspace-roots"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        let state = &value["electron-persisted-atom-state"];
        assert_eq!(state["other"], true);
        assert_eq!(state["project-order"].as_array().unwrap().len(), 2);
        assert_eq!(
            state["electron-saved-workspace-roots"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }
}
