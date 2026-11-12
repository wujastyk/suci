//! Copying entries from one library into another .bib file.

use crate::bib::Entry;
use crate::doc::Document;
use crate::fetch::Ident;
use crate::files::{parse_file_field, serialize_file_field, FileDirs};
use crate::keygen::unique_key;
use std::collections::{HashMap, HashSet};

/// What stands in the way of copying one entry.
#[derive(Debug, Clone)]
pub enum Clash {
    /// nothing: the key is free
    None,
    /// the target has this key with the same content already
    Identical,
    /// the target has this key for a different entry
    KeyTaken(Entry),
}

#[derive(Debug, Clone)]
pub struct Planned {
    pub entry: Entry,
    pub clash: Clash,
    /// the target already has the same work under another key
    /// (same DOI or ISBN, or the same content): that key
    pub same_work: Option<String>,
    /// copied because a copied volume needs its set (crossref parent)
    pub as_parent: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// add under this key
    Add(String),
    /// overwrite the target's entry with the same key
    Replace,
    Skip,
}

impl Planned {
    /// The choice offered first: never overwrite, never duplicate.
    pub fn default_action(&self, suggested_key: &str) -> Action {
        match (&self.clash, &self.same_work) {
            (Clash::Identical, _) => Action::Skip,
            (_, Some(_)) => Action::Skip,
            (Clash::KeyTaken(_), None) => Action::Add(suggested_key.to_string()),
            (Clash::None, None) => Action::Add(self.entry.key.clone()),
        }
    }
}

/// Same content, ignoring housekeeping fields and the order of fields.
pub fn same_content(a: &Entry, b: &Entry) -> bool {
    const IGNORE: &[&str] = &["creationdate", "modificationdate", "timestamp", "owner", "file"];
    let norm = |e: &Entry| -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = e
            .fields
            .iter()
            .filter(|f| !IGNORE.contains(&f.name.to_lowercase().as_str()))
            .map(|f| (f.name.to_lowercase(), e.get(&f.name).unwrap_or_default().split_whitespace().collect::<Vec<_>>().join(" ")))
            .filter(|(_, v)| !v.is_empty())
            .collect();
        v.sort();
        v
    };
    a.ty.eq_ignore_ascii_case(&b.ty) && norm(a) == norm(b)
}

/// Work out what copying `entries` from `source` into `target` would do.
/// Volumes whose set (crossref parent) is missing from the target bring
/// the set along, placed before them.
pub fn plan(source: &Document, target: &Document, entries: &[Entry]) -> Vec<Planned> {
    let chosen: HashSet<String> = entries.iter().map(|e| e.key.to_lowercase()).collect();
    let mut list: Vec<(Entry, bool)> = Vec::new();
    let mut seen = HashSet::new();
    for e in entries {
        if let Some(pk) = e.get_nonempty("crossref") {
            let lk = pk.to_lowercase();
            if !chosen.contains(&lk) && target.find_key(&pk).is_none() && !seen.contains(&lk) {
                if let Some(p) = source.lookup_entry(&pk) {
                    seen.insert(lk);
                    list.push((p, true));
                }
            }
        }
        if seen.insert(e.key.to_lowercase()) {
            list.push((e.clone(), false));
        }
    }
    list.into_iter()
        .map(|(entry, as_parent)| {
            let clash = match target.find_key(&entry.key).and_then(|i| target.entry(i)) {
                None => Clash::None,
                Some(t) if same_content(t, &entry) => Clash::Identical,
                Some(t) => Clash::KeyTaken(t.clone()),
            };
            let same_work = if matches!(clash, Clash::Identical) {
                None
            } else {
                [entry.get_nonempty("doi").map(Ident::Doi), entry.get_nonempty("isbn").map(Ident::Isbn)]
                    .into_iter()
                    .flatten()
                    .find_map(|id| target.find_identifier(&id).first().and_then(|&i| target.entry(i)).map(|t| t.key.clone()))
                    .filter(|k| !k.eq_ignore_ascii_case(&entry.key))
                    // copied before under another key?
                    .or_else(|| target.lib.entries().find(|t| !t.key.eq_ignore_ascii_case(&entry.key) && same_content(t, &entry)).map(|t| t.key.clone()))
            };
            Planned { entry, clash, same_work, as_parent }
        })
        .collect()
}

/// A free key in the target for a copy of `key` (wuja-2022 → wuja-2022a),
/// also avoiding keys already chosen for other copies.
pub fn suggest_key(target: &Document, key: &str, also_taken: &HashSet<String>) -> String {
    let taken: HashSet<String> = target.keys().iter().map(|k| k.to_lowercase()).chain(also_taken.iter().map(|k| k.to_lowercase())).collect();
    unique_key(key, |k| taken.contains(&k.to_lowercase()))
}

/// Linked files are stored relative to a library's PDF folders; make the
/// links of a copied entry work from the target library too.
pub fn relink(e: &mut Entry, from: &FileDirs, to: &FileDirs) {
    let Some(field) = e.get_nonempty("file") else { return };
    let mut files = parse_file_field(&field);
    let mut changed = false;
    for f in &mut files {
        if f.path.contains("://") {
            continue;
        }
        let Some(abs) = from.resolve(&f.path) else { continue };
        if to.resolve(&f.path).as_deref() == Some(abs.as_path()) {
            continue;
        }
        f.path = to.relativize(&abs);
        changed = true;
    }
    if changed {
        e.set("file", &serialize_file_field(&files));
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Outcome {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub skipped: Vec<String>,
}

/// Carry out the chosen actions on the target (in memory; the caller saves).
/// A copy added under a new key keeps its volumes pointing at it.
pub fn apply(source: &Document, target: &mut Document, planned: &[Planned], actions: &[Action]) -> Result<Outcome, String> {
    // new keys must be free and distinct
    let mut new_keys = HashSet::new();
    for (p, a) in planned.iter().zip(actions) {
        if let Action::Add(k) = a {
            let k = k.trim();
            if k.is_empty() || k.contains(|c: char| c.is_whitespace() || "{}(),=\"#%'".contains(c)) {
                return Err(format!("“{k}” cannot be used as a citation key."));
            }
            let lk = k.to_lowercase();
            if target.find_key(k).is_some() || !new_keys.insert(lk) {
                return Err(format!("The key {k} is already taken in the target library (copy of {}).", p.entry.key));
            }
        }
    }
    let renamed: HashMap<String, String> = planned
        .iter()
        .zip(actions)
        .filter_map(|(p, a)| match a {
            Action::Add(k) if k.trim() != p.entry.key => Some((p.entry.key.to_lowercase(), k.trim().to_string())),
            _ => None,
        })
        .collect();
    let mut out = Outcome::default();
    for (p, a) in planned.iter().zip(actions) {
        let mut e = p.entry.clone();
        relink(&mut e, &source.file_dirs, &target.file_dirs);
        if let Some(pk) = e.get_nonempty("crossref") {
            if let Some(nk) = renamed.get(&pk.to_lowercase()) {
                e.set("crossref", nk);
            }
        }
        match a {
            Action::Skip => out.skipped.push(p.entry.key.clone()),
            Action::Add(k) => {
                e.key = k.trim().to_string();
                out.added.push(e.key.clone());
                target.add_entry(e);
            }
            Action::Replace => {
                let Some(i) = target.find_key(&e.key) else {
                    out.added.push(e.key.clone());
                    target.add_entry(e);
                    continue;
                };
                out.replaced.push(e.key.clone());
                target.update_entry(i, e);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::Library;

    fn doc(text: &str) -> Document {
        let dir = std::env::temp_dir().join(format!("suci-transfer-{}-{}", std::process::id(), text.len()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.bib");
        std::fs::write(&p, text).unwrap();
        Document::open(&p, None).unwrap()
    }

    fn entry(text: &str) -> Entry {
        Library::parse(text).entries().next().unwrap().clone()
    }

    #[test]
    fn plans_and_copies() {
        let source = doc("@MvBook{kavi-sadv,\n  title = {Sadvaidya},\n  date = {1900},\n}\n\n@Book{kavi-sadv2,\n  crossref = {kavi-sadv},\n  volume = {2},\n}\n\n@Book{wuja-2003,\n  author = {Wujastyk, Dominik},\n  title = {The Roots of Ayurveda},\n  date = {2003},\n  doi = {10.1/roots},\n}\n");
        let mut target = doc("@Book{wuja-2003,\n  author = {Wujastyk, Dominik},\n  title = {Something else},\n  date = {2003},\n}\n\n@Book{other,\n  title = {Other},\n  doi = {10.1/roots},\n}\n");
        let a = entry("@Book{kavi-sadv2,\n  crossref = {kavi-sadv},\n  volume = {2},\n}\n");
        let b = source.lookup_entry("wuja-2003").unwrap();
        let p = plan(&source, &target, &[a, b]);
        // the volume brings its set along, first
        assert_eq!(p.iter().map(|x| x.entry.key.as_str()).collect::<Vec<_>>(), ["kavi-sadv", "kavi-sadv2", "wuja-2003"]);
        assert!(p[0].as_parent);
        assert!(matches!(p[2].clash, Clash::KeyTaken(_)));
        let sugg = suggest_key(&target, "wuja-2003", &HashSet::new());
        assert_eq!(sugg, "wuja-2003a");
        // the target has this work already, as "other" (same DOI): not copied by default
        assert_eq!(p[2].same_work.as_deref(), Some("other"));
        assert_eq!(p[2].default_action(&sugg), Action::Skip);

        // rename the set too: its volume must follow
        let actions = vec![Action::Add("kavi-sadv-x".into()), Action::Add("kavi-sadv2".into()), Action::Add(sugg)];
        let out = apply(&source, &mut target, &p, &actions).unwrap();
        assert_eq!(out.added, ["kavi-sadv-x", "kavi-sadv2", "wuja-2003a"]);
        let v = target.lookup_entry("kavi-sadv2").unwrap();
        assert_eq!(v.get("crossref").unwrap(), "kavi-sadv-x");
        // the original wuja-2003 in the target is untouched
        assert_eq!(target.lookup_entry("wuja-2003").unwrap().get("title").unwrap(), "Something else");
    }

    #[test]
    fn identical_same_work_and_replace() {
        let source = doc("@Book{a,\n  title = {A},\n  doi = {10.1/x},\n  owner = {me},\n}\n\n@Book{b,\n  title = {B},\n  isbn = {9780195171464},\n}\n");
        let mut target = doc("@book{a,\n  title    = {A},\n  doi = {10.1/x},\n}\n\n@Book{z,\n  title = {B again},\n  isbn = {978-0-19-517146-4},\n}\n");
        let es = vec![source.lookup_entry("a").unwrap(), source.lookup_entry("b").unwrap()];
        let p = plan(&source, &target, &es);
        assert!(matches!(p[0].clash, Clash::Identical));
        assert_eq!(p[1].same_work.as_deref(), Some("z"));
        assert_eq!(p[0].default_action("a1"), Action::Skip);
        assert_eq!(p[1].default_action("b"), Action::Skip);
        // a taken key is refused for Add
        assert!(apply(&source, &mut target, &p, &[Action::Add("z".into()), Action::Skip]).is_err());
        let out = apply(&source, &mut target, &p, &[Action::Replace, Action::Add("b".into())]).unwrap();
        assert_eq!(out.replaced, ["a"]);
        assert_eq!(out.added, ["b"]);
    }

    #[test]
    fn copied_before_under_another_key() {
        let source = doc("@Book{wuja-2003,\n  title = {The Roots of Ayurveda},\n  date = {2003},\n}\n");
        let target = doc("@Book{wuja-2003,\n  title = {Well-Mannered Medicine},\n}\n\n@Book{wuja-roots,\n  title = {The Roots of Ayurveda},\n  date = {2003},\n  creationdate = {2026-11-12T13:36:42},\n}\n");
        let p = plan(&source, &target, &[source.lookup_entry("wuja-2003").unwrap()]);
        assert!(matches!(p[0].clash, Clash::KeyTaken(_)));
        assert_eq!(p[0].same_work.as_deref(), Some("wuja-roots"));
        assert_eq!(p[0].default_action("wuja-2003a"), Action::Skip);
    }

    #[test]
    fn file_links_follow() {
        let base = std::env::temp_dir().join(format!("suci-relink-{}", std::process::id()));
        let (src, dst) = (base.join("src"), base.join("dst"));
        std::fs::create_dir_all(src.join("pdf")).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        std::fs::write(src.join("pdf/x.pdf"), b"%PDF").unwrap();
        let from = FileDirs { dirs: vec![src.clone()] };
        let to = FileDirs { dirs: vec![dst.clone()] };
        let mut e = entry("@Book{x,\n  file = {:pdf/x.pdf:PDF},\n}\n");
        relink(&mut e, &from, &to);
        // the target has no such folder: the link becomes absolute, and still resolves
        let f = &parse_file_field(&e.get("file").unwrap())[0];
        assert_eq!(to.resolve(&f.path).unwrap(), src.join("pdf/x.pdf"));
        // a target sharing the PDF folder keeps the short link
        let mut e2 = entry("@Book{x,\n  file = {:pdf/x.pdf:PDF},\n}\n");
        relink(&mut e2, &from, &FileDirs { dirs: vec![src.clone()] });
        assert_eq!(e2.get("file").unwrap(), ":pdf/x.pdf:PDF");
    }
}
