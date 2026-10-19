//! JabRef-compatible linked files (`file = {desc:path:type;...}`).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedFile {
    pub description: String,
    pub path: String,
    pub file_type: String,
}

fn split_escaped(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&n) = chars.peek() {
                if n == sep || n == ':' || n == ';' || n == '\\' {
                    cur.push(n);
                    chars.next();
                    continue;
                }
            }
            cur.push(c);
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

pub fn parse_file_field(s: &str) -> Vec<LinkedFile> {
    let mut out = Vec::new();
    for item in split_escaped_keep(s, ';') {
        if item.trim().is_empty() {
            continue;
        }
        let parts = split_escaped(&item, ':');
        let lf = match parts.len() {
            1 => LinkedFile { description: String::new(), path: parts[0].clone(), file_type: String::new() },
            2 => LinkedFile { description: parts[0].clone(), path: parts[1].clone(), file_type: String::new() },
            _ => {
                // Windows drive letters ("C:\...") produce an extra split; rejoin middle parts
                let desc = parts[0].clone();
                let ty = parts[parts.len() - 1].clone();
                let path = parts[1..parts.len() - 1].join(":");
                LinkedFile { description: desc, path, file_type: ty }
            }
        };
        out.push(lf);
    }
    out
}

/// Split on `sep` but keep escapes intact for the second-level split.
fn split_escaped_keep(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            cur.push(c);
            if let Some(n) = chars.next() {
                cur.push(n);
            }
        } else if c == sep {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace(':', "\\:").replace(';', "\\;")
}

pub fn serialize_file_field(files: &[LinkedFile]) -> String {
    files
        .iter()
        .map(|f| format!("{}:{}:{}", escape(&f.description), escape(&f.path), escape(&f.file_type)))
        .collect::<Vec<_>>()
        .join(";")
}

/// Directories used to resolve relative file links, in priority order.
#[derive(Debug, Clone, Default)]
pub struct FileDirs {
    pub dirs: Vec<PathBuf>,
}

impl FileDirs {
    /// Build from the library's JabRef metadata, the user's setting, JabRef's
    /// own preferences, and the .bib file's folder.
    pub fn discover(meta: &std::collections::HashMap<String, String>, bib_path: Option<&Path>, setting: Option<&str>) -> FileDirs {
        let mut dirs = Vec::new();
        let bib_dir = bib_path.and_then(|p| p.parent()).map(|p| p.to_path_buf());
        let resolve = |d: &str| -> PathBuf {
            let p = PathBuf::from(d);
            if p.is_absolute() {
                p
            } else if let Some(b) = &bib_dir {
                b.join(p)
            } else {
                p
            }
        };
        // user-specific JabRef entries first: fileDirectory-<user>-<host>
        let user = std::env::var("USER").unwrap_or_default();
        for (k, v) in meta {
            if k.starts_with("fileDirectory-") && k.contains(&user) && !v.is_empty() {
                dirs.push(resolve(v));
            }
        }
        if let Some(v) = meta.get("fileDirectory") {
            if !v.is_empty() {
                dirs.push(resolve(v));
            }
        }
        if let Some(s) = setting {
            if !s.trim().is_empty() {
                dirs.push(PathBuf::from(s.trim()));
            }
        }
        if let Some(d) = jabref_main_file_directory() {
            dirs.push(d);
        }
        if let Some(b) = bib_dir {
            dirs.push(b);
        }
        dirs.dedup();
        FileDirs { dirs }
    }

    pub fn resolve(&self, path: &str) -> Option<PathBuf> {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            return p.exists().then_some(p);
        }
        for d in &self.dirs {
            let c = d.join(&p);
            if c.exists() {
                return Some(c);
            }
        }
        // links like "home/dom/..." that lost their leading slash
        let abs = PathBuf::from(format!("/{path}"));
        if abs.exists() {
            return Some(abs);
        }
        None
    }

    /// Path to store for a newly attached file: relative to the main
    /// directory when inside it, absolute otherwise.
    pub fn relativize(&self, path: &Path) -> String {
        for d in &self.dirs {
            if let Ok(rel) = path.strip_prefix(d) {
                return rel.to_string_lossy().to_string();
            }
        }
        path.to_string_lossy().to_string()
    }
}

/// JabRef stores its preferences with Java's Preferences API, which on Linux
/// writes ~/.java/.userPrefs/org/jabref/prefs.xml.
pub fn jabref_main_file_directory() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    // regular install, Flatpak, Snap
    for rel in [
        ".java/.userPrefs/org/jabref/prefs.xml",
        ".var/app/org.jabref.jabref/.java/.userPrefs/org/jabref/prefs.xml",
        "snap/jabref/current/.java/.userPrefs/org/jabref/prefs.xml",
    ] {
        let Ok(xml) = std::fs::read_to_string(home.join(rel)) else { continue };
        for key in ["fileDirectory"] {
            let pat = format!("key=\"{key}\" value=\"");
            if let Some(i) = xml.find(&pat) {
                let rest = &xml[i + pat.len()..];
                if let Some(j) = rest.find('"') {
                    let v = rest[..j].replace("&amp;", "&").replace("&quot;", "\"").replace("&apos;", "'");
                    if !v.is_empty() {
                        return Some(PathBuf::from(v));
                    }
                }
            }
        }
    }
    None
}

/// Default eprint type for this library (archive.org ARK identifiers).
pub const DEFAULT_EPRINTTYPE: &str = "ark";

/// "https://n2t.net/ark:/13960/t0wq6n34h" → "ark:/13960/t0wq6n34h".
pub fn normalize_eprint(eprint: &str, eprinttype: &str) -> String {
    let e = eprint.trim();
    if (eprinttype.is_empty() || eprinttype.eq_ignore_ascii_case("ark")) && !e.starts_with("ark:") {
        if let Some(i) = e.find("ark:/") {
            return e[i..].trim_end_matches('/').to_string();
        }
    }
    e.to_string()
}

/// Web address for an eprint, by type.
pub fn eprint_url(eprint: &str, eprinttype: &str) -> Option<String> {
    let e = eprint.trim();
    if e.is_empty() {
        return None;
    }
    if e.starts_with("http://") || e.starts_with("https://") {
        return Some(e.to_string());
    }
    let t = if eprinttype.trim().is_empty() { DEFAULT_EPRINTTYPE } else { eprinttype.trim() };
    Some(match t.to_lowercase().as_str() {
        "ark" => format!("https://n2t.net/{e}"),
        "archive.org" => format!("https://archive.org/details/{e}"),
        "jstor" => format!("https://www.jstor.org/stable/{e}"),
        "hdl" | "handle" => format!("https://hdl.handle.net/{e}"),
        "arxiv" => format!("https://arxiv.org/abs/{e}"),
        "googlebooks" => format!("https://books.google.com/books?id={e}"),
        "doi" => format!("https://doi.org/{e}"),
        "pubmed" => format!("https://pubmed.ncbi.nlm.nih.gov/{e}"),
        _ => return None,
    })
}

pub fn file_type_for(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_lowercase()).as_deref() {
        Some("pdf") => "PDF".into(),
        Some("djvu") => "Djvu".into(),
        Some("epub") => "ePUB".into(),
        Some("html") | Some("htm") => "HTML".into(),
        Some("doc") | Some("docx") => "Word".into(),
        Some("odt") => "OpenDocument text".into(),
        Some("txt") => "Text".into(),
        Some(e) => e.to_uppercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eprints() {
        assert_eq!(normalize_eprint("https://n2t.net/ark:/13960/t0wq6n34h", ""), "ark:/13960/t0wq6n34h");
        assert_eq!(normalize_eprint("ark:/13960/t0wq6n34h", "ark"), "ark:/13960/t0wq6n34h");
        assert_eq!(normalize_eprint("2101.00001", "arXiv"), "2101.00001");
        assert_eq!(eprint_url("ark:/13960/t0wq6n34h", "ark").unwrap(), "https://n2t.net/ark:/13960/t0wq6n34h");
        assert_eq!(eprint_url("602910", "jstor").unwrap(), "https://www.jstor.org/stable/602910");
        assert_eq!(eprint_url("ark:/13960/x", "").unwrap(), "https://n2t.net/ark:/13960/x");
    }

    #[test]
    fn parse_and_serialize() {
        let f = parse_file_field(":Lubotsky, Alexander/2023 - Fire and Water_ the Bronze Age.pdf:PDF");
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].path, "Lubotsky, Alexander/2023 - Fire and Water_ the Bronze Age.pdf");
        assert_eq!(f[0].file_type, "PDF");
        let f = parse_file_field("Main:a\\:b.pdf:PDF;:c.djvu:Djvu");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].path, "a:b.pdf");
        assert_eq!(serialize_file_field(&f), "Main:a\\:b.pdf:PDF;:c.djvu:Djvu");
        let f = parse_file_field(":C\\:\\\\Users\\\\x.pdf:PDF");
        assert_eq!(f[0].path, "C:\\Users\\x.pdf");
        let f = parse_file_field(":C:\\Users\\x.pdf:PDF");
        assert_eq!(f[0].path, "C:\\Users\\x.pdf");
    }
}
