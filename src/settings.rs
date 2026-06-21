use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LanguagePreference {
    System,
    Chinese,
    English,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub language: LanguagePreference,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            language: LanguagePreference::System,
        }
    }
}

impl AppSettings {
    pub fn load() -> Self {
        settings_path()
            .and_then(|path| fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path =
            settings_path().ok_or_else(|| anyhow::anyhow!("config directory unavailable"))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_vec_pretty(self)?)?;
        fs::rename(temporary, path)?;
        Ok(())
    }
}

pub fn system_is_chinese() -> bool {
    let environment_locale = std::env::var("LC_ALL")
        .ok()
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var("LC_MESSAGES").ok())
        .or_else(|| std::env::var("LANG").ok());
    let locale = environment_locale
        .filter(|value| {
            let normalized = value.to_ascii_lowercase();
            normalized != "c" && normalized != "posix" && !normalized.starts_with("c.")
        })
        .or_else(platform_locale)
        .unwrap_or_default()
        .to_ascii_lowercase();
    locale.starts_with("zh") || locale.contains("_cn") || locale.contains("-cn")
}

fn platform_locale() -> Option<String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleLocale"])
            .output()
            .ok()?;
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    #[cfg(target_os = "windows")]
    {
        let output = std::process::Command::new("powershell")
            .args(["-NoProfile", "-Command", "(Get-Culture).Name"])
            .output()
            .ok()?;
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    #[allow(unreachable_code)]
    None
}

fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("codex-migrate").join("settings.json"))
}
