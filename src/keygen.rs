//! Citation-key generation, matching the patterns already used in the library:
//! `wuja-2022` (first four letters of the first author's surname, year) or
//! `mitr-yoga` (surname + first significant title word).

use crate::bib::Entry;
use crate::collate::fold;
use crate::latex::to_plain;
use crate::names::parse_names;
use crate::search::entry_year;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum KeyPattern {
    /// `wuja-2022`
    #[default]
    Auth4Year,
    /// `wuja-scie`
    Auth4Title4,
}

fn letters(s: &str) -> String {
    fold(&to_plain(s)).chars().filter(|c| c.is_ascii_alphanumeric()).collect()
}

const STOP: &[&str] = &[
    "the", "a", "an", "on", "of", "in", "and", "to", "for", "der", "die", "das", "des", "le", "la", "les", "il",
    "un", "une", "ein", "eine", "zur", "zum", "du", "de",
];

pub fn base_key(e: &Entry, pattern: KeyPattern) -> String {
    let persons = e.get_nonempty("author").or_else(|| e.get_nonempty("editor")).unwrap_or_default();
    let mut auth = parse_names(&persons)
        .first()
        .map(|n| {
            if n.literal {
                // corporate name: first word
                n.last.split_whitespace().next().unwrap_or("").to_string()
            } else {
                n.last.clone()
            }
        })
        .map(|l| letters(&l).to_lowercase())
        .unwrap_or_default();
    if auth.is_empty() {
        // fall back to the title
        auth = title_word(e);
    }
    let auth: String = auth.chars().take(4).collect();
    let tail = match pattern {
        KeyPattern::Auth4Year => entry_year(e).map(|y| y.to_string()).unwrap_or_default(),
        KeyPattern::Auth4Title4 => title_word(e).chars().take(4).collect(),
    };
    format!("{auth}-{tail}")
}

fn title_word(e: &Entry) -> String {
    let t = to_plain(&e.get("title").unwrap_or_default());
    for w in t.split(|c: char| !c.is_alphanumeric()) {
        let f = fold(w);
        if f.len() > 1 && !STOP.contains(&f.as_str()) {
            return f.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        }
    }
    String::new()
}

/// Unique key, adding a, b, c… on collision.
pub fn unique_key(base: &str, exists: impl Fn(&str) -> bool) -> String {
    if !exists(base) {
        return base.to_string();
    }
    for c in 'a'..='z' {
        let k = format!("{base}{c}");
        if !exists(&k) {
            return k;
        }
    }
    let mut n = 2;
    loop {
        let k = format!("{base}-{n}");
        if !exists(&k) {
            return k;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::Library;

    #[test]
    fn keys() {
        let lib = Library::parse(
            "@Book{x, author={Hari Śaṅkara Śarmā}, title={The Yogasūtra}, date={2003}}\n@Book{y, editor={{Śrāvakabhūmi Study Group}}, title={On Yoga}, year={2007}}",
        );
        let v: Vec<_> = lib.entries().collect();
        assert_eq!(base_key(v[0], KeyPattern::Auth4Year), "sarm-2003");
        assert_eq!(base_key(v[0], KeyPattern::Auth4Title4), "sarm-yoga");
        assert_eq!(base_key(v[1], KeyPattern::Auth4Year), "srav-2007");
        assert_eq!(unique_key("sarm-2003", |k| k == "sarm-2003" || k == "sarm-2003a"), "sarm-2003b");
    }
}
