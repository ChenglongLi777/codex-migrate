use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub fn register_thread(
    codex: &Path,
    codex_home: &Path,
    rollout_path: &Path,
    thread_id: &str,
    cwd: &Path,
) -> Result<()> {
    let mut child = Command::new(codex)
        .args(["app-server", "--listen", "stdio://"])
        .env("CODEX_HOME", codex_home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("start Codex App Server using {}", codex.display()))?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("missing stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("missing stdout"))?;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(value) = serde_json::from_str::<Value>(&line) {
                let _ = sender.send(value);
            }
        }
    });

    let result = (|| {
        send(
            &mut stdin,
            json!({
                "method": "initialize",
                "id": 1,
                "params": {
                    "clientInfo": {
                        "name": "codex_migrate",
                        "title": "Codex Migrate",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {"experimentalApi": true}
                }
            }),
        )?;
        wait_for_result(&receiver, 1, Duration::from_secs(10))?;
        send(&mut stdin, json!({"method": "initialized", "params": {}}))?;
        send(
            &mut stdin,
            json!({
                "method": "thread/resume",
                "id": 2,
                "params": {
                    "threadId": thread_id,
                    "path": rollout_path.to_string_lossy(),
                    "cwd": cwd.to_string_lossy(),
                    "excludeTurns": true
                }
            }),
        )?;
        wait_for_result(&receiver, 2, Duration::from_secs(30))?;
        send(
            &mut stdin,
            json!({
                "method": "thread/read",
                "id": 3,
                "params": {"threadId": thread_id, "includeTurns": false}
            }),
        )?;
        wait_for_result(&receiver, 3, Duration::from_secs(10))?;
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn send(stdin: &mut impl Write, value: Value) -> Result<()> {
    serde_json::to_writer(&mut *stdin, &value)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn wait_for_result(receiver: &mpsc::Receiver<Value>, id: i64, timeout: Duration) -> Result<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("App Server request {id} timed out"));
        }
        let value = receiver
            .recv_timeout(remaining)
            .map_err(|_| anyhow!("App Server request {id} timed out"))?;
        if value.get("id").and_then(Value::as_i64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            return Err(anyhow!("App Server request {id} failed: {error}"));
        }
        return value
            .get("result")
            .cloned()
            .ok_or_else(|| anyhow!("App Server request {id} returned no result"));
    }
}
