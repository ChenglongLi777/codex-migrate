use anyhow::{Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;

pub fn rewrite_cwd_bytes(content: &[u8], cwd: &Path) -> Result<(Vec<u8>, usize)> {
    let cwd = cwd.to_string_lossy();
    let mut output = Vec::with_capacity(content.len());
    let mut changed = 0;
    for (line_number, line) in content.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let mut value: Value = serde_json::from_slice(line)
            .with_context(|| format!("parse rollout line {}", line_number + 1))?;
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta" | "turn_context")
        ) {
            if let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) {
                if payload.get("cwd").and_then(Value::as_str) != Some(cwd.as_ref()) {
                    payload.insert("cwd".to_owned(), Value::String(cwd.to_string()));
                    changed += 1;
                }
            }
        }
        output.extend_from_slice(&serde_json::to_vec(&value)?);
        output.push(b'\n');
    }
    Ok((output, changed))
}

pub fn rewrite_cwd_file(path: &Path, cwd: &Path) -> Result<usize> {
    let original = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let (rewritten, changed) = rewrite_cwd_bytes(&original, cwd)?;
    if changed > 0 {
        let temporary = path.with_extension("jsonl.tmp");
        fs::write(&temporary, rewritten)?;
        fs::rename(&temporary, path)?;
    }
    Ok(changed)
}

pub fn validate_cwd_file(path: &Path, cwd: &Path) -> Result<()> {
    let content = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let expected = cwd.to_string_lossy();
    let mut session_meta = 0;
    for (line_number, line) in content.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(line)
            .with_context(|| format!("parse rollout line {}", line_number + 1))?;
        if !matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta" | "turn_context")
        ) {
            continue;
        }
        let Some(actual) = value
            .get("payload")
            .and_then(|payload| payload.get("cwd"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if actual != expected {
            anyhow::bail!(
                "{} contains cwd {actual}, expected {}",
                path.display(),
                cwd.display()
            );
        }
        if value.get("type").and_then(Value::as_str) == Some("session_meta") {
            session_meta += 1;
        }
    }
    if session_meta == 0 {
        anyhow::bail!("{} has no mapped session_meta cwd", path.display());
    }
    Ok(())
}

pub fn canonicalize_cwd(content: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(content.len());
    for line in content.split(|byte| *byte == b'\n') {
        if line.is_empty() {
            continue;
        }
        let mut value: Value = serde_json::from_slice(line).ok()?;
        if matches!(
            value.get("type").and_then(Value::as_str),
            Some("session_meta" | "turn_context")
        ) {
            if let Some(payload) = value.get_mut("payload").and_then(Value::as_object_mut) {
                if payload.contains_key("cwd") {
                    payload.insert("cwd".to_owned(), Value::String("<mapped-cwd>".to_owned()));
                }
            }
        }
        output.extend_from_slice(&serde_json::to_vec(&value).ok()?);
        output.push(b'\n');
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_only_structured_cwd_fields() {
        let input = br#"{"type":"session_meta","payload":{"cwd":"/old","note":"/old"}}
{"type":"event_msg","payload":{"message":"/old"}}
"#;
        let (output, changed) = rewrite_cwd_bytes(input, Path::new("/new")).unwrap();
        let text = String::from_utf8(output).unwrap();
        assert_eq!(changed, 1);
        assert!(text.contains("\"cwd\":\"/new\""));
        assert!(text.contains("\"note\":\"/old\""));
        assert!(text.contains("\"message\":\"/old\""));
    }

    #[test]
    fn canonical_form_ignores_mapped_cwd() {
        let old = br#"{"type":"session_meta","payload":{"id":"1","cwd":"/old"}}"#;
        let new = br#"{"type":"session_meta","payload":{"id":"1","cwd":"C:\\new"}}"#;
        assert_eq!(canonicalize_cwd(old), canonicalize_cwd(new));
    }
}
