use crate::model::ScannedThread;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HtmlExportSummary {
    pub exported: usize,
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct ChatEntry {
    role: String,
    text: Option<String>,
    images: Vec<String>,
    tool_output: bool,
}

pub fn export_threads(
    threads: &[ScannedThread],
    selected_ids: &BTreeSet<String>,
    output_root: Option<&Path>,
) -> Result<HtmlExportSummary> {
    let mut files = Vec::new();
    for thread in threads
        .iter()
        .filter(|thread| selected_ids.contains(&thread.record.id))
    {
        let project = Path::new(&thread.record.cwd);
        if !project.is_dir() {
            anyhow::bail!(
                "project folder does not exist for HTML export: {}",
                project.display()
            );
        }
        let output_dir = output_root
            .map(Path::to_path_buf)
            .unwrap_or_else(|| project.join("Codex_sessions"));
        fs::create_dir_all(&output_dir)?;
        let output = output_dir.join(format!(
            "{}-{}.html",
            safe_name(&thread.record.title),
            &thread.record.id[..thread.record.id.len().min(8)]
        ));
        fs::write(&output, render_thread(thread)?)?;
        files.push(output.to_string_lossy().into_owned());
    }
    Ok(HtmlExportSummary {
        exported: files.len(),
        files,
    })
}

fn render_thread(thread: &ScannedThread) -> Result<String> {
    let mut messages = Vec::<ChatEntry>::new();
    let mut seen = HashSet::<ChatEntry>::new();
    let mut seen_images = HashSet::<String>::new();
    for (line_number, line) in thread.content.split(|byte| *byte == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_slice(line).with_context(|| {
            format!(
                "parse line {} in {}",
                line_number + 1,
                thread.source_path.display()
            )
        })?;
        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }
        let payload = &value["payload"];
        match payload.get("type").and_then(Value::as_str) {
            Some("message") => {
                let Some(role) = payload.get("role").and_then(Value::as_str) else {
                    continue;
                };
                if !matches!(role, "user" | "assistant") {
                    continue;
                }
                let content = payload.get("content").and_then(Value::as_array);
                let text = content
                    .into_iter()
                    .flatten()
                    .filter_map(|item| {
                        item.get("text")
                            .or_else(|| item.get("input_text"))
                            .or_else(|| item.get("output_text"))
                            .and_then(Value::as_str)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let text = if role == "user" {
                    clean_user_message(&text)
                } else {
                    clean_text(&text)
                };
                let images = content
                    .into_iter()
                    .flatten()
                    .filter_map(|item| item.get("image_url").and_then(Value::as_str))
                    .filter(|url| is_embeddable_image(url))
                    .filter(|url| seen_images.insert((*url).to_owned()))
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
                if text.is_none() && images.is_empty() {
                    continue;
                }
                let entry = ChatEntry {
                    role: role.to_owned(),
                    text,
                    images,
                    tool_output: false,
                };
                if seen.insert(entry.clone()) {
                    messages.push(entry);
                }
            }
            Some("function_call_output") => {
                let mut images = Vec::new();
                collect_embedded_images(payload.get("output").unwrap_or(&Value::Null), &mut images);
                images.retain(|url| seen_images.insert(url.clone()));
                if !images.is_empty() {
                    messages.push(ChatEntry {
                        role: "assistant".to_owned(),
                        text: None,
                        images,
                        tool_output: true,
                    });
                }
            }
            _ => {}
        }
    }
    let mut body = String::new();
    for entry in messages {
        let label = if entry.tool_output {
            "Tool"
        } else if entry.role == "user" {
            "You"
        } else {
            "Codex"
        };
        let tool_class = if entry.tool_output { " tool" } else { "" };
        let text = entry
            .text
            .as_deref()
            .map(|text| {
                format!(
                    "<div class=\"content\">{}</div>",
                    html_escape(text).replace('\n', "<br>")
                )
            })
            .unwrap_or_default();
        let images = entry
            .images
            .iter()
            .map(|url| {
                format!(
                    "<img class=\"chat-image\" src=\"{}\" alt=\"Chat image\" loading=\"lazy\">",
                    html_escape(url)
                )
            })
            .collect::<String>();
        body.push_str(&format!(
            "<article class=\"message {role}{tool_class}\"><div class=\"role\">{label}</div>{text}<div class=\"images\">{images}</div></article>",
            role = entry.role,
        ));
    }
    Ok(format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{title}</title><style>
:root{{--bg:#f6f7f4;--surface:#fff;--text:#272d30;--muted:#687276;--accent:#1f6f68;--border:#dcDEda}}
*{{box-sizing:border-box}} body{{margin:0;background:var(--bg);color:var(--text);font:15px/1.65 -apple-system,BlinkMacSystemFont,"Segoe UI",sans-serif}}
main{{max-width:980px;margin:0 auto;padding:40px 24px 80px;display:flex;flex-direction:column;gap:14px}}
.message{{padding:14px 17px}} .message.assistant{{align-self:stretch;width:100%;background:transparent;padding-left:0;padding-right:0}}
.message.user{{max-width:72%;align-self:flex-end;background:#e5f1ee;border:1px solid #c9dfda;border-radius:14px;border-top-right-radius:5px;box-shadow:0 1px 2px rgba(20,30,30,.03)}}
.message.tool{{padding-top:4px;padding-bottom:4px}} .message.tool .role{{color:var(--muted)}}
.role{{font-weight:650;color:var(--accent);font-size:12px;margin-bottom:6px}} .message.user .role{{text-align:right}} .content{{white-space:normal;word-break:break-word}}
.images{{display:flex;flex-direction:column;gap:10px}} .content + .images:not(:empty){{margin-top:12px}}
.chat-image{{display:block;max-width:100%;height:auto;border:1px solid var(--border);border-radius:10px;background:var(--surface)}}
@media(max-width:640px){{main{{padding:20px 12px 48px}}.message.user{{max-width:90%}}}}
</style></head><body><main>{body}</main></body></html>"#,
        title = html_escape(if thread.record.title.is_empty() {
            &thread.record.id
        } else {
            &thread.record.title
        }),
    ))
}

fn is_embeddable_image(value: &str) -> bool {
    value.starts_with("data:image/") && value.contains(";base64,")
}

fn collect_embedded_images(value: &Value, images: &mut Vec<String>) {
    match value {
        Value::String(value) => {
            if is_embeddable_image(value) {
                images.push(value.clone());
            } else if let Ok(nested) = serde_json::from_str::<Value>(value) {
                collect_embedded_images(&nested, images);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_embedded_images(value, images);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_embedded_images(value, images);
            }
        }
        _ => {}
    }
}

fn clean_user_message(value: &str) -> Option<String> {
    for marker in ["## My request for Codex:", "## My request:"] {
        if let Some((_, request)) = value.rsplit_once(marker) {
            return clean_text(request);
        }
    }
    let trimmed = value.trim();
    if trimmed.starts_with("# AGENTS.md instructions")
        || trimmed.starts_with("<environment_context>")
        || trimmed.starts_with("<permissions instructions>")
    {
        return None;
    }
    clean_text(trimmed)
}

fn clean_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn safe_name(value: &str) -> String {
    let value = if value.trim().is_empty() {
        "Codex-session"
    } else {
        value
    };
    let cleaned = value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | ' ') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    cleaned.chars().take(80).collect()
}

fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ThreadRecord;
    use tempfile::TempDir;

    #[test]
    fn escapes_html() {
        assert_eq!(html_escape("<a&b>"), "&lt;a&amp;b&gt;");
    }

    #[test]
    fn exports_to_project_codex_sessions_folder() {
        let project = TempDir::new().unwrap();
        let id = "11111111-2222-3333-4444-555555555555";
        let content =
            br#"{"type":"event_msg","payload":{"type":"user_message","message":"hello <world>"}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"hello <world>"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}
"#
            .to_vec();
        let thread = ScannedThread {
            source_path: project.path().join("rollout.jsonl"),
            content,
            record: ThreadRecord {
                id: id.to_owned(),
                title: "Demo".to_owned(),
                created_at: 0,
                updated_at: 0,
                cwd: project.path().to_string_lossy().into_owned(),
                source: "test".to_owned(),
                thread_source: None,
                model_provider: "openai".to_owned(),
                cli_version: String::new(),
                archived: false,
                archive_path: String::new(),
                sha256: String::new(),
                byte_len: 0,
                first_user_message: "hello".to_owned(),
                sandbox_policy: None,
                approval_mode: None,
                model: None,
                reasoning_effort: None,
            },
        };
        let result =
            export_threads(&[thread], &[id.to_owned()].into_iter().collect(), None).unwrap();
        assert_eq!(result.exported, 1);
        let html = fs::read_to_string(&result.files[0]).unwrap();
        assert!(html.contains("hello &lt;world&gt;"));
        assert_eq!(html.matches("hello &lt;world&gt;").count(), 1);
        assert!(!html.contains("019e"));
        assert!(result.files[0].contains("Codex_sessions"));
    }

    #[test]
    fn embeds_user_images_and_tool_screenshots_in_one_html_file() {
        let project = TempDir::new().unwrap();
        let id = "12121212-3434-5656-7878-909090909090";
        let user_image = "data:image/png;base64,dXNlcg==";
        let tool_image = "data:image/jpeg;base64,dG9vbA==";
        let content = format!(
            r#"{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"show image"}},{{"type":"input_image","image_url":"{user_image}"}}]}}}}
{{"type":"response_item","payload":{{"type":"function_call_output","call_id":"call_1","output":[{{"type":"input_image","image_url":"{tool_image}"}}]}}}}
{{"type":"response_item","payload":{{"type":"function_call_output","call_id":"call_2","output":[{{"type":"input_image","image_url":"{tool_image}"}}]}}}}
"#
        )
        .into_bytes();
        let thread = ScannedThread {
            source_path: project.path().join("rollout.jsonl"),
            content,
            record: ThreadRecord {
                id: id.to_owned(),
                title: "Images".to_owned(),
                created_at: 0,
                updated_at: 0,
                cwd: project.path().to_string_lossy().into_owned(),
                source: "test".to_owned(),
                thread_source: None,
                model_provider: "openai".to_owned(),
                cli_version: String::new(),
                archived: false,
                archive_path: String::new(),
                sha256: String::new(),
                byte_len: 0,
                first_user_message: "show image".to_owned(),
                sandbox_policy: None,
                approval_mode: None,
                model: None,
                reasoning_effort: None,
            },
        };
        let result =
            export_threads(&[thread], &[id.to_owned()].into_iter().collect(), None).unwrap();
        let html = fs::read_to_string(&result.files[0]).unwrap();
        assert!(html.contains(user_image));
        assert!(html.contains(tool_image));
        assert_eq!(html.matches(tool_image).count(), 1);
        assert_eq!(result.files.len(), 1);
    }

    #[test]
    fn exports_to_user_selected_folder() {
        let project = TempDir::new().unwrap();
        let output = TempDir::new().unwrap();
        let id = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let thread = ScannedThread {
            source_path: project.path().join("rollout.jsonl"),
            content: br#"{"type":"event_msg","payload":{"type":"user_message","message":"hello"}}"#
                .to_vec(),
            record: ThreadRecord {
                id: id.to_owned(),
                title: "Selected folder".to_owned(),
                created_at: 0,
                updated_at: 0,
                cwd: project.path().to_string_lossy().into_owned(),
                source: "test".to_owned(),
                thread_source: None,
                model_provider: "openai".to_owned(),
                cli_version: String::new(),
                archived: false,
                archive_path: String::new(),
                sha256: String::new(),
                byte_len: 0,
                first_user_message: "hello".to_owned(),
                sandbox_policy: None,
                approval_mode: None,
                model: None,
                reasoning_effort: None,
            },
        };
        let result = export_threads(
            &[thread],
            &[id.to_owned()].into_iter().collect(),
            Some(output.path()),
        )
        .unwrap();
        assert!(Path::new(&result.files[0]).starts_with(output.path()));
    }

    #[test]
    fn filters_context_and_extracts_actual_request() {
        assert!(clean_user_message(
            "# AGENTS.md instructions for /tmp\n\n<INSTRUCTIONS>hidden</INSTRUCTIONS>"
        )
        .is_none());
        assert_eq!(
            clean_user_message(
                "# Applications mentioned by the user:\n<appshot />\n\n## My request for Codex:\n只保留这个请求"
            )
            .as_deref(),
            Some("只保留这个请求")
        );
    }
}
