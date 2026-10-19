//! Field-usage profile of a library, used to lay out the entry editor with the
//! fields this user actually fills in.

use crate::bib::Library;
use std::collections::HashMap;

/// Scholarly reading order for fields.
pub const CANONICAL: &[&str] = &[
    "author", "editor", "translator", "bookauthor", "title", "subtitle", "titleaddon", "booktitle",
    "booksubtitle", "maintitle", "journaltitle", "journal", "issuetitle", "series", "number", "volume",
    "volumes", "edition", "part", "chapter", "pages", "pagetotal", "publisher", "location", "address",
    "institution", "school", "organization", "type", "date", "year", "month", "origdate", "origlanguage",
    "langid", "language", "isbn", "issn", "doi", "url", "urldate", "eprint", "eprinttype", "crossref",
    "related", "relatedtype", "shorthand", "shorttitle", "library", "shelfmark", "collection",
    "shortlibrary", "catalog", "howpublished", "note", "addendum", "keywords", "file", "abstract",
    "annotation", "contents",
];

/// Housekeeping fields shown in a collapsed section.
pub const BOOKKEEPING: &[&str] =
    &["owner", "creationdate", "modificationdate", "timestamp", "citationkey", "groups", "source", "ppn_gvk"];

/// Fields edited in a multi-line box.
pub const MULTILINE: &[&str] = &["abstract", "annotation", "annote", "note", "contents", "comment", "addendum"];

/// biblatex required fields per type ("a|b" = either).
pub fn required_fields(ty: &str, biblatex: bool) -> Vec<&'static str> {
    let date = if biblatex { "date|year" } else { "year|date" };
    let journal = if biblatex { "journaltitle|journal" } else { "journal|journaltitle" };
    let inst = "institution|school";
    match ty {
        "article" | "review" => vec!["author", "title", journal, date],
        "book" | "mvbook" => vec!["author|editor", "title", date],
        "inbook" | "bookinbook" | "suppbook" => vec!["author", "title", "booktitle", date],
        "incollection" | "suppcollection" | "inreference" => vec!["author", "title", "booktitle", date],
        "collection" | "mvcollection" | "reference" | "mvreference" => vec!["editor|author", "title", date],
        "inproceedings" => vec!["author", "title", "booktitle", date],
        "proceedings" | "mvproceedings" => vec!["title", date],
        "thesis" | "phdthesis" | "mastersthesis" => vec!["author", "title", inst, date],
        "report" | "techreport" => vec!["author", "title", inst, date],
        "online" | "electronic" | "www" => vec!["title", "url"],
        "unpublished" => vec!["author", "title"],
        "booklet" => vec!["title"],
        "manual" => vec!["title"],
        "periodical" => vec!["title", date],
        "patent" => vec!["author", "title", "number", date],
        "software" | "dataset" => vec!["title"],
        _ => vec!["title"],
    }
}

#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// type -> field -> count
    pub by_type: HashMap<String, HashMap<String, usize>>,
    pub type_counts: HashMap<String, usize>,
    pub global: HashMap<String, usize>,
}

impl Profile {
    pub fn build(lib: &Library) -> Profile {
        let mut p = Profile::default();
        for e in lib.entries() {
            let ty = e.ty_lower();
            *p.type_counts.entry(ty.clone()).or_default() += 1;
            let m = p.by_type.entry(ty).or_default();
            for f in &e.fields {
                let n = f.name.to_lowercase();
                *m.entry(n.clone()).or_default() += 1;
                *p.global.entry(n).or_default() += 1;
            }
        }
        p
    }

    /// Share of entries of this type that use the field (0..1).
    pub fn usage(&self, ty: &str, field: &str) -> f64 {
        let n = *self.type_counts.get(ty).unwrap_or(&0);
        if n < 5 {
            // too few entries of this type: fall back to library-wide usage
            let total: usize = self.type_counts.values().sum();
            return *self.global.get(field).unwrap_or(&0) as f64 / total.max(1) as f64;
        }
        *self.by_type.get(ty).and_then(|m| m.get(field)).unwrap_or(&0) as f64 / n as f64
    }

    /// Entry types ordered by frequency.
    pub fn common_types(&self) -> Vec<String> {
        let mut v: Vec<_> = self.type_counts.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        v.into_iter().map(|(k, _)| k.clone()).collect()
    }

    /// Field names ordered by library-wide frequency (for "add field").
    pub fn fields_by_frequency(&self) -> Vec<String> {
        let mut v: Vec<_> = self.global.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter().map(|(k, _)| k.clone()).collect()
    }
}

/// Editor layout for one entry.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Layout {
    /// Fields shown as blank slots even if empty: the ones this library uses
    /// for this type, in reading order.
    pub main: Vec<String>,
    /// Other non-empty fields in this entry.
    pub other: Vec<String>,
    /// Housekeeping fields present in this entry.
    pub bookkeeping: Vec<String>,
}

/// Fields used in at least this share of a type's entries get a slot.
pub const SLOT_THRESHOLD: f64 = 0.12;

pub fn layout(profile: &Profile, ty: &str, present: &[String], biblatex: bool) -> Layout {
    let ty = ty.to_lowercase();
    let present_l: Vec<String> = present.iter().map(|s| s.to_lowercase()).collect();
    let req: Vec<&str> = required_fields(&ty, biblatex).iter().flat_map(|r| r.split('|').take(1)).collect();
    let mut main = Vec::new();
    for &f in CANONICAL {
        if BOOKKEEPING.contains(&f) {
            continue;
        }
        let is_present = present_l.iter().any(|p| p == f);
        let used = profile.usage(&ty, f) >= SLOT_THRESHOLD;
        // don't offer both spellings of an alias pair unless present
        let alias_dup = match f {
            "journal" => !is_present && (biblatex || present_l.iter().any(|p| p == "journaltitle")),
            "journaltitle" => !is_present && !biblatex,
            "address" => !is_present && (biblatex || present_l.iter().any(|p| p == "location")),
            "location" => !is_present && !biblatex,
            "year" => !is_present && biblatex,
            "school" => !is_present && biblatex,
            _ => false,
        };
        if alias_dup {
            continue;
        }
        if is_present || used || req.contains(&f) {
            main.push(f.to_string());
        }
    }
    let mut other = Vec::new();
    let mut bookkeeping = Vec::new();
    for (orig, l) in present.iter().zip(present_l.iter()) {
        if main.contains(l) {
            continue;
        }
        if BOOKKEEPING.contains(&l.as_str()) {
            bookkeeping.push(orig.clone());
        } else {
            other.push(orig.clone());
        }
    }
    Layout { main, other, bookkeeping }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_prioritises_used_fields() {
        let mut src = String::new();
        for i in 0..20 {
            src.push_str(&format!(
                "@Book{{b{i}, author={{A}}, title={{T}}, date={{2000}}, publisher={{P}}, location={{L}}, owner={{dom}}, series={{S}}}}\n"
            ));
        }
        src.push_str("@Book{x, author={A}, title={T}, date={2000}, eprint={123}}\n");
        let lib = Library::parse(&src);
        let p = Profile::build(&lib);
        let e = lib.entries().last().unwrap();
        let present: Vec<String> = e.fields.iter().map(|f| f.name.clone()).collect();
        let l = layout(&p, "book", &present, true);
        assert_eq!(l.main, vec!["author", "title", "series", "publisher", "location", "date", "eprint"]);
        assert!(l.other.is_empty());
        let l = layout(&p, "book", &["owner".to_string()], true);
        assert_eq!(l.bookkeeping, vec!["owner"]);
    }
}
