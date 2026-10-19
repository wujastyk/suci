//! Search queries.
//!
//! Syntax (all terms must match; case- and diacritic-insensitive):
//!   `wujastyk medicine`      words anywhere
//!   `"science of medicine"`  phrase
//!   `author:olivelle`        in one field (also `title:`, `key:`, `type:` …)
//!   `year:1990..2000`        year range (also `year:2001`, `year:..1950`)
//!   `has:file`  `has:doi`    field present;  `-has:doi` absent
//!   `-term`                  exclude

use crate::bib::Entry;
use crate::collate::fold;
use crate::latex::to_plain;

#[derive(Debug, Clone, PartialEq)]
enum Cond {
    Any(String),
    Field(String, String),
    Has(String),
    YearRange(Option<i32>, Option<i32>),
}

#[derive(Debug, Clone, PartialEq)]
struct Term {
    neg: bool,
    cond: Cond,
}

#[derive(Debug, Clone, Default)]
pub struct Query {
    terms: Vec<Term>,
}

impl Query {
    pub fn parse(q: &str) -> Query {
        let mut terms = Vec::new();
        let chars: Vec<char> = q.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            while i < chars.len() && chars[i].is_whitespace() {
                i += 1;
            }
            if i >= chars.len() {
                break;
            }
            let mut neg = false;
            if chars[i] == '-' && i + 1 < chars.len() && !chars[i + 1].is_whitespace() {
                neg = true;
                i += 1;
            }
            // optional field prefix
            let mut field = None;
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_' || chars[i] == '-') {
                i += 1;
            }
            if i < chars.len() && chars[i] == ':' && i > start {
                field = Some(chars[start..i].iter().collect::<String>().to_lowercase());
                i += 1;
            } else {
                i = start;
            }
            // value: quoted or bare
            let value: String = if i < chars.len() && chars[i] == '"' {
                i += 1;
                let s = i;
                while i < chars.len() && chars[i] != '"' {
                    i += 1;
                }
                let v: String = chars[s..i].iter().collect();
                i += 1;
                v
            } else {
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[s..i].iter().collect()
            };
            let cond = match field.as_deref() {
                Some("has") => Cond::Has(value.to_lowercase()),
                Some("year") | Some("date") if value.contains("..") || value.chars().all(|c| c.is_ascii_digit()) => {
                    let (a, b) = match value.split_once("..") {
                        Some((a, b)) => (a.parse().ok(), b.parse().ok()),
                        None => {
                            let y = value.parse().ok();
                            (y, y)
                        }
                    };
                    Cond::YearRange(a, b)
                }
                Some(f) => {
                    if value.is_empty() {
                        continue;
                    }
                    Cond::Field(f.to_string(), fold(&value))
                }
                None => {
                    if value.is_empty() {
                        continue;
                    }
                    Cond::Any(fold(&value))
                }
            };
            terms.push(Term { neg, cond });
        }
        Query { terms }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    pub fn matches(&self, idx: &SearchIndex) -> bool {
        self.terms.iter().all(|t| t.matches(idx) != t.neg)
    }
}

impl Term {
    fn matches(&self, idx: &SearchIndex) -> bool {
        match &self.cond {
            Cond::Any(v) => idx.all.contains(v.as_str()),
            Cond::Field(f, v) => idx.fields.iter().any(|(n, t)| field_covers(f, n) && t.contains(v.as_str())),
            Cond::Has(f) => idx.fields.iter().any(|(n, t)| field_covers(f, n) && !t.trim().is_empty()),
            Cond::YearRange(a, b) => match idx.year {
                Some(y) => a.map_or(true, |a| y >= a) && b.map_or(true, |b| y <= b),
                None => false,
            },
        }
    }
}

/// Does a query field name (possibly an alias) cover this entry field?
fn field_covers(query: &str, field: &str) -> bool {
    let group: &[&str] = match query {
        "journal" | "journaltitle" => &["journal", "journaltitle"],
        "location" | "address" | "place" => &["location", "address"],
        "pdf" | "file" => &["file"],
        "year" | "date" => &["year", "date"],
        "school" | "institution" => &["school", "institution"],
        "note" | "notes" => &["note", "annotation", "annote", "abstract", "comment"],
        "person" | "name" => &["author", "editor", "translator", "bookauthor"],
        _ => return query == field,
    };
    group.contains(&field)
}

/// Pre-folded text of one entry.
#[derive(Debug, Clone, Default)]
pub struct SearchIndex {
    pub all: String,
    pub fields: Vec<(String, String)>,
    pub year: Option<i32>,
}

impl SearchIndex {
    pub fn build(e: &Entry) -> SearchIndex {
        let mut fields = Vec::with_capacity(e.fields.len() + 2);
        fields.push(("key".to_string(), fold(&e.key)));
        fields.push(("type".to_string(), e.ty.to_lowercase()));
        let mut all = String::new();
        all.push_str(&fold(&e.key));
        all.push('\n');
        for f in &e.fields {
            let t = fold(&to_plain(&f.editor_text()));
            all.push_str(&t);
            all.push('\n');
            fields.push((f.name.to_lowercase(), t));
        }
        SearchIndex { all, fields, year: entry_year(e) }
    }
}

/// Year from `date` (biblatex) or `year`.
pub fn entry_year(e: &Entry) -> Option<i32> {
    for name in ["date", "year"] {
        if let Some(v) = e.get_nonempty(name) {
            let digits: String = v.trim().trim_start_matches('-').chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 3 {
                if let Ok(y) = digits.parse::<i32>() {
                    return Some(if v.trim().starts_with('-') { -y } else { y });
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::Library;

    #[test]
    fn queries() {
        let lib = Library::parse(
            "@Book{olive-2005, author={Patrick Olivelle}, title={Manu's Code of Law: {M\\=a}nava-Dharma{\\'s}{\\=a}stra}, date={2005}, file={:a.pdf:PDF}}",
        );
        let e = lib.entries().next().unwrap();
        let idx = SearchIndex::build(e);
        let m = |q: &str| Query::parse(q).matches(&idx);
        assert!(m("olivelle"));
        assert!(m("dharmasastra"));
        assert!(m("dharmaśāstra"));
        assert!(m("author:olivelle"));
        assert!(!m("title:olivelle"));
        assert!(m("year:2000..2010"));
        assert!(!m("year:..1999"));
        assert!(m("year:2005"));
        assert!(m("has:file"));
        assert!(!m("-has:file"));
        assert!(m("has:pdf"));
        assert!(m("\"code of law\""));
        assert!(!m("\"law of code\""));
        assert!(m("olivelle -wujastyk"));
        assert!(m("key:olive"));
        assert!(m("type:book"));
    }
}
