//! An open library file: entries, search index, undo history, saving.
//!
//! Entries are addressed by their index in `Library::items`. Indices never
//! change: deleted entries leave an `Item::Deleted` slot and new entries are
//! inserted after the last entry (before JabRef's trailing @Comment blocks).

use crate::bib::{Entry, Item, Library};
use crate::files::FileDirs;
use crate::keygen::{base_key, unique_key, KeyPattern};
use crate::profile::Profile;
use crate::search::{Query, SearchIndex};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone)]
struct Change {
    idx: usize,
    before: Item,
    after: Item,
}

#[derive(Debug)]
pub enum SaveError {
    /// The file was changed on disk by another program since it was opened.
    ChangedOnDisk,
    Io(String),
    NoPath,
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SaveError::ChangedOnDisk => write!(f, "The file was changed by another program since it was opened."),
            SaveError::Io(e) => write!(f, "{e}"),
            SaveError::NoPath => write!(f, "No file name"),
        }
    }
}

pub struct Document {
    pub path: Option<PathBuf>,
    pub lib: Library,
    pub profile: Profile,
    pub file_dirs: FileDirs,
    index: Vec<Option<SearchIndex>>,
    undo: Vec<Vec<Change>>,
    redo: Vec<Vec<Change>>,
    /// Number of changes since the last save (negative after undoing past it).
    saved_at: usize,
    mtime: Option<SystemTime>,
    pub track_modification_date: bool,
    pub key_pattern: KeyPattern,
    pub owner: String,
    /// The file started with a UTF-8 byte-order mark; keep it.
    bom: bool,
}

/// Search index for all items, built on several threads.
fn build_index(lib: &Library) -> Vec<Option<SearchIndex>> {
    let items = &lib.items;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 8);
    let chunk = items.len().div_ceil(threads).max(1);
    std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter().map(|it| if let Item::Entry(e) = it { Some(SearchIndex::build(e)) } else { None }).collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    })
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string()
}

impl Document {
    pub fn new_empty() -> Document {
        let mut lib = Library::default();
        lib.items.push(Item::Special {
            kind: "Comment".into(),
            body: "jabref-meta: databaseType:biblatex;".into(),
            raw: "@Comment{jabref-meta: databaseType:biblatex;}".into(),
        });
        lib.items.push(Item::Junk("\n".into()));
        Document::from_library(lib, None)
    }

    pub fn open(path: &Path, file_dir_setting: Option<&str>) -> Result<Document, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
        let text = match String::from_utf8(bytes) {
            Ok(t) => t,
            Err(e) => {
                // Latin-1 fallback for old files
                e.into_bytes().iter().map(|&b| b as char).collect()
            }
        };
        let bom = text.starts_with('\u{feff}');
        let text = text.strip_prefix('\u{feff}').map(|s| s.to_string()).unwrap_or(text);
        let lib = Library::parse(&text);
        let mut d = Document::from_library(lib, Some(path.to_path_buf()));
        d.bom = bom;
        d.mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        d.file_dirs = FileDirs::discover(&d.lib.jabref_meta(), Some(path), file_dir_setting);
        Ok(d)
    }

    fn from_library(lib: Library, path: Option<PathBuf>) -> Document {
        let index = build_index(&lib);
        let profile = Profile::build(&lib);
        Document {
            path,
            lib,
            profile,
            file_dirs: FileDirs::default(),
            index,
            undo: Vec::new(),
            redo: Vec::new(),
            saved_at: 0,
            mtime: None,
            track_modification_date: true,
            key_pattern: KeyPattern::Auth4Year,
            owner: String::new(),
            bom: false,
        }
    }

    pub fn refresh_file_dirs(&mut self, setting: Option<&str>) {
        self.file_dirs = FileDirs::discover(&self.lib.jabref_meta(), self.path.as_deref(), setting);
    }

    pub fn is_dirty(&self) -> bool {
        self.undo.len() != self.saved_at
    }

    pub fn entry(&self, idx: usize) -> Option<&Entry> {
        match self.lib.items.get(idx) {
            Some(Item::Entry(e)) => Some(e),
            _ => None,
        }
    }

    pub fn entry_indices(&self) -> Vec<usize> {
        self.lib.items.iter().enumerate().filter_map(|(i, it)| matches!(it, Item::Entry(_)).then_some(i)).collect()
    }

    pub fn find_key(&self, key: &str) -> Option<usize> {
        self.lib.items.iter().position(|it| matches!(it, Item::Entry(e) if e.key == key))
    }

    pub fn search(&self, q: &str) -> Vec<usize> {
        let query = Query::parse(q);
        self.index
            .iter()
            .enumerate()
            .filter_map(|(i, ix)| ix.as_ref().filter(|ix| query.is_empty() || query.matches(ix)).map(|_| i))
            .collect()
    }

    pub fn keys(&self) -> HashSet<String> {
        self.lib.entries().map(|e| e.key.clone()).collect()
    }

    /// A fresh unique key for an entry (ignoring the entry's own current key).
    pub fn generate_key(&self, e: &Entry, own_idx: Option<usize>) -> String {
        let base = base_key(e, self.key_pattern);
        let taken: HashSet<String> = self
            .lib
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| match it {
                Item::Entry(x) if Some(i) != own_idx => Some(x.key.to_lowercase()),
                _ => None,
            })
            .collect();
        unique_key(&base, |k| taken.contains(&k.to_lowercase()))
    }

    fn set_item(&mut self, idx: usize, item: Item) {
        if idx >= self.lib.items.len() {
            self.lib.items.resize(idx + 1, Item::Deleted);
            self.index.resize(idx + 1, None);
        }
        self.index[idx] = if let Item::Entry(e) = &item { Some(SearchIndex::build(e)) } else { None };
        self.lib.items[idx] = item;
    }

    fn commit(&mut self, changes: Vec<Change>) {
        if changes.is_empty() {
            return;
        }
        if self.saved_at > self.undo.len() {
            // the saved state is no longer reachable by undo
            self.saved_at = usize::MAX;
        }
        self.undo.push(changes);
        self.redo.clear();
    }

    /// Replace an entry. Returns false if nothing changed.
    pub fn update_entry(&mut self, idx: usize, mut new: Entry) -> bool {
        let Some(old) = self.entry(idx).cloned() else { return false };
        if old.ty == new.ty && old.key == new.key && old.fields == new.fields {
            return false;
        }
        new.raw = None;
        if self.track_modification_date {
            new.set("modificationdate", &now_stamp());
        }
        self.set_item(idx, Item::Entry(new.clone()));
        self.commit(vec![Change { idx, before: Item::Entry(old), after: Item::Entry(new) }]);
        true
    }

    /// Several entry updates as one undo step.
    pub fn update_entries(&mut self, updates: Vec<(usize, Entry)>) -> usize {
        let mut changes = Vec::new();
        let stamp = now_stamp();
        for (idx, mut new) in updates {
            let Some(old) = self.entry(idx).cloned() else { continue };
            if old.fields == new.fields && old.key == new.key && old.ty == new.ty {
                continue;
            }
            new.raw = None;
            if self.track_modification_date {
                new.set("modificationdate", &stamp);
            }
            self.set_item(idx, Item::Entry(new.clone()));
            changes.push(Change { idx, before: Item::Entry(old), after: Item::Entry(new) });
        }
        let n = changes.len();
        self.commit(changes);
        n
    }

    /// Insertion slot for new entries: after the last entry.
    fn insertion_point(&self) -> usize {
        self.lib
            .items
            .iter()
            .rposition(|it| matches!(it, Item::Entry(_) | Item::Deleted))
            .map(|p| p + 1)
            .unwrap_or(0)
    }

    pub fn add_entry(&mut self, mut e: Entry) -> usize {
        e.raw = None;
        if e.key.trim().is_empty() {
            e.key = self.generate_key(&e, None);
        }
        if self.track_modification_date {
            let stamp = now_stamp();
            if e.get_nonempty("creationdate").is_none() {
                e.set("creationdate", &stamp);
            }
        }
        if !self.owner.is_empty() && e.get_nonempty("owner").is_none() {
            e.set("owner", &self.owner);
        }
        let pos = self.insertion_point();
        // insert a slot plus a blank-line separator; existing entries keep their indices
        self.lib.items.insert(pos, Item::Deleted);
        self.index.insert(pos, None);
        self.lib.items.insert(pos, Item::Junk("\n\n".into()));
        self.index.insert(pos, None);
        let idx = pos + 1;
        self.set_item(idx, Item::Entry(e.clone()));
        self.commit(vec![Change { idx, before: Item::Deleted, after: Item::Entry(e) }]);
        self.profile = Profile::build(&self.lib);
        idx
    }

    pub fn delete(&mut self, idxs: &[usize]) -> usize {
        let mut changes = Vec::new();
        for &idx in idxs {
            if let Some(old) = self.entry(idx).cloned() {
                self.set_item(idx, Item::Deleted);
                changes.push(Change { idx, before: Item::Entry(old), after: Item::Deleted });
            }
        }
        let n = changes.len();
        self.commit(changes);
        n
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Returns the affected item indices.
    pub fn undo(&mut self) -> Vec<usize> {
        let Some(changes) = self.undo.pop() else { return vec![] };
        let mut idxs = Vec::new();
        for c in changes.iter().rev() {
            self.set_item(c.idx, c.before.clone());
            idxs.push(c.idx);
        }
        self.redo.push(changes);
        idxs
    }

    pub fn redo(&mut self) -> Vec<usize> {
        let Some(changes) = self.redo.pop() else { return vec![] };
        let mut idxs = Vec::new();
        for c in &changes {
            self.set_item(c.idx, c.after.clone());
            idxs.push(c.idx);
        }
        self.undo.push(changes);
        idxs
    }

    pub fn changed_on_disk(&self) -> bool {
        match (&self.path, self.mtime) {
            (Some(p), Some(t)) => std::fs::metadata(p).and_then(|m| m.modified()).map(|m| m != t).unwrap_or(false),
            _ => false,
        }
    }

    /// Save to `self.path`. A copy of the previous file is kept as `name.bib.bak`.
    pub fn save(&mut self, force: bool) -> Result<(), SaveError> {
        let path = self.path.clone().ok_or(SaveError::NoPath)?;
        if !force && self.changed_on_disk() {
            return Err(SaveError::ChangedOnDisk);
        }
        let mut text = self.lib.to_bibtex();
        if self.bom {
            text.insert(0, '\u{feff}');
        }
        if path.exists() {
            let bak = PathBuf::from(format!("{}.bak", path.display()));
            let _ = std::fs::copy(&path, bak);
        }
        let tmp = PathBuf::from(format!("{}.suci-tmp", path.display()));
        std::fs::write(&tmp, text.as_bytes()).map_err(|e| SaveError::Io(format!("Cannot write {}: {e}", tmp.display())))?;
        if let Ok(meta) = std::fs::metadata(&path) {
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, &path).map_err(|e| SaveError::Io(format!("Cannot replace {}: {e}", path.display())))?;
        self.mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        self.saved_at = self.undo.len();
        // saved entries now match the file; let unchanged ones keep their text
        Ok(())
    }

    pub fn save_as(&mut self, path: &Path) -> Result<(), SaveError> {
        self.path = Some(path.to_path_buf());
        self.mtime = None;
        self.save(true)
    }

    /// Reload search data after external edits to many entries.
    pub fn rebuild(&mut self) {
        self.index = build_index(&self.lib);
        self.profile = Profile::build(&self.lib);
    }

    pub fn lookup_entry(&self, key: &str) -> Option<Entry> {
        self.lib.entries().find(|e| e.key.eq_ignore_ascii_case(key)).cloned()
    }

    /// Entries that already carry this DOI or ISBN.
    pub fn find_identifier(&self, id: &crate::fetch::Ident) -> Vec<usize> {
        let mut out = Vec::new();
        for (i, it) in self.lib.items.iter().enumerate() {
            let Item::Entry(e) = it else { continue };
            let hit = match id {
                crate::fetch::Ident::Doi(d) => e
                    .get_nonempty("doi")
                    .is_some_and(|x| crate::checks::clean_doi(&x).eq_ignore_ascii_case(d)),
                crate::fetch::Ident::Isbn(isbn) => {
                    let want = crate::checks::isbn13(isbn);
                    e.get_nonempty("isbn").is_some_and(|x| {
                        crate::checks::split_isbns(&x).iter().any(|i| crate::checks::isbn13(i) == want)
                    }) && !matches!(e.ty_lower().as_str(), "incollection" | "inbook" | "inproceedings")
                }
            };
            if hit {
                out.push(i);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_undo_save() {
        let dir = std::env::temp_dir().join(format!("suci-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.bib");
        let src = "@Book{a,\n  title = {A},\n}\n\n@Book{b,\n  title = {B},\n}\n\n@Comment{jabref-meta: databaseType:biblatex;}\n";
        std::fs::write(&p, src).unwrap();
        let mut d = Document::open(&p, None).unwrap();
        d.track_modification_date = false;
        assert_eq!(d.entry_indices().len(), 2);
        assert!(!d.is_dirty());
        // edit
        let i = d.find_key("a").unwrap();
        let mut e = d.entry(i).unwrap().clone();
        e.set("date", "2001");
        assert!(d.update_entry(i, e));
        assert!(d.is_dirty());
        // add
        let mut n = Entry::new("Article", "");
        n.set("author", "Dominik Wujastyk");
        n.set("title", "New");
        n.set("date", "2024");
        let ni = d.add_entry(n);
        assert_eq!(d.entry(ni).unwrap().key, "wuja-2024");
        // delete b
        let bi = d.find_key("b").unwrap();
        d.delete(&[bi]);
        d.save(false).unwrap();
        let out = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            out,
            "@Book{a,\n  title = {A},\n  date  = {2001},\n}\n\n@Article{wuja-2024,\n  author = {Dominik Wujastyk},\n  title  = {New},\n  date   = {2024},\n}\n\n@Comment{jabref-meta: databaseType:biblatex;}\n"
        );
        assert!(dir.join("t.bib.bak").exists());
        assert!(!d.is_dirty());
        // undo delete, add, edit
        d.undo();
        d.undo();
        d.undo();
        assert!(d.is_dirty());
        d.save(false).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), src.replace("@Book{a,\n  title = {A},\n}", "@Book{a,\n  title = {A},\n}"));
        // external modification is detected
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&p, "@Book{z, title={Z}}").unwrap();
        let mut e = d.entry(i).unwrap().clone();
        e.set("note", "x");
        d.update_entry(i, e);
        assert!(matches!(d.save(false), Err(SaveError::ChangedOnDisk)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
