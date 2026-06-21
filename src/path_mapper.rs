use crate::model::PlatformKind;
use anyhow::{anyhow, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMapping {
    pub source: String,
    pub target: String,
}

pub fn parse_mapping(value: &str) -> Result<PathMapping> {
    let (source, target) = value
        .split_once('=')
        .ok_or_else(|| anyhow!("path mapping must use OLD=NEW: {value}"))?;
    if source.trim().is_empty() || target.trim().is_empty() {
        return Err(anyhow!(
            "path mapping cannot contain an empty side: {value}"
        ));
    }
    Ok(PathMapping {
        source: normalize(source),
        target: normalize(target),
    })
}

pub fn map_path(
    original: &str,
    mappings: &[PathMapping],
    platform: &PlatformKind,
    history_root: &Path,
    thread_id: &str,
) -> PathBuf {
    let normalized = normalize(original);
    let case_insensitive = matches!(platform, PlatformKind::Windows);
    let mut candidates = mappings
        .iter()
        .filter(|mapping| prefix_matches(&normalized, &mapping.source, case_insensitive))
        .collect::<Vec<_>>();
    candidates.sort_by_key(|mapping| std::cmp::Reverse(mapping.source.len()));

    if let Some(mapping) = candidates.first() {
        let suffix = &normalized[mapping.source.len()..];
        let mapped = format!("{}{}", mapping.target.trim_end_matches('/'), suffix);
        return platform_path(&mapped, platform);
    }

    if path_is_usable(&normalized, platform) {
        return platform_path(&normalized, platform);
    }
    history_root.join(thread_id)
}

pub fn normalize(value: &str) -> String {
    let mut result = value.trim().replace('\\', "/");
    while result.contains("//") && !result.starts_with("//") {
        result = result.replace("//", "/");
    }
    if result.len() > 1 {
        result = result.trim_end_matches('/').to_owned();
    }
    result
}

pub fn history_only_path(root: &Path, original: &str) -> PathBuf {
    let normalized = normalize(original);
    let digest = hex::encode(Sha256::digest(normalized.as_bytes()));
    let name = normalized
        .rsplit('/')
        .find(|part| !part.is_empty())
        .unwrap_or("project");
    let safe_name = name
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    root.join(format!("{}-{}", &digest[..12], safe_name))
}

pub fn map_explicit(
    original: &str,
    mappings: &BTreeMap<String, String>,
    platform: &PlatformKind,
) -> Option<PathBuf> {
    let normalized = normalize(original);
    let case_insensitive = matches!(platform, PlatformKind::Windows);
    mappings
        .iter()
        .filter(|(source, _)| prefix_matches(&normalized, source, case_insensitive))
        .max_by_key(|(source, _)| source.len())
        .map(|(source, target)| {
            let suffix = &normalized[source.len()..];
            platform_path(
                &format!("{}{}", target.trim_end_matches('/'), suffix),
                platform,
            )
        })
}

pub fn prefix_matches(path: &str, prefix: &str, insensitive: bool) -> bool {
    let (path, prefix) = if insensitive {
        (path.to_ascii_lowercase(), prefix.to_ascii_lowercase())
    } else {
        (path.to_owned(), prefix.to_owned())
    };
    path == prefix
        || path
            .strip_prefix(&prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn path_is_usable(path: &str, platform: &PlatformKind) -> bool {
    match platform {
        PlatformKind::Windows => {
            (path.len() >= 3 && path.as_bytes()[1] == b':' && path.as_bytes()[2] == b'/')
                || path.starts_with("//")
        }
        PlatformKind::Wsl => path.starts_with('/') || is_windows_drive(path),
        _ => path.starts_with('/'),
    }
}

fn platform_path(path: &str, platform: &PlatformKind) -> PathBuf {
    if matches!(platform, PlatformKind::Wsl) && is_windows_drive(path) {
        let drive = path[0..1].to_ascii_lowercase();
        let suffix = path[2..].trim_start_matches('/');
        return PathBuf::from(format!("/mnt/{drive}/{suffix}"));
    }
    PathBuf::from(path)
}

fn is_windows_drive(path: &str) -> bool {
    path.len() >= 3
        && path.as_bytes()[0].is_ascii_alphabetic()
        && path.as_bytes()[1] == b':'
        && path.as_bytes()[2] == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longest_prefix_wins() {
        let mappings = vec![
            parse_mapping("/Users/a=D:/Users").unwrap(),
            parse_mapping("/Users/a/Projects=D:/Projects").unwrap(),
        ];
        let mapped = map_path(
            "/Users/a/Projects/demo",
            &mappings,
            &PlatformKind::Windows,
            Path::new("history"),
            "id",
        );
        assert_eq!(normalize(&mapped.to_string_lossy()), "D:/Projects/demo");
    }

    #[test]
    fn converts_windows_drive_to_wsl() {
        let mappings = vec![parse_mapping("/Users/a=D:/Work").unwrap()];
        let mapped = map_path(
            "/Users/a/demo",
            &mappings,
            &PlatformKind::Wsl,
            Path::new("/tmp/history"),
            "id",
        );
        assert_eq!(mapped, PathBuf::from("/mnt/d/Work/demo"));
    }

    #[test]
    fn unresolved_path_uses_history_root() {
        let mapped = map_path(
            "C:/work/demo",
            &[],
            &PlatformKind::Linux,
            Path::new("/tmp/history"),
            "abc",
        );
        assert_eq!(mapped, PathBuf::from("/tmp/history/abc"));
    }

    #[test]
    fn windows_mapping_is_case_insensitive() {
        let mappings = vec![parse_mapping("C:/Users/A/Work=D:/Projects").unwrap()];
        let mapped = map_path(
            "c:/users/a/work/demo",
            &mappings,
            &PlatformKind::Windows,
            Path::new("history"),
            "id",
        );
        assert_eq!(normalize(&mapped.to_string_lossy()), "D:/Projects/demo");
    }

    #[test]
    fn preserves_unc_paths_on_windows() {
        let mapped = map_path(
            r"\\server\share\demo",
            &[],
            &PlatformKind::Windows,
            Path::new("history"),
            "id",
        );
        assert_eq!(normalize(&mapped.to_string_lossy()), "//server/share/demo");
    }

    #[test]
    fn history_path_is_stable_per_project() {
        let first = history_only_path(Path::new("/history"), "/Users/a/Project A");
        let second = history_only_path(Path::new("/history"), "/Users/a/Project A");
        assert_eq!(first, second);
        assert!(first.to_string_lossy().contains("Project_A"));
    }
}
