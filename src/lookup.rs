//! Find the DOI or ISBN of a work from its title, author and year.
//!
//! Crossref and OpenAlex are searched for DOIs (OpenAlex also says where an
//! open-access copy is), K10plus/GVK, Google Books and Open Library for
//! ISBNs and public-domain scans. Results are scored against the query;
//! records that look like book reviews or chapters, when a book is wanted,
//! are set aside with the reason, never offered as the answer.

use crate::bib::Entry;
use crate::collate::fold;
use crate::fetch::{enc, get, langid, marc_entry, marc_records, parse_crossref, person, set, str_at, tex, year_in};
use crate::latex::to_plain;
use crate::names::parse_names;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Article,
    Chapter,
    Book,
    Any,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Article, Kind::Chapter, Kind::Book, Kind::Any];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Article => "Journal article",
            Kind::Chapter => "Chapter in a book",
            Kind::Book => "Book",
            Kind::Any => "Anything",
        }
    }

    pub fn for_entry_type(ty: &str) -> Kind {
        match ty.to_lowercase().as_str() {
            "article" | "periodical" | "suppperiodical" => Kind::Article,
            "incollection" | "inbook" | "inproceedings" | "inreference" | "bookinbook" | "suppbook" | "suppcollection" => Kind::Chapter,
            "book" | "mvbook" | "collection" | "mvcollection" | "proceedings" | "mvproceedings" | "reference" | "mvreference" => Kind::Book,
            _ => Kind::Any,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Query {
    pub title: String,
    /// surnames, as typed
    pub authors: Vec<String>,
    pub year: Option<i32>,
    /// journal or book title
    pub container: String,
    pub kind: Kind,
    /// optional OpenAlex API key (free; raises the daily allowance)
    pub openalex_key: String,
}

impl Query {
    pub fn from_entry(e: &Entry) -> Query {
        let plain = |f: &str| e.get(f).map(|v| to_plain(&v)).unwrap_or_default().trim().to_string();
        let mut title = plain("title");
        let sub = plain("subtitle");
        if !sub.is_empty() && !title.is_empty() {
            title = format!("{title}: {sub}");
        }
        let names = e.get_nonempty("author").or_else(|| e.get_nonempty("editor")).unwrap_or_default();
        let authors = parse_names(&names).into_iter().map(|n| to_plain(&n.last)).filter(|s| !s.trim().is_empty()).take(3).collect();
        let year = e.get_nonempty("date").or_else(|| e.get_nonempty("year")).and_then(|d| first_year(&d));
        let container = ["journaltitle", "journal", "booktitle"].iter().map(|f| plain(f)).find(|s| !s.is_empty()).unwrap_or_default();
        Query { title, authors, year, container, kind: Kind::for_entry_type(&e.ty), openalex_key: String::new() }
    }

    /// From free text typed in the Add box: a four-digit year is taken as the
    /// year, the rest as the title.
    pub fn from_text(s: &str) -> Query {
        let mut year = None;
        let mut words = Vec::new();
        for w in s.split_whitespace() {
            let t = w.trim_matches(|c: char| !c.is_alphanumeric());
            if year.is_none() && t.len() == 4 && t.chars().all(|c| c.is_ascii_digit()) {
                year = t.parse().ok();
                continue;
            }
            words.push(w);
        }
        Query { title: words.join(" "), authors: vec![], year, container: String::new(), kind: Kind::Any, openalex_key: String::new() }
    }
}

fn first_year(s: &str) -> Option<i32> {
    let b = s.as_bytes();
    (0..b.len().saturating_sub(3)).find_map(|i| {
        let w = &s[i..i + 4];
        (w.chars().all(|c| c.is_ascii_digit()) && (i == 0 || !b[i - 1].is_ascii_digit()) && b.get(i + 4).is_none_or(|c| !c.is_ascii_digit()))
            .then(|| w.parse().ok())
            .flatten()
    })
}

#[derive(Debug, Clone)]
pub struct OpenAccess {
    /// page to read it (or the PDF itself)
    pub url: String,
    /// a direct PDF link, when known
    pub pdf: Option<String>,
    /// e.g. "Open access (gold) · published version · CC BY"
    pub note: String,
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub source: String,
    /// the record as an entry (doi / isbn set when known)
    pub entry: Entry,
    /// "Journal article", "Book", …
    pub kind: String,
    pub score: f32,
    pub open_access: Option<OpenAccess>,
    /// why it is set aside (e.g. "looks like a book review")
    pub excluded: Option<String>,
    /// other sources that returned the same DOI/ISBN
    pub also_in: Vec<String>,
}

impl Hit {
    pub fn doi(&self) -> Option<String> {
        self.entry.get_nonempty("doi")
    }
    pub fn isbn(&self) -> Option<String> {
        self.entry.get_nonempty("isbn")
    }
    pub fn strength(&self) -> &'static str {
        if self.score >= STRONG {
            "Strong match"
        } else if self.score >= POSSIBLE {
            "Possible match"
        } else {
            "Weak match"
        }
    }
    fn tier(&self) -> u8 {
        if self.excluded.is_some() {
            0
        } else if self.score >= STRONG {
            3
        } else if self.score >= POSSIBLE {
            2
        } else {
            1
        }
    }
}

pub const STRONG: f32 = 0.85;
pub const POSSIBLE: f32 = 0.65;
const MIN_TITLE: f32 = 0.5;

// ---------- matching ----------

const STOP: &[&str] = &[
    "a", "an", "the", "of", "and", "in", "on", "for", "to", "with", "by", "at", "from", "de", "la", "le", "les", "des", "du", "der", "die",
    "das", "und", "zur", "zum", "im", "et", "il", "di", "del", "e", "y",
];

fn tokens(s: &str) -> Vec<String> {
    fold(&to_plain(s)).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && !STOP.contains(w)).map(|w| w.to_string()).collect()
}

fn main_title(s: &str) -> &str {
    s.split([':', '.', '?', '—', '–', '=']).next().unwrap_or(s)
}

fn f1(q: &[String], h: &[String]) -> f32 {
    if q.is_empty() || h.is_empty() {
        return 0.0;
    }
    let qs: HashSet<&String> = q.iter().collect();
    let hs: HashSet<&String> = h.iter().collect();
    let common = qs.intersection(&hs).count() as f32;
    let p = common / hs.len() as f32;
    let r = common / qs.len() as f32;
    if common == 0.0 { 0.0 } else { 2.0 * p * r / (p + r) }
}

/// 0..1: how well a found title matches the wanted one. Titles with and
/// without their subtitle are compared both ways.
pub fn title_score(want: &str, found: &str) -> f32 {
    let (wf, ff) = (tokens(want), tokens(found));
    let (wm, fm) = (tokens(main_title(want)), tokens(main_title(found)));
    let mut s = f1(&wf, &ff).max(f1(&wm, &fm));
    // same main title, with or without a subtitle
    if !wm.is_empty() && f1(&wm, &fm) >= 0.999 {
        s = s.max(0.95);
    }
    // a fragment of at least three words, all of it in the found title
    let all_in = |short: &[String], long: &[String]| {
        let l: HashSet<&String> = long.iter().collect();
        short.len() >= 3 && short.iter().all(|t| l.contains(t))
    };
    if all_in(&wf, &ff) || all_in(&fm, &wf) {
        s = s.max(0.85);
    }
    s
}

fn surnames(e: &Entry) -> Vec<String> {
    ["author", "editor", "translator"]
        .iter()
        .filter_map(|f| e.get_nonempty(f))
        .flat_map(|names| parse_names(&names))
        .map(|n| fold(&to_plain(&n.last)))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Does the record name this person (in a name field or, for catalogue
/// records, the title page's statement of responsibility)?
fn names_person(e: &Entry, theirs: &[String], who: &str) -> bool {
    theirs.iter().any(|h| name_match(who, h))
        || e.get_nonempty("_resp").is_some_and(|r| {
            let w = fold(who);
            w.len() >= 3 && fold(&r).split(|c: char| !c.is_alphanumeric()).any(|t| t == w)
        })
}

fn name_match(a: &str, b: &str) -> bool {
    let (a, b) = (fold(a), fold(b));
    let (a, b) = (a.trim(), b.trim());
    !a.is_empty() && !b.is_empty() && (a == b || (a.len() >= 4 && b.len() >= 4 && (a.starts_with(b) || b.starts_with(a))))
}

fn score(q: &Query, e: &Entry) -> f32 {
    let mut found_title = e.get("title").unwrap_or_default();
    if let Some(sub) = e.get_nonempty("subtitle") {
        found_title = format!("{found_title}: {sub}");
    }
    let theirs = surnames(e);
    // free text ("Whitney Sanskrit grammar"): an author's name among the words
    let mut named_in_text = false;
    let want_title = if q.authors.is_empty() && !theirs.is_empty() {
        let kept: Vec<&str> = q
            .title
            .split_whitespace()
            .filter(|w| {
                let f = fold(w.trim_matches(|c: char| !c.is_alphanumeric()));
                let hit = f.len() >= 3 && theirs.iter().any(|h| *h == f);
                named_in_text |= hit;
                !hit
            })
            .collect();
        kept.join(" ")
    } else {
        q.title.clone()
    };
    let t = title_score(&want_title, &found_title);
    let a = if named_in_text {
        1.0
    } else if q.authors.is_empty() || (theirs.is_empty() && e.get_nonempty("_resp").is_none()) {
        -1.0
    } else if q.authors.iter().any(|w| names_person(e, &theirs, w)) {
        1.0
    } else {
        0.0
    };
    let y = match (q.year, e.get_nonempty("date").or_else(|| e.get_nonempty("year")).and_then(|d| first_year(&d))) {
        (Some(w), Some(f)) => match (w - f).abs() {
            0 => 1.0,
            1 => 0.75,
            2 => 0.4,
            _ => 0.0,
        },
        _ => -1.0,
    };
    // weigh only what is known on both sides
    let parts = [(0.6, t), (0.25, a), (0.15, y)];
    let known: f32 = parts.iter().filter(|(_, v)| *v >= 0.0).map(|(w, _)| w).sum();
    let mut s = parts.iter().filter(|(_, v)| *v >= 0.0).map(|(w, v)| w * v).sum::<f32>() / known;
    if !q.container.is_empty() {
        let c = ["journaltitle", "booktitle"].iter().find_map(|f| e.get_nonempty(f)).unwrap_or_default();
        if !c.is_empty() && title_score(&q.container, &c) >= 0.8 {
            s += 0.05;
        }
    }
    if a == 0.0 {
        // somebody else's work: never better than a weak match
        s = s.min(POSSIBLE - 0.05);
    }
    if y == 0.0 {
        // years far apart: another edition or reprint at best
        s = s.min(STRONG - 0.01);
    }
    if t < MIN_TITLE {
        s.min(0.3)
    } else if t < 0.7 {
        // only part of the title matches: weak at best
        s.min(POSSIBLE - 0.05)
    } else {
        s.min(1.0)
    }
}

static REVIEW_WORDS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(book reviews?|reviews? of|reviewed by|review article|rezension|besprechung|compte[- ]rendu|recensione|reseña)\b|\breviews?\s*[:\]]|\[review\]").unwrap()
});
static REVIEW_IMPRINT: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b\d+\s*pp\b|\bpp\.\s*[xivlc\d]|[$£€]\s*\d|\brs\.?\s*\d|\bisbn\b|\bhardback\b|\bpaperback\b|\(cloth\)|\bhb\b.*\bpb\b").unwrap()
});
static BY: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\b(by|von|par|ed\.|edited|translated|trans\.)\s").unwrap());

/// Why a found record should not be offered, if it looks like a review of
/// the wanted work, or a part of it when the whole book is wanted.
pub fn exclusion(q: &Query, e: &Entry, raw_type: &str) -> Option<String> {
    let title = to_plain(&format!("{} {}", e.get("title").unwrap_or_default(), e.get("subtitle").unwrap_or_default()));
    if raw_type == "peer-review" || REVIEW_WORDS.is_match(&title) {
        return Some("looks like a review".into());
    }
    if REVIEW_IMPRINT.is_match(&title) {
        return Some("looks like a book review (the title gives pages, price or ISBN)".into());
    }
    // "… By Patrick Olivelle" in a title by somebody else: a review
    if let Some(m) = BY.find(&title) {
        let after = fold(&title[m.end()..]);
        let theirs = surnames(e);
        let named = q.authors.iter().map(|a| fold(a)).find(|a| !a.is_empty() && after.contains(a.as_str()));
        if let Some(a) = named {
            if !names_person(e, &theirs, &a) {
                return Some("looks like a review (names the author in its title)".into());
            }
        }
    }
    // a journal item with the work's own title but by somebody else
    let theirs = surnames(e);
    if e.ty.eq_ignore_ascii_case("article")
        && !q.authors.is_empty()
        && !theirs.is_empty()
        && !q.authors.iter().any(|w| names_person(e, &theirs, w))
        && title_score(&q.title, &title) >= 0.9
    {
        return Some("same title, by someone else: probably a review".into());
    }
    if q.kind == Kind::Book {
        let t = e.ty.to_lowercase();
        let bookish = matches!(t.as_str(), "book" | "mvbook" | "collection" | "mvcollection" | "reference" | "proceedings");
        if !bookish {
            let what = match t.as_str() {
                "article" => "a journal article — perhaps a review of the book",
                "incollection" | "inbook" => "a chapter, not the whole book",
                _ => "not a book",
            };
            return Some(what.into());
        }
    }
    None
}

// ---------- sources ----------

fn search_words(s: &str, max: usize) -> String {
    let t: Vec<String> = to_plain(s)
        .split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’' && c != '-')
        .filter(|w| !w.is_empty())
        .take(max)
        .map(|w| w.to_string())
        .collect();
    t.join(" ")
}

fn q_enc(s: &str) -> String {
    enc(s).replace("%20", "+")
}

fn crossref(q: &Query) -> Result<Vec<Hit>, String> {
    let mut url = format!("https://api.crossref.org/works?rows=15&query.bibliographic={}", q_enc(&search_words(&format!("{} {}", q.title, q.container), 30)));
    if !q.authors.is_empty() {
        url.push_str(&format!("&query.author={}", q_enc(&q.authors.join(" "))));
    }
    let types = match q.kind {
        Kind::Book => "type:book,type:monograph,type:edited-book,type:reference-book,type:book-set",
        Kind::Article => "type:journal-article",
        Kind::Chapter => "type:book-chapter,type:book-part,type:book-section,type:reference-entry,type:proceedings-article",
        Kind::Any => "",
    };
    if !types.is_empty() {
        url.push_str(&format!("&filter={types}"));
    }
    parse_crossref_search(&get(&url, "application/json")?)
}

pub fn parse_crossref_search(body: &str) -> Result<Vec<Hit>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let items = v.pointer("/message/items").and_then(|i| i.as_array()).cloned().unwrap_or_default();
    let mut out = Vec::new();
    for item in items {
        let raw_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
        let Ok(e) = parse_crossref(&serde_json::json!({ "message": item }).to_string()) else { continue };
        let licence = item.get("license").and_then(|l| l.as_array()).and_then(|ls| {
            ls.iter().filter_map(|l| l.get("URL").and_then(|u| u.as_str())).find(|u| u.contains("creativecommons.org"))
        });
        let open_access = licence.map(|l| {
            let doi = e.get("doi").unwrap_or_default();
            OpenAccess { url: format!("https://doi.org/{doi}"), pdf: None, note: format!("Open licence ({})", cc_name(l)) }
        });
        out.push(Hit {
            source: "Crossref".into(),
            kind: crossref_kind(&raw_type).into(),
            entry: e,
            score: 0.0,
            open_access,
            excluded: (raw_type == "peer-review" || raw_type == "component").then(|| "not a publication (peer review or component)".to_string()),
            also_in: vec![],
        });
        // keep the raw type for the review check
        out.last_mut().unwrap().entry.set("_type", &raw_type);
    }
    Ok(out)
}

fn cc_name(url: &str) -> String {
    let u = url.to_lowercase();
    if u.contains("publicdomain") {
        return "public domain".into();
    }
    let parts: Vec<&str> = u.split('/').filter(|s| !s.is_empty()).collect();
    match parts.iter().position(|p| *p == "licenses") {
        Some(i) if i + 1 < parts.len() => format!("CC {}", parts[i + 1].to_uppercase()),
        _ => "Creative Commons".into(),
    }
}

fn crossref_kind(t: &str) -> &'static str {
    match t {
        "journal-article" => "Journal article",
        "book-chapter" | "book-part" | "book-section" => "Chapter",
        "book" | "monograph" | "edited-book" | "reference-book" | "book-set" => "Book",
        "proceedings-article" => "Conference paper",
        "reference-entry" => "Reference entry",
        "dissertation" => "Thesis",
        "posted-content" => "Preprint",
        _ => "Other",
    }
}

const OA_SELECT: &str = "id,doi,display_name,publication_year,type,authorships,primary_location,open_access,best_oa_location,biblio";

fn openalex_url(q: &Query, filter: &str) -> String {
    let mut url = format!("https://api.openalex.org/works?per-page=12&select={OA_SELECT}");
    if !filter.is_empty() {
        url.push_str(&format!("&filter={filter}"));
    }
    if !q.openalex_key.trim().is_empty() {
        url.push_str(&format!("&api_key={}", enc(q.openalex_key.trim())));
    }
    url
}

fn openalex(q: &Query) -> Result<Vec<Hit>, String> {
    let words = search_words(&q.title, 14).replace(['\'', '’'], "");
    if words.is_empty() {
        return Ok(vec![]);
    }
    let types = match q.kind {
        Kind::Book => "type:book",
        Kind::Article => "type:article|review|letter|editorial",
        Kind::Chapter => "type:book-chapter",
        Kind::Any => "",
    };
    let url = if q.authors.is_empty() {
        // free text may include the author: search everything
        let mut u = openalex_url(q, types);
        u.push_str(&format!("&search={}", enc(&words)));
        u
    } else {
        let mut filter = format!("title.search:{}", enc(&words));
        if !types.is_empty() {
            filter = format!("{filter},{types}");
        }
        openalex_url(q, &filter)
    };
    parse_openalex(&openalex_get(&url)?)
}

fn openalex_get(url: &str) -> Result<String, String> {
    get(url, "application/json").map_err(|e| {
        if e.contains("429") {
            "daily allowance used up (an OpenAlex key in Preferences raises it)".to_string()
        } else {
            e
        }
    })
}

pub fn parse_openalex(body: &str) -> Result<Vec<Hit>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    let mut out = Vec::new();
    for w in v.get("results").and_then(|r| r.as_array()).cloned().unwrap_or_default() {
        let ty = w.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let source_name = w.pointer("/primary_location/source/display_name").and_then(|s| s.as_str()).unwrap_or("");
        let source_type = w.pointer("/primary_location/source/type").and_then(|s| s.as_str()).unwrap_or("");
        let bty = match ty {
            "article" | "review" | "letter" | "editorial" | "erratum" if source_type != "repository" => "Article",
            "book-chapter" | "reference-entry" => "InCollection",
            "book" | "monograph" | "edited-book" => "Book",
            "dissertation" => "Thesis",
            "preprint" => "Online",
            _ => "Misc",
        };
        let mut e = Entry::new(bty, "");
        let authors: Vec<String> = w
            .get("authorships")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.pointer("/author/display_name").and_then(|n| n.as_str())).map(|s| tex(&person(s))).collect())
            .unwrap_or_default();
        if !authors.is_empty() {
            set(&mut e, "author", &authors.join(" and "));
        }
        let Some(title) = w.get("display_name").and_then(|t| t.as_str()) else { continue };
        let title = title.replace("<i>", "").replace("</i>", "");
        match title.split_once(": ") {
            Some((m, s)) if bty == "Book" => {
                set(&mut e, "title", &tex(m));
                set(&mut e, "subtitle", &tex(s));
            }
            _ => set(&mut e, "title", &tex(&title)),
        }
        if let Some(y) = w.get("publication_year").and_then(|y| y.as_i64()) {
            set(&mut e, "date", &y.to_string());
        }
        if !source_name.is_empty() {
            match bty {
                "Article" => set(&mut e, "journaltitle", &tex(source_name)),
                "InCollection" => set(&mut e, "booktitle", &tex(source_name)),
                _ => {}
            }
        }
        if let Some(b) = w.get("biblio") {
            let s = |k: &str| b.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
            if let Some(v) = s("volume") {
                set(&mut e, "volume", &v);
            }
            if let Some(v) = s("issue") {
                set(&mut e, "number", &v);
            }
            match (s("first_page"), s("last_page")) {
                (Some(a), Some(z)) if a != z => set(&mut e, "pages", &format!("{a}--{z}")),
                (Some(a), _) => set(&mut e, "pages", &a),
                _ => {}
            }
        }
        if let Some(d) = w.get("doi").and_then(|d| d.as_str()) {
            set(&mut e, "doi", d.trim_start_matches("https://doi.org/"));
        }
        e.set("_type", ty);
        out.push(Hit {
            source: "OpenAlex".into(),
            kind: openalex_kind(ty).into(),
            entry: e,
            score: 0.0,
            open_access: openalex_oa(&w),
            excluded: None,
            also_in: vec![],
        });
    }
    Ok(out)
}

fn openalex_kind(t: &str) -> &'static str {
    match t {
        "article" => "Journal article",
        "review" => "Review article",
        "book-chapter" => "Chapter",
        "book" => "Book",
        "dissertation" => "Thesis",
        "preprint" => "Preprint",
        _ => "Other",
    }
}

fn openalex_oa(w: &Value) -> Option<OpenAccess> {
    let is_oa = w.pointer("/open_access/is_oa").and_then(|b| b.as_bool()).unwrap_or(false);
    if !is_oa {
        return None;
    }
    let status = w.pointer("/open_access/oa_status").and_then(|s| s.as_str()).unwrap_or("");
    let best = w.get("best_oa_location").filter(|b| !b.is_null());
    let s = |k: &str| best.and_then(|b| b.get(k)).and_then(|x| x.as_str()).map(|x| x.to_string());
    let pdf = s("pdf_url");
    let url = s("landing_page_url").or_else(|| pdf.clone()).or_else(|| w.pointer("/open_access/oa_url").and_then(|u| u.as_str()).map(|u| u.to_string()))?;
    let mut note = vec![if status.is_empty() { "Open access".to_string() } else { format!("Open access ({status})") }];
    match s("version").as_deref() {
        Some("publishedVersion") => note.push("published version".into()),
        Some("acceptedVersion") => note.push("accepted manuscript".into()),
        Some("submittedVersion") => note.push("preprint".into()),
        _ => {}
    }
    if let Some(host) = best.and_then(|b| b.pointer("/source/display_name")).and_then(|x| x.as_str()) {
        note.push(host.to_string());
    }
    if let Some(l) = s("license") {
        note.push(l.to_uppercase().replace("CC-", "CC "));
    }
    Some(OpenAccess { url, pdf, note: note.join(" · ") })
}

/// Open-access information for DOIs found elsewhere (one OpenAlex request).
fn openalex_oa_for(dois: &[String], key: &str) -> Result<Vec<(String, OpenAccess)>, String> {
    if dois.is_empty() {
        return Ok(vec![]);
    }
    let list = dois.iter().take(40).map(|d| enc(&d.to_lowercase())).collect::<Vec<_>>().join("|");
    let mut url = format!("https://api.openalex.org/works?per-page=50&select=doi,open_access,best_oa_location&filter=doi:{list}");
    if !key.trim().is_empty() {
        url.push_str(&format!("&api_key={}", enc(key.trim())));
    }
    let v: Value = serde_json::from_str(&openalex_get(&url)?).map_err(|e| format!("bad response: {e}"))?;
    Ok(v.get("results")
        .and_then(|r| r.as_array())
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|w| {
            let doi = w.get("doi")?.as_str()?.trim_start_matches("https://doi.org/").to_lowercase();
            Some((doi, openalex_oa(w)?))
        })
        .collect())
}

fn k10plus(q: &Query) -> Result<Vec<Hit>, String> {
    let words: Vec<String> = tokens(&q.title).into_iter().take(8).collect();
    if words.is_empty() {
        return Ok(vec![]);
    }
    // with no separate author the words may include one: search all fields
    let index = if q.authors.is_empty() { "pica.all" } else { "pica.tit" };
    let mut cql = format!("{index}=\"{}\"", words.join(" "));
    if let Some(a) = q.authors.first() {
        cql.push_str(&format!(" and pica.per={}", fold(a).replace(' ', "")));
    }
    let url = format!(
        "https://sru.k10plus.de/opac-de-627?version=1.1&operation=searchRetrieve&query={}&maximumRecords=10&recordSchema=marcxml",
        enc(&cql)
    );
    parse_k10plus(&get(&url, "application/xml")?)
}

pub fn parse_k10plus(xml: &str) -> Result<Vec<Hit>, String> {
    let mut out = Vec::new();
    for (ppn, fields) in marc_records(xml)? {
        // 773 = part of a larger work: an article (often a review) in a journal
        let part_of = fields.iter().any(|f| f.tag == "773");
        let genre_review = fields.iter().any(|f| f.tag == "655" && f.sub('a').is_some_and(|a| a.to_lowercase().contains("rezension")));
        let online = fields.iter().any(|f| f.tag == "338" && f.sub('b') == Some("cr"));
        let Some(mut e) = marc_entry(ppn, &fields) else { continue };
        if part_of {
            e.set_type("Article");
        }
        let url = fields
            .iter()
            .filter(|f| f.tag == "856")
            .filter_map(|f| f.sub('u'))
            .find(|u| u.starts_with("http") && !u.contains("d-nb.info") && !u.contains("/toc") && !u.contains("cover"))
            .map(|u| u.to_string());
        let free = fields.iter().any(|f| f.tag == "856" && f.sub('z').is_some_and(|z| z.to_lowercase().contains("kostenfrei")))
            || fields.iter().any(|f| f.tag == "506" && f.sub('f').is_some_and(|z| z.to_lowercase().contains("unrestricted")));
        e.set("_type", if part_of { "article" } else { "book" });
        if let Some(c) = fields.iter().find(|f| f.tag == "245").and_then(|f| f.sub('c')) {
            e.set("_resp", c);
        }
        out.push(Hit {
            source: "K10plus/GVK".into(),
            kind: if part_of { "Article (catalogue)".into() } else if online { "Book (online)".into() } else { "Book".into() },
            entry: e,
            score: 0.0,
            open_access: if free { url.map(|u| OpenAccess { url: u, pdf: None, note: "Free online copy (catalogue link)".into() }) } else { None },
            excluded: genre_review.then(|| "catalogued as a review".to_string()),
            also_in: vec![],
        });
    }
    Ok(out)
}

fn google_books(q: &Query) -> Result<Vec<Hit>, String> {
    let words = search_words(&q.title, 10);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let mut query = if q.authors.is_empty() { words.clone() } else { format!("intitle:{}", words) };
    if let Some(a) = q.authors.first() {
        query.push_str(&format!(" inauthor:{a}"));
    }
    let url = format!("https://www.googleapis.com/books/v1/volumes?printType=books&maxResults=12&q={}", q_enc(&query));
    parse_google_search(&get(&url, "application/json")?)
}

pub fn parse_google_search(body: &str) -> Result<Vec<Hit>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let mut out = Vec::new();
    for item in v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default() {
        let Some(vi) = item.get("volumeInfo") else { continue };
        let mut e = Entry::new("Book", "");
        let authors: Vec<String> = vi
            .get("authors")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str()).map(|s| tex(&person(s))).collect())
            .unwrap_or_default();
        if !authors.is_empty() {
            set(&mut e, "author", &authors.join(" and "));
        }
        let Some(t) = str_at(vi, &["title"]) else { continue };
        set(&mut e, "title", &tex(t));
        if let Some(t) = str_at(vi, &["subtitle"]) {
            set(&mut e, "subtitle", &tex(t));
        }
        if let Some(p) = str_at(vi, &["publisher"]) {
            set(&mut e, "publisher", &tex(p));
        }
        if let Some(d) = str_at(vi, &["publishedDate"]).and_then(year_in) {
            set(&mut e, "date", &d);
        }
        if let Some(n) = vi.get("pageCount").and_then(|n| n.as_i64()) {
            set(&mut e, "pagetotal", &n.to_string());
        }
        if let Some(ids) = vi.get("industryIdentifiers").and_then(|a| a.as_array()) {
            let pick = |ty: &str| ids.iter().find(|x| x.get("type").and_then(|t| t.as_str()) == Some(ty)).and_then(|x| x.get("identifier")).and_then(|x| x.as_str());
            if let Some(i) = pick("ISBN_13").or_else(|| pick("ISBN_10")) {
                set(&mut e, "isbn", i);
            }
        }
        if let Some(l) = str_at(vi, &["language"]).and_then(langid) {
            set(&mut e, "langid", l);
        }
        let access = item.get("accessInfo");
        let public = access.and_then(|a| a.get("accessViewStatus")).and_then(|s| s.as_str()) == Some("FULL_PUBLIC_DOMAIN")
            || access.and_then(|a| a.get("viewability")).and_then(|s| s.as_str()) == Some("ALL_PAGES") && access.and_then(|a| a.get("publicDomain")).and_then(|b| b.as_bool()) == Some(true);
        let open_access = public.then(|| {
            let link = access.and_then(|a| a.get("webReaderLink")).and_then(|s| s.as_str()).or_else(|| str_at(vi, &["previewLink"])).unwrap_or("").to_string();
            let pdf = access.and_then(|a| a.pointer("/pdf/downloadLink")).and_then(|s| s.as_str()).map(|s| s.to_string());
            OpenAccess { url: link, pdf, note: "Full view (public domain) · Google Books".into() }
        });
        e.set("_type", "book");
        out.push(Hit { source: "Google Books".into(), kind: "Book".into(), entry: e, score: 0.0, open_access, excluded: None, also_in: vec![] });
    }
    Ok(out)
}

fn open_library(q: &Query) -> Result<Vec<Hit>, String> {
    let words = search_words(&q.title, 12);
    if words.is_empty() {
        return Ok(vec![]);
    }
    let mut url = format!(
        "https://openlibrary.org/search.json?limit=8&fields=key,title,subtitle,author_name,first_publish_year,publish_year,isbn,publisher,publish_place,ebook_access,ia,language&{}={}",
        if q.authors.is_empty() { "q" } else { "title" },
        q_enc(&words)
    );
    if let Some(a) = q.authors.first() {
        url.push_str(&format!("&author={}", q_enc(a)));
    }
    parse_open_library_search(&get(&url, "application/json")?, q.year)
}

pub fn parse_open_library_search(body: &str, want_year: Option<i32>) -> Result<Vec<Hit>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let mut out = Vec::new();
    for d in v.get("docs").and_then(|d| d.as_array()).cloned().unwrap_or_default() {
        let Some(t) = d.get("title").and_then(|t| t.as_str()) else { continue };
        let mut e = Entry::new("Book", "");
        set(&mut e, "title", &tex(t));
        if let Some(s) = d.get("subtitle").and_then(|t| t.as_str()) {
            set(&mut e, "subtitle", &tex(s));
        }
        let list = |k: &str| -> Vec<String> { d.get(k).and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default() };
        let authors: Vec<String> = list("author_name").iter().map(|s| tex(&person(s))).collect();
        if !authors.is_empty() {
            set(&mut e, "author", &authors.join(" and "));
        }
        let years: Vec<i64> = d.get("publish_year").and_then(|a| a.as_array()).map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
        let year = want_year.map(|y| y as i64).filter(|y| years.contains(y)).or_else(|| d.get("first_publish_year").and_then(|y| y.as_i64()));
        if let Some(y) = year {
            set(&mut e, "date", &y.to_string());
        }
        // a work with many editions has many ISBNs and publishers; only one edition's are safe to take
        let isbn13: HashSet<String> = list("isbn").iter().filter_map(|i| to_isbn13(i)).collect();
        let publishers = list("publisher");
        if publishers.len() == 1 && isbn13.len() <= 1 {
            set(&mut e, "publisher", &tex(&publishers[0]));
        }
        if isbn13.len() == 1 {
            set(&mut e, "isbn", isbn13.iter().next().unwrap());
        }
        let ia = list("ia");
        let access = d.get("ebook_access").and_then(|a| a.as_str()).unwrap_or("");
        let open_access = (access == "public").then(|| ia.first().cloned()).flatten().map(|id| OpenAccess {
            url: format!("https://archive.org/details/{id}"),
            pdf: Some(format!("https://archive.org/download/{id}/{id}.pdf")),
            note: "Public scan · Internet Archive".into(),
        });
        e.set("_type", "book");
        let editions = if isbn13.len() > 1 { format!(" ({} ISBNs for different editions)", isbn13.len()) } else { String::new() };
        out.push(Hit {
            source: "Open Library".into(),
            kind: format!("Book{editions}"),
            entry: e,
            score: 0.0,
            open_access,
            excluded: None,
            also_in: vec![],
        });
    }
    Ok(out)
}

fn to_isbn13(i: &str) -> Option<String> {
    let d: String = i.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
    match d.len() {
        13 => Some(d),
        10 => {
            let core = format!("978{}", &d[..9]);
            let sum: u32 = core.chars().enumerate().map(|(k, c)| c.to_digit(10).unwrap() * if k % 2 == 0 { 1 } else { 3 }).sum();
            Some(format!("{core}{}", (10 - sum % 10) % 10))
        }
        _ => None,
    }
}

// ---------- putting it together ----------

/// Search all sources at once. Blocking. Returns hits (best first, set-aside
/// ones last) and per-source error messages.
pub fn search(q: &Query) -> (Vec<Hit>, Vec<String>) {
    type Job = (&'static str, fn(&Query) -> Result<Vec<Hit>, String>);
    let mut jobs: Vec<Job> = vec![("Crossref", crossref), ("OpenAlex", openalex)];
    if matches!(q.kind, Kind::Book | Kind::Any) {
        jobs.push(("K10plus/GVK", k10plus));
        jobs.push(("Google Books", google_books));
        jobs.push(("Open Library", open_library));
    }
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(name, f)| {
            let q = q.clone();
            (name, std::thread::spawn(move || f(&q)))
        })
        .collect();
    let mut hits = Vec::new();
    let mut errors = Vec::new();
    for (name, h) in handles {
        match h.join() {
            Ok(Ok(v)) => hits.extend(v),
            Ok(Err(e)) => errors.push(format!("{name}: {e}")),
            Err(_) => errors.push(format!("{name}: internal error")),
        }
    }
    let mut hits = rank(q, hits);
    archive_details(&mut hits);
    // open-access information for good DOI matches that lack it
    let need: Vec<String> = hits.iter().filter(|h| h.excluded.is_none() && h.open_access.is_none() && h.score >= POSSIBLE).filter_map(|h| h.doi()).collect();
    if !need.is_empty() && !errors.iter().any(|e| e.starts_with("OpenAlex")) {
        match openalex_oa_for(&need, &q.openalex_key) {
            Ok(found) => {
                for (doi, oa) in found {
                    if let Some(h) = hits.iter_mut().find(|h| h.doi().is_some_and(|d| d.to_lowercase() == doi)) {
                        h.open_access = Some(oa);
                    }
                }
                sort(&mut hits);
            }
            Err(e) => errors.push(format!("OpenAlex: {e}")),
        }
    }
    (hits, errors)
}

/// For scans at the Internet Archive: their ARK (for the eprint field), the
/// real PDF file name, and the publisher and date of the scanned copy.
fn archive_details(hits: &mut [Hit]) {
    let jobs: Vec<(usize, String)> = hits
        .iter()
        .enumerate()
        .filter(|(_, h)| h.excluded.is_none() && h.score >= POSSIBLE)
        .filter_map(|(i, h)| h.open_access.as_ref()?.url.strip_prefix("https://archive.org/details/").map(|id| (i, id.to_string())))
        .take(4)
        .collect();
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|(i, id)| (i, std::thread::spawn(move || get(&format!("https://archive.org/metadata/{}", enc(&id)), "application/json").ok().map(|b| (id, b)))))
        .collect();
    for (i, h) in handles {
        let Some((id, body)) = h.join().ok().flatten() else { continue };
        apply_archive_metadata(&mut hits[i], &id, &body);
    }
}

pub fn apply_archive_metadata(h: &mut Hit, id: &str, body: &str) {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return };
    let m = |k: &str| v.pointer(&format!("/metadata/{k}")).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(ark) = m("identifier-ark") {
        h.entry.set("eprint", &ark);
        h.entry.set("eprinttype", "ark");
    }
    if let Some(p) = m("publisher") {
        // "Leipzig, Breitkopf and Härtel; [etc., etc.]" → location + publisher
        let p = p.split(';').next().unwrap_or(&p).trim().to_string();
        match p.split_once(" : ").or_else(|| p.split_once(" :")).or_else(|| p.split_once(", ")) {
            Some((place, publ)) if !place.contains(' ') || place.len() < 20 => {
                h.entry.set("location", &tex(place.trim()));
                h.entry.set("publisher", &tex(publ.trim()));
            }
            _ => {
                h.entry.set("publisher", &tex(&p));
            }
        }
    }
    if let Some(y) = m("date").and_then(|d| first_year(&d)) {
        h.entry.set("date", &y.to_string());
    }
    let pdf = v.get("files").and_then(|f| f.as_array()).and_then(|files| {
        files
            .iter()
            .filter(|f| f.get("name").and_then(|n| n.as_str()).is_some_and(|n| n.to_lowercase().ends_with(".pdf")))
            .max_by_key(|f| f.get("format").and_then(|x| x.as_str()) == Some("Text PDF"))
            .and_then(|f| f.get("name").and_then(|n| n.as_str()))
            .map(|n| format!("https://archive.org/download/{id}/{}", enc(n)))
    });
    if let Some(oa) = h.open_access.as_mut() {
        oa.pdf = pdf;
        oa.note = "Public scan · Internet Archive".into();
    }
}

/// Score, set aside reviews etc., merge duplicates and sort.
pub fn rank(q: &Query, hits: Vec<Hit>) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    for mut h in hits {
        let raw = h.entry.get("_type").unwrap_or_default();
        h.entry.remove("_type");
        h.score = score(q, &h.entry);
        if h.excluded.is_none() {
            h.excluded = exclusion(q, &h.entry, &raw);
        }
        h.entry.remove("_resp");
        // nothing usable: no identifier and no free copy (still shown, so
        // that one can see the work exists but has no DOI or ISBN)
        if h.doi().is_none() && h.isbn().is_none() && h.open_access.is_none() && h.excluded.is_none() {
            h.excluded = Some("no DOI or ISBN in this record".into());
        }
        if h.score < 0.4 {
            continue;
        }
        let lower = |d: Option<String>| d.map(|d| d.to_lowercase());
        let same = out.iter_mut().find(|o| {
            let (od, hd) = (lower(o.doi()), lower(h.doi()));
            (od.is_some() && od == hd)
                // chapters carry their book's ISBN: only whole books are merged by ISBN
                || ((od.is_none() || hd.is_none())
                    && o.kind.starts_with("Book")
                    && h.kind.starts_with("Book")
                    && o.isbn().is_some()
                    && o.isbn().and_then(|i| to_isbn13(&i)) == h.isbn().and_then(|i| to_isbn13(&i)))
        });
        match same {
            Some(o) => {
                // one result carrying both identifiers
                if o.doi().is_none() {
                    if let Some(d) = h.doi() {
                        o.entry.set("doi", &d);
                    }
                }
                if o.isbn().is_none() {
                    if let Some(i) = h.isbn() {
                        o.entry.set("isbn", &i);
                    }
                }
                if !o.also_in.contains(&h.source) && o.source != h.source {
                    o.also_in.push(h.source.clone());
                }
                if o.open_access.is_none() {
                    o.open_access = h.open_access;
                } else if o.open_access.as_ref().is_some_and(|x| x.pdf.is_none()) && h.open_access.as_ref().is_some_and(|x| x.pdf.is_some()) {
                    o.open_access = h.open_access;
                }
                if o.excluded.is_none() && h.excluded.is_some() {
                    o.excluded = h.excluded;
                }
                o.score = o.score.max(h.score);
            }
            None => out.push(h),
        }
    }
    sort(&mut out);
    out
}

/// Best matches first; among equally good matches, open-access copies first
/// (with a PDF before a landing page).
fn sort(hits: &mut [Hit]) {
    hits.sort_by(|a, b| {
        let oa = |h: &Hit| match &h.open_access {
            Some(o) if o.pdf.is_some() => 2,
            Some(_) => 1,
            None => 0,
        };
        let doi = |h: &Hit| h.doi().is_some() as u8;
        let print = |h: &Hit| !h.kind.contains("online") as u8;
        b.tier()
            .cmp(&a.tier())
            .then(oa(b).cmp(&oa(a)))
            .then(b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal))
            .then(doi(b).cmp(&doi(a)))
            .then(print(b).cmp(&print(a)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(title: &str, author: &str, year: i32, kind: Kind) -> Query {
        Query { title: title.into(), authors: vec![author.into()], year: Some(year), container: String::new(), kind, openalex_key: String::new() }
    }

    #[test]
    fn titles() {
        assert!(title_score("Manu's Code of Law", "Manu’s Code Of Law") > 0.99);
        assert!(title_score("Manu's Code of Law: A Critical Edition and Translation of the Mānava-Dharmaśāstra", "Manu’s Code Of Law") >= 0.9);
        assert!(title_score("A Dictionary of Sanskrit Grammar", "Dictionary of Sanskrit grammar") > 0.99);
        assert!(title_score("Manu's Code of Law", "Manu and the Arthaśāstra, A Study in Śāstric Intertextuality") < 0.5);
        assert!(title_score("A Sanskrit Grammar", "The history of Sanskrit grammar") < 0.85);
        assert!(title_score("book of women's rituals Central Asian", "The book of women's rituals: the Central Asian adaptation") >= 0.85);
        assert_eq!(first_year("2004-12-09"), Some(2004));
        assert_eq!(first_year("c. 1986"), Some(1986));
    }

    const CR_BOOKS: &str = r#"{"message":{"items":[
      {"type":"book","title":["Manu’s Code Of Law"],"author":[{"given":"Patrick","family":"Olivelle"}],"issued":{"date-parts":[[2004,12,9]]},"DOI":"10.1093/oso/9780195171464.001.0001","ISBN":["9780195171464"],"publisher":"Oxford University Press"},
      {"type":"monograph","title":["King, Governance, and Law in Ancient India"],"author":[{"given":"Patrick","family":"Olivelle"}],"issued":{"date-parts":[[2013,1,31]]},"DOI":"10.1093/acprof:osobl/9780199891825.001.0001"}]}}"#;

    const CR_ARTICLES: &str = r#"{"message":{"items":[
      {"type":"journal-article","title":["MANU'S CODE OF LAW: A CRITICAL EDITION AND TRANSLATION OF THE MANAVA‐DHARMASASTRA – By Patrick Olivelle THE LAW CODE OF MANU – By Patrick Olivelle"],"container-title":["Religious Studies Review"],"author":[{"given":"Frederick M.","family":"Smith"}],"issued":{"date-parts":[[2008,5,27]]},"page":"125-125","DOI":"10.1111/j.1748-0922.2008.00281_6.x"},
      {"type":"journal-article","title":["Manu's Code of Law: A Critical Edition and Translation of the Mānava-Dharmaśāstra. By Patrick Olivelle. Oxford and New York: Oxford University Press, 2005. x, 1131 pp. $99.00 (cloth)."],"container-title":["The Journal of Asian Studies"],"author":[{"given":"Mikael","family":"Aktor"}],"issued":{"date-parts":[[2006,11]]},"page":"850-852","DOI":"10.1017/s0021911806001926"},
      {"type":"journal-article","title":["Manu’s Code of Law"],"container-title":["Journal of the American Oriental Society"],"author":[{"given":"Ludo","family":"Rocher"}],"issued":{"date-parts":[[2006]]},"DOI":"10.2307/20064510"},
      {"type":"book-chapter","title":["Chapter Nine"],"container-title":["Manu’s Code Of Law"],"author":[{"given":"Patrick","family":"Olivelle"}],"issued":{"date-parts":[[2004,12,9]]},"DOI":"10.1093/oso/9780195171464.003.0014"},
      {"type":"journal-article","title":["Manu and the Arthaśāstra, A Study in Śāstric Intertextuality"],"container-title":["Journal of Indian Philosophy"],"author":[{"given":"Patrick","family":"Olivelle"}],"issued":{"date-parts":[[2004,6]]},"DOI":"10.1023/b:indi.0000021078.31452.8a","license":[{"URL":"http://creativecommons.org/licenses/by/4.0/"}]}]}}"#;

    #[test]
    fn book_search_never_offers_reviews() {
        let want = q("Manu's Code of Law", "Olivelle", 2005, Kind::Book);
        let mut hits = parse_crossref_search(CR_BOOKS).unwrap();
        // even if journal articles turn up (e.g. from another source) they are set aside
        hits.extend(parse_crossref_search(CR_ARTICLES).unwrap());
        let ranked = rank(&want, hits);
        let best = &ranked[0];
        assert_eq!(best.doi().unwrap(), "10.1093/oso/9780195171464.001.0001");
        assert_eq!(best.strength(), "Strong match");
        for h in &ranked {
            if h.doi().is_some_and(|d| d.starts_with("10.1111") || d.starts_with("10.1017") || d.starts_with("10.2307")) {
                assert!(h.excluded.is_some(), "review not set aside: {:?}", h.entry.get("title"));
            }
        }
        assert!(ranked.iter().all(|h| h.excluded.is_some() || h.kind == "Book"));
    }

    #[test]
    fn reviews_recognised_in_article_search() {
        let want = q("Manu's Code of Law", "Olivelle", 2005, Kind::Any);
        let ranked = rank(&want, parse_crossref_search(CR_ARTICLES).unwrap());
        let get = |p: &str| ranked.iter().find(|h| h.doi().is_some_and(|d| d.starts_with(p)));
        assert!(get("10.1111").unwrap().excluded.is_some()); // "– By Patrick Olivelle"
        assert!(get("10.1017").unwrap().excluded.is_some()); // pp., $99.00
        // same title, different author: offered only as a weak match
        assert!(get("10.2307").unwrap().excluded.is_some());
        // the chapter "Chapter Nine" does not match the title at all
        assert!(get("10.1093").is_none());
    }

    #[test]
    fn article_with_open_licence() {
        let want = q("Manu and the Arthaśāstra", "Olivelle", 2004, Kind::Article);
        let ranked = rank(&want, parse_crossref_search(CR_ARTICLES).unwrap());
        let h = &ranked[0];
        assert_eq!(h.doi().unwrap(), "10.1023/b:indi.0000021078.31452.8a");
        assert!(h.excluded.is_none());
        assert_eq!(h.open_access.as_ref().unwrap().note, "Open licence (CC BY)");
    }

    #[test]
    fn openalex_results() {
        let body = r#"{"meta":{"count":2},"results":[
          {"id":"https://openalex.org/W1","doi":"https://doi.org/10.1017/s1356186322000268","display_name":"The book of women’s rituals: the Central Asian adaptation of the <i>ʿAqāʾid al-nisāʾ</i>","publication_year":2022,"type":"article",
           "authorships":[{"author":{"display_name":"Aziza Shanazarova"}}],
           "primary_location":{"source":{"display_name":"Journal of the Royal Asiatic Society","type":"journal"}},
           "open_access":{"is_oa":true,"oa_status":"hybrid","oa_url":"https://www.cambridge.org/core/services/aop-cambridge-core/content/view/X"},
           "best_oa_location":{"pdf_url":"https://www.cambridge.org/core/services/aop-cambridge-core/content/view/X.pdf","landing_page_url":"https://doi.org/10.1017/s1356186322000268","version":"publishedVersion","license":"cc-by","source":{"display_name":"Journal of the Royal Asiatic Society"}},
           "biblio":{"volume":"33","issue":"2","first_page":"317","last_page":"333"}},
          {"id":"https://openalex.org/W2","doi":null,"display_name":"Women's rituals","publication_year":1990,"type":"article","authorships":[],"primary_location":null,"open_access":{"is_oa":false},"best_oa_location":null,"biblio":{}}]}"#;
        let want = q("The book of women's rituals: the Central Asian adaptation of the ʿAqāʾid al-nisāʾ", "Shanazarova", 2023, Kind::Article);
        let ranked = rank(&want, parse_openalex(body).unwrap());
        // the second has no DOI and no free copy
        assert_eq!(ranked.iter().filter(|h| h.excluded.is_none()).count(), 1);
        let h = &ranked[0];
        assert_eq!(h.doi().unwrap(), "10.1017/s1356186322000268");
        assert_eq!(h.entry.get("pages").unwrap(), "317--333");
        assert_eq!(h.entry.get("journaltitle").unwrap(), "Journal of the Royal Asiatic Society");
        let oa = h.open_access.as_ref().unwrap();
        assert!(oa.pdf.as_ref().unwrap().ends_with(".pdf"));
        assert_eq!(oa.note, "Open access (hybrid) · published version · Journal of the Royal Asiatic Society · CC BY");
        assert_eq!(h.strength(), "Strong match");
    }

    #[test]
    fn open_access_ranked_first_among_equals() {
        let mk = |doi: &str, oa: bool| Hit {
            source: "X".into(),
            entry: {
                let mut e = Entry::new("Article", "");
                e.set("title", "Sanskrit medicine");
                e.set("author", "Wujastyk, Dominik");
                e.set("date", "2003");
                e.set("doi", doi);
                e
            },
            kind: "Journal article".into(),
            score: 0.0,
            open_access: oa.then(|| OpenAccess { url: "u".into(), pdf: Some("p".into()), note: String::new() }),
            excluded: None,
            also_in: vec![],
        };
        let ranked = rank(&q("Sanskrit medicine", "Wujastyk", 2003, Kind::Article), vec![mk("10.1/a", false), mk("10.1/b", true)]);
        assert_eq!(ranked[0].doi().unwrap(), "10.1/b");
    }

    #[test]
    fn open_library_isbn_only_for_single_edition() {
        let body = r#"{"docs":[
          {"title":"A dictionary of Sanskrit grammar","author_name":["Kashinath Vasudev Abhyankar"],"first_publish_year":1961,"publish_year":[1961,1977,1986],"isbn":["8170800366","9788170800361","9780000000002"],"ebook_access":"borrowable","ia":["dictionaryofsans0000abhy"]},
          {"title":"Sanskrit grammar","author_name":["W. D. Whitney"],"first_publish_year":1879,"publish_year":[1879],"ebook_access":"public","ia":["sanskritgrammari00whit"]}]}"#;
        let v = parse_open_library_search(body, Some(1986)).unwrap();
        assert!(v[0].isbn().is_none());
        assert_eq!(v[0].entry.get("date").unwrap(), "1986");
        assert!(v[0].kind.contains("2 ISBNs"));
        assert!(v[0].open_access.is_none());
        let oa = v[1].open_access.as_ref().unwrap();
        assert_eq!(oa.url, "https://archive.org/details/sanskritgrammari00whit");
        assert_eq!(to_isbn13("8170800366").unwrap(), "9788170800361");
    }

    #[test]
    fn internet_archive_item() {
        let mut h = Hit {
            source: "Open Library".into(),
            entry: Entry::new("Book", ""),
            kind: "Book".into(),
            score: 1.0,
            open_access: Some(OpenAccess { url: "https://archive.org/details/x".into(), pdf: Some("guess".into()), note: String::new() }),
            excluded: None,
            also_in: vec![],
        };
        let body = r#"{"metadata":{"identifier-ark":"ark:/13960/t5gb20h2d","publisher":"Leipzig, Brietkopf and Härtel; [etc., etc.]","date":"[c1879.]"},
            "files":[{"name":"x_jp2.zip","format":"Single Page Processed JP2 ZIP"},{"name":"x_bw.pdf","format":"Additional Text PDF"},{"name":"x.pdf","format":"Text PDF"}]}"#;
        apply_archive_metadata(&mut h, "x", body);
        assert_eq!(h.entry.get("eprint").unwrap(), "ark:/13960/t5gb20h2d");
        assert_eq!(h.entry.get("location").unwrap(), "Leipzig");
        assert_eq!(h.entry.get("publisher").unwrap(), "Brietkopf and Härtel");
        assert_eq!(h.entry.get("date").unwrap(), "1879");
        assert_eq!(h.open_access.as_ref().unwrap().pdf.as_ref().unwrap(), "https://archive.org/download/x/x.pdf");
        apply_archive_metadata(&mut h, "x", r#"{"metadata":{"publisher":"Leipzig : Breitkopf & Härtel"}}"#);
        assert_eq!(h.entry.get("location").unwrap(), "Leipzig");
        assert_eq!(h.entry.get("publisher").unwrap(), "Breitkopf \\& Härtel");
    }

    #[test]
    fn chapter_not_merged_with_its_book() {
        let body = r#"{"message":{"items":[
          {"type":"book-chapter","title":["Sanskrit Index"],"container-title":["Sanskrit Grammar"],"author":[{"given":"William Dwight","family":"Whitney"}],"issued":{"date-parts":[[2025]]},"DOI":"10.1163/9789004752726_021","ISBN":["9789004752726"]},
          {"type":"book","title":["Sanskrit Grammar"],"author":[{"given":"William Dwight","family":"Whitney"}],"issued":{"date-parts":[[2024]]},"DOI":"10.1163/9789004752726","ISBN":["9789004752726"]}]}}"#;
        let ranked = rank(&q("Sanskrit Grammar", "Whitney", 2024, Kind::Any), parse_crossref_search(body).unwrap());
        assert_eq!(ranked[0].doi().unwrap(), "10.1163/9789004752726");
        assert!(ranked.iter().all(|h| h.doi().unwrap() != "10.1163/9789004752726_021" || h.score < POSSIBLE));
    }

    #[test]
    fn from_text() {
        let q = Query::from_text("Manu's Code of Law 2005");
        assert_eq!(q.title, "Manu's Code of Law");
        assert_eq!(q.year, Some(2005));
    }
}
