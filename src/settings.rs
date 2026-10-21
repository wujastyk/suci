//! User settings, stored as JSON in ~/.config/suci/settings.json.

use crate::collate::SortMode;
use crate::keygen::KeyPattern;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub last_file: Option<PathBuf>,
    pub recent_files: Vec<PathBuf>,
    /// Folder for relative PDF links (in addition to JabRef's own setting).
    pub file_directory: String,
    pub citation_style: String,
    pub sort_mode: SortMode,
    pub key_pattern: KeyPattern,
    /// Update `modificationdate` when an entry is edited (as JabRef does).
    pub track_modification_date: bool,
    /// Store fetched data with LaTeX accents instead of Unicode.
    pub fetch_as_latex: bool,
    /// Capitalise English titles in formatted citations.
    pub style_title_case: bool,
    /// Capitalise English titles when entries are added or titles edited.
    pub auto_title_case: bool,
    /// Store DOIs of fetched entries as shortDOIs.
    pub short_dois: bool,
    pub window_width: i32,
    pub window_height: i32,
    pub pane_position: i32,
    /// Owner name written to new entries (JabRef's `owner` field); empty = none.
    pub owner: String,
    pub font_scale: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            last_file: None,
            recent_files: Vec::new(),
            file_directory: String::new(),
            citation_style: "chicago-author-date-17th-edition".into(),
            sort_mode: SortMode::Latin,
            key_pattern: KeyPattern::Auth4Year,
            track_modification_date: true,
            fetch_as_latex: false,
            style_title_case: false,
            auto_title_case: false,
            short_dois: false,
            window_width: 1400,
            window_height: 900,
            pane_position: 800,
            owner: String::new(),
            font_scale: 1.0,
        }
    }
}

fn path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("suci").join("settings.json"))
}

impl Settings {
    pub fn load() -> Settings {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        if let Some(p) = path() {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(s) = serde_json::to_string_pretty(self) {
                let _ = std::fs::write(p, s);
            }
        }
    }

    pub fn remember_file(&mut self, p: &std::path::Path) {
        let p = p.to_path_buf();
        self.last_file = Some(p.clone());
        self.recent_files.retain(|x| *x != p);
        self.recent_files.insert(0, p);
        self.recent_files.truncate(10);
    }
}
