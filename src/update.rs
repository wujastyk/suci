//! Checking GitHub for a newer release of Sūcī.

use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const RELEASES_API: &str = "https://api.github.com/repos/wujastyk/suci/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/wujastyk/suci/releases";

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Debug, Clone)]
pub struct Release {
    /// e.g. "0.6.0" (without the leading "v")
    pub version: String,
    pub page: String,
    /// release notes, lightly cleaned for plain-text display
    pub notes: String,
    /// "2026-10-21"
    pub date: String,
    /// the download for this computer, if the release has one
    pub asset: Option<Asset>,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// "v0.5.1" → [0, 5, 1]. Anything after a number ("1-beta") is ignored.
pub fn parse_version(s: &str) -> Option<Vec<u64>> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let mut out = Vec::new();
    for part in s.split('.') {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return None;
        }
        out.push(digits.parse().ok()?);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// Is `candidate` a later version than `current`?
pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

/// The file name ending of the download meant for this computer.
pub fn platform_suffix() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("_amd64.deb")
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        Some("-windows-x64.zip")
    } else {
        None
    }
}

fn clean_notes(body: &str) -> String {
    body.replace("\r\n", "\n")
        .lines()
        .map(|l| {
            let indent = l.len() - l.trim_start().len();
            let t = l.trim_start();
            let t = t.strip_prefix("* ").or_else(|| t.strip_prefix("- ")).map(|r| format!("• {r}")).unwrap_or_else(|| t.trim_start_matches('#').trim_start().to_string());
            format!("{}{}", " ".repeat(indent), t.replace("**", ""))
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn parse_release(json: &str, suffix: Option<&str>) -> Result<Release, String> {
    let r: GhRelease = serde_json::from_str(json).map_err(|e| format!("unexpected reply from GitHub: {e}"))?;
    if r.draft || r.prerelease {
        return Err("no finished release found".into());
    }
    let version = r.tag_name.trim_start_matches(['v', 'V']).to_string();
    let asset = suffix.and_then(|suf| r.assets.iter().find(|a| a.name.ends_with(suf))).map(|a| Asset {
        name: a.name.clone(),
        url: a.browser_download_url.clone(),
        size: a.size,
    });
    Ok(Release {
        version,
        page: r.html_url,
        notes: clean_notes(r.body.as_deref().unwrap_or("")),
        date: r.published_at.unwrap_or_default().chars().take(10).collect(),
        asset,
    })
}

/// Ask GitHub for the latest release. Blocking.
pub fn latest() -> Result<Release, String> {
    let json = crate::fetch::get(RELEASES_API, "application/vnd.github+json").map_err(|e| {
        if e == "not found" { "no release has been published yet".to_string() } else { e }
    })?;
    parse_release(&json, platform_suffix())
}

/// Download a release file into `dir` (created if missing). Blocking.
/// Returns the path of the saved file.
pub fn download(asset: &Asset, dir: &Path) -> Result<PathBuf, String> {
    let bytes = crate::fetch::get_bytes(&asset.url, 300 * 1024 * 1024)?;
    if asset.size > 0 && bytes.len() as u64 != asset.size {
        return Err(format!("download incomplete ({} of {} bytes)", bytes.len(), asset.size));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let name = Path::new(&asset.name).file_name().ok_or("bad file name")?;
    let path = dir.join(name);
    std::fs::write(&path, &bytes).map_err(|e| format!("could not save {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v0.5.1"), Some(vec![0, 5, 1]));
        assert!(is_newer("v0.6.0", "0.5.1"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.99.99"));
        assert!(!is_newer("v0.5.1", "0.5.1"));
        assert!(!is_newer("0.5", "0.5.0"));
        assert!(!is_newer("v0.5.0", "0.5.1"));
        assert!(!is_newer("nightly", "0.5.1"));
    }

    #[test]
    fn github_reply() {
        let json = r#"{"tag_name":"v0.6.0","html_url":"https://github.com/wujastyk/suci/releases/tag/v0.6.0",
            "published_at":"2026-10-25T15:00:00Z","draft":false,"prerelease":false,
            "body":"* First change\r\n  * detail\r\n* **Second**",
            "assets":[{"name":"suci-0.6.0-windows-x64.zip","browser_download_url":"https://x/w.zip","size":10},
                      {"name":"suci_0.6.0_amd64.deb","browser_download_url":"https://x/s.deb","size":20}]}"#;
        let r = parse_release(json, Some("_amd64.deb")).unwrap();
        assert_eq!(r.version, "0.6.0");
        assert_eq!(r.date, "2026-10-25");
        assert_eq!(r.asset.as_ref().unwrap().url, "https://x/s.deb");
        assert_eq!(r.notes, "• First change\n  • detail\n• Second");
        let r = parse_release(json, Some("-windows-x64.zip")).unwrap();
        assert_eq!(r.asset.unwrap().size, 10);
        assert!(parse_release(json, None).unwrap().asset.is_none());
    }
}
