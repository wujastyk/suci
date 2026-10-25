//! Fetch bibliographic data by DOI or ISBN.
//!
//! DOI: Crossref, then doi.org content negotiation (DataCite, Zenodo, …).
//! ISBN: K10plus/GVK union catalogue (strong on Indological and European
//! scholarly books), Open Library and Google Books — queried in parallel so
//! the user can pick the best record.

use crate::bib::{Entry, Field, Library};
use crate::latex::unicode_to_latex;
use crate::names::parse_name;
use serde_json::Value;
use std::time::Duration;
use unicode_normalization::UnicodeNormalization;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ident {
    Doi(String),
    Isbn(String),
}

impl Ident {
    pub fn label(&self) -> String {
        match self {
            Ident::Doi(d) => format!("DOI {d}"),
            Ident::Isbn(i) => format!("ISBN {i}"),
        }
    }
}

/// Recognise a DOI (bare, doi:, or doi.org URL), a shortDOI (10/abcd) or an
/// ISBN in pasted text.
pub fn detect(text: &str) -> Option<Ident> {
    let t = text.trim();
    let lower = t.to_lowercase();
    let short = lower
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("dx.")
        .trim_start_matches("doi.org/")
        .trim_start_matches("doi:")
        .trim();
    if let Some(code) = short.strip_prefix("10/") {
        if !code.is_empty() && code.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Some(Ident::Doi(format!("10/{code}")));
        }
    }
    if let Some(i) = lower.find("10.") {
        let cand = &t[i..];
        let doi: String = cand.chars().take_while(|c| !c.is_whitespace()).collect();
        let doi = doi.trim_end_matches(['.', ',', ';', ')', ']']).to_string();
        if doi.contains('/') && doi.len() > 7 {
            let before = &lower[..i];
            if before.is_empty()
                || before.ends_with("doi.org/")
                || before.ends_with("doi:")
                || before.ends_with("doi: ")
                || before.ends_with("doi ")
                || before.ends_with('/')
            {
                return Some(Ident::Doi(doi));
            }
        }
    }
    let digits: String = t
        .trim_start_matches(|c: char| c.is_alphabetic() || c == ':' || c.is_whitespace())
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x')
        .collect();
    if (digits.len() == 10 || digits.len() == 13) && crate::checks::isbn_valid(&digits) {
        return Some(Ident::Isbn(digits.to_uppercase()));
    }
    None
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub source: String,
    pub entry: Entry,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(concat!("Suci/", env!("CARGO_PKG_VERSION"), " (bibliography manager; https://github.com)"))
        .build()
        .into()
}

fn http_error(e: ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(404) => "not found".to_string(),
        ureq::Error::StatusCode(c) => format!("server returned error {c}"),
        ureq::Error::Timeout(_) => "timed out".to_string(),
        other => format!("network error: {other}"),
    }
}

/// Download a file as bytes (no overall time limit, for large files). Blocking.
pub(crate) fn get_bytes(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .user_agent(concat!("Suci/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent.get(url).call().map_err(http_error)?;
    resp.body_mut().with_config().limit(limit).read_to_vec().map_err(|e| format!("download failed: {e}"))
}

pub(crate) fn get(url: &str, accept: &str) -> Result<String, String> {
    let mut resp = agent().get(url).header("Accept", accept).call().map_err(|e| match e {
        ureq::Error::StatusCode(404) => "not found".to_string(),
        ureq::Error::StatusCode(c) => format!("server returned error {c}"),
        ureq::Error::Timeout(_) => "timed out".to_string(),
        other => format!("network error: {other}"),
    })?;
    resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_string().map_err(|e| format!("could not read response: {e}"))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn is_short_doi(doi: &str) -> bool {
    doi.trim().to_lowercase().starts_with("10/")
}

/// Contract a DOI with the shortDOI service (https://shortdoi.org).
/// Blocking. Returns e.g. "10/hbph7k".
pub fn shorten_doi(doi: &str) -> Result<String, String> {
    let doi = crate::checks::clean_doi(doi);
    if is_short_doi(&doi) {
        return Ok(doi);
    }
    let body = get(&format!("https://shortdoi.org/{}?format=json", enc(&doi)), "application/json")?;
    let v: Value = serde_json::from_str(&body).map_err(|_| "shortdoi.org did not return a short DOI (is the DOI registered?)".to_string())?;
    v.get("ShortDOI")
        .and_then(|s| s.as_str())
        .filter(|s| s.starts_with("10/"))
        .map(|s| s.to_string())
        .ok_or_else(|| "shortdoi.org did not return a short DOI".into())
}

/// Expand a shortDOI to the full DOI, using the DOI handle API.
pub fn expand_short_doi(short: &str) -> Result<String, String> {
    let body = get(&format!("https://doi.org/api/handles/{}", short.trim()), "application/json")?;
    parse_handle_alias(&body).ok_or_else(|| format!("could not expand the short DOI {short}"))
}

fn parse_handle_alias(body: &str) -> Option<String> {
    let v: Value = serde_json::from_str(body).ok()?;
    v.get("values")?.as_array()?.iter().find_map(|x| {
        (x.get("type")?.as_str()? == "HS_ALIAS").then(|| x.get("data")?.get("value")?.as_str().map(|s| s.to_string()))?
    })
}

/// Fetch all candidates for an identifier. Blocking; call from a worker thread.
pub fn fetch(id: &Ident) -> (Vec<Candidate>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    // a shortDOI is looked up by its full form, and kept as given
    let mut short_given = None;
    let expanded;
    let id = match id {
        Ident::Doi(d) if is_short_doi(d) => match expand_short_doi(d) {
            Ok(full) => {
                short_given = Some(d.clone());
                expanded = Ident::Doi(full);
                &expanded
            }
            Err(e) => return (vec![], vec![format!("doi.org: {e}")]),
        },
        other => other,
    };
    let (mut found_inner, errs) = fetch_full(id);
    if let Some(s) = short_given {
        for c in found_inner.iter_mut() {
            c.entry.set("doi", &s);
            c.entry.raw = None;
        }
    }
    found.append(&mut found_inner);
    errors.extend(errs);
    (found, errors)
}

fn fetch_full(id: &Ident) -> (Vec<Candidate>, Vec<String>) {
    let mut found = Vec::new();
    let mut errors = Vec::new();
    match id {
        Ident::Doi(doi) => {
            match get(&format!("https://api.crossref.org/works/{}", enc(doi)), "application/json")
                .and_then(|b| parse_crossref(&b))
            {
                Ok(e) => found.push(Candidate { source: "Crossref".into(), entry: e }),
                Err(err) => {
                    errors.push(format!("Crossref: {err}"));
                    match get(&format!("https://doi.org/{}", enc(doi)), "application/x-bibtex; charset=utf-8")
                        .and_then(|b| parse_bibtex_response(&b))
                    {
                        Ok(mut e) => {
                            if e.get_nonempty("doi").is_none() {
                                e.set("doi", doi);
                            }
                            found.push(Candidate { source: "doi.org".into(), entry: e })
                        }
                        Err(err) => errors.push(format!("doi.org: {err}")),
                    }
                }
            }
        }
        Ident::Isbn(isbn) => {
            let isbn = isbn.clone();
            let i1 = isbn.clone();
            let i2 = isbn.clone();
            let i3 = isbn.clone();
            let h1 = std::thread::spawn(move || {
                get(
                    &format!(
                        "https://sru.k10plus.de/opac-de-627?version=1.1&operation=searchRetrieve&query=pica.isb%3D{i1}&maximumRecords=3&recordSchema=marcxml"
                    ),
                    "application/xml",
                )
                .and_then(|b| parse_marcxml(&b))
                .map(|v| ("K10plus/GVK", v))
            });
            let h2 = std::thread::spawn(move || {
                get(&format!("https://openlibrary.org/api/books?bibkeys=ISBN:{i2}&format=json&jscmd=data"), "application/json")
                    .and_then(|b| parse_openlibrary(&b))
                    .map(|v| ("Open Library", v))
            });
            let h3 = std::thread::spawn(move || {
                get(&format!("https://www.googleapis.com/books/v1/volumes?q=isbn:{i3}"), "application/json")
                    .and_then(|b| parse_google_books(&b))
                    .map(|v| ("Google Books", v))
            });
            for (name, h) in [("K10plus/GVK", h1), ("Open Library", h2), ("Google Books", h3)] {
                match h.join() {
                    Ok(Ok((src, list))) => {
                        if list.is_empty() {
                            errors.push(format!("{src}: no record"));
                        }
                        for mut e in list {
                            if e.get_nonempty("isbn").is_none() {
                                e.set("isbn", &isbn);
                            }
                            found.push(Candidate { source: src.into(), entry: e });
                        }
                    }
                    Ok(Err(err)) => errors.push(format!("{name}: {err}")),
                    Err(_) => errors.push(format!("{name}: internal error")),
                }
            }
        }
    }
    (found, errors)
}

// ---------- helpers ----------

/// Escape characters that are special in BibTeX values.
fn tex(s: &str) -> String {
    let s: String = s.nfc().collect();
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("\\&"),
            '%' => out.push_str("\\%"),
            '#' => out.push_str("\\#"),
            '$' => out.push_str("\\$"),
            '_' => out.push_str("\\_"),
            '{' | '}' => {}
            '\u{00A0}' => out.push(' '),
            c => out.push(c),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Clean HTML/JATS markup from Crossref titles and abstracts into LaTeX.
fn clean_markup(s: &str) -> String {
    let mut s = s.to_string();
    for (a, b) in [
        ("<i>", "\u{1}"),
        ("</i>", "\u{2}"),
        ("<em>", "\u{1}"),
        ("</em>", "\u{2}"),
        ("<jats:italic>", "\u{1}"),
        ("</jats:italic>", "\u{2}"),
    ] {
        s = s.replace(a, b);
    }
    // drop remaining tags
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if in_tag => {}
            c => out.push(c),
        }
    }
    let out = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&nbsp;", " ");
    tex(&out).replace('\u{1}', "\\emph{").replace('\u{2}', "}").replace("\\emph{ ", "\\emph{").replace(" }", "}")
}

/// ISBD punctuation cleanup for MARC subfields: "Title /" → "Title".
fn isbd(s: &str) -> String {
    let s = s.trim().trim_end_matches([' ', '/', ':', ';', ',', '=']).trim();
    let s = s.strip_suffix('.').filter(|x| !x.ends_with(char::is_uppercase) || x.len() > 3 && !x.ends_with(" ed")).unwrap_or(s);
    let s = s.trim_start_matches('[').trim_end_matches(']');
    s.trim().to_string()
}

/// "Hock, Hans Henrich" → "Hans Henrich Hock" (the form used in this library).
fn person(s: &str) -> String {
    let s = isbd(s);
    let s = s.trim_end_matches(',').trim();
    if s.contains(',') {
        let n = parse_name(s);
        n.first_last()
    } else {
        s.to_string()
    }
}

fn year_in(s: &str) -> Option<String> {
    let b = s.as_bytes();
    for i in 0..b.len().saturating_sub(3) {
        if b[i..i + 4].iter().all(|c| c.is_ascii_digit())
            && (i == 0 || !b[i - 1].is_ascii_digit())
            && (i + 4 == b.len() || !b[i + 4].is_ascii_digit())
        {
            return Some(s[i..i + 4].to_string());
        }
    }
    None
}

fn set(e: &mut Entry, f: &str, v: &str) {
    let v = v.trim();
    if !v.is_empty() {
        e.fields.push(Field::new(f, v));
    }
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for p in path {
        cur = cur.get(p)?;
    }
    cur.as_str()
}

fn first_str(v: &Value, key: &str) -> Option<String> {
    match v.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Array(a) => a.iter().find_map(|x| x.as_str()).map(|s| s.to_string()),
        _ => None,
    }
}

fn parse_bibtex_response(body: &str) -> Result<Entry, String> {
    let lib = Library::parse(body);
    let mut e = lib.entries().next().cloned().ok_or("no BibTeX in response")?;
    e.raw = None;
    e.key.clear();
    // doi.org returns BibTeX-style fields; map to biblatex
    if let Some(j) = e.get_nonempty("journal") {
        e.remove("journal");
        e.set("journaltitle", &j);
    }
    if let Some(y) = e.get_nonempty("year") {
        e.remove("year");
        e.remove("month");
        e.set("date", &y);
    }
    e.remove("url");
    Ok(e)
}

// ---------- Crossref ----------

pub fn parse_crossref(body: &str) -> Result<Entry, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let m = v.get("message").ok_or("no data")?;
    let ty = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let has_container = first_str(m, "container-title").is_some_and(|s| !s.is_empty());
    let has_isbn = m.get("ISBN").is_some();
    let editors = names_list(m.get("editor"));
    let authors = names_list(m.get("author"));
    let bty = match ty {
        "journal-article" => "Article",
        "book-chapter" | "book-section" | "book-part" | "reference-entry" => "InCollection",
        "book" | "monograph" | "reference-book" | "edited-book" | "book-set" | "book-series" => {
            if authors.is_empty() && !editors.is_empty() {
                "Collection"
            } else {
                "Book"
            }
        }
        "proceedings-article" => "InProceedings",
        "proceedings" => "Proceedings",
        "dissertation" => "Thesis",
        "report" | "report-component" => "Report",
        "posted-content" => "Online",
        "dataset" => "Dataset",
        _ if has_container && has_isbn => "InCollection",
        _ if has_container => "Article",
        _ => "Misc",
    };
    let mut e = Entry::new(bty, "");
    if !authors.is_empty() {
        set(&mut e, "author", &authors.join(" and "));
    }
    if !editors.is_empty() {
        set(&mut e, "editor", &editors.join(" and "));
    }
    if let Some(t) = first_str(m, "title") {
        set(&mut e, "title", &clean_markup(&t));
    }
    if let Some(st) = first_str(m, "subtitle") {
        let st = clean_markup(&st);
        let title = e.get("title").unwrap_or_default();
        if !st.is_empty() && !title.contains(&st) {
            set(&mut e, "subtitle", &st);
        }
    }
    if let Some(c) = first_str(m, "container-title") {
        let c = clean_markup(&c);
        let f = match bty {
            "Article" => "journaltitle",
            "InCollection" | "InProceedings" => "booktitle",
            _ => "",
        };
        if !f.is_empty() {
            set(&mut e, f, &c);
        }
    }
    if let Some(x) = m.get("volume").and_then(|x| x.as_str()) {
        set(&mut e, "volume", x);
    }
    if let Some(x) = m.get("issue").and_then(|x| x.as_str()) {
        set(&mut e, "number", x);
    }
    if let Some(p) = m.get("page").and_then(|x| x.as_str()) {
        set(&mut e, "pages", &p.replace('-', "--"));
    }
    if let Some(x) = m.get("edition-number").and_then(|x| x.as_str()) {
        set(&mut e, "edition", x);
    }
    if bty != "Article" {
        if let Some(p) = m.get("publisher").and_then(|x| x.as_str()) {
            set(&mut e, "publisher", &tex(p));
        }
        if let Some(p) = m.get("publisher-location").and_then(|x| x.as_str()) {
            set(&mut e, "location", &tex(p));
        }
    }
    if let Some(d) = crossref_date(m) {
        set(&mut e, "date", &d);
    }
    if bty == "Thesis" {
        if let Some(inst) = m.get("institution").and_then(|i| i.as_array()).and_then(|a| a.first()).and_then(|i| i.get("name")).and_then(|n| n.as_str()) {
            set(&mut e, "institution", &tex(inst));
        }
        set(&mut e, "type", "phdthesis");
    }
    if let Some(isbn) = m.get("isbn-type").and_then(|a| a.as_array()).and_then(|a| {
        a.iter().find(|x| x.get("type").and_then(|t| t.as_str()) == Some("print")).or(a.first())
    }).and_then(|x| x.get("value")).and_then(|x| x.as_str()).map(|s| s.to_string()).or_else(|| first_str(m, "ISBN")) {
        set(&mut e, "isbn", &isbn);
    }
    if bty == "Article" {
        if let Some(issn) = first_str(m, "ISSN") {
            set(&mut e, "issn", &issn);
        }
    }
    if let Some(d) = m.get("DOI").and_then(|x| x.as_str()) {
        set(&mut e, "doi", d);
    }
    if let Some(a) = m.get("abstract").and_then(|x| x.as_str()) {
        let a = clean_markup(a);
        let a = a.trim_start_matches("Abstract").trim();
        set(&mut e, "abstract", a);
    }
    if let Some(l) = m.get("language").and_then(|x| x.as_str()) {
        if let Some(id) = langid(l) {
            set(&mut e, "langid", id);
        }
    }
    if e.get_nonempty("title").is_none() {
        return Err("record has no title".into());
    }
    Ok(e)
}

fn langid(code: &str) -> Option<&'static str> {
    Some(match code.get(..2)?.to_lowercase().as_str() {
        "en" => "english",
        "de" => "german",
        "fr" => "french",
        "it" => "italian",
        "es" => "spanish",
        "nl" => "dutch",
        "pt" => "portuguese",
        "ru" => "russian",
        "sa" => "sanskrit",
        "hi" => "hindi",
        "ja" => "japanese",
        _ => return None,
    })
}

fn names_list(v: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(a)) = v else { return vec![] };
    a.iter()
        .filter_map(|p| {
            if let Some(name) = p.get("name").and_then(|x| x.as_str()) {
                return Some(format!("{{{}}}", tex(name)));
            }
            let fam = p.get("family").and_then(|x| x.as_str()).unwrap_or("");
            let giv = p.get("given").and_then(|x| x.as_str()).unwrap_or("");
            if fam.is_empty() && giv.is_empty() {
                None
            } else if giv.is_empty() {
                Some(tex(fam))
            } else {
                Some(tex(&format!("{giv} {fam}")))
            }
        })
        .collect()
}

fn crossref_date(m: &Value) -> Option<String> {
    for key in ["published-print", "issued", "published", "published-online", "created"] {
        if let Some(parts) = m.get(key).and_then(|d| d.get("date-parts")).and_then(|p| p.get(0)).and_then(|p| p.as_array()) {
            let nums: Vec<i64> = parts.iter().filter_map(|x| x.as_i64()).collect();
            if let Some(y) = nums.first() {
                // print issues give the year of the volume; keep year-month only for articles
                return Some(match nums.len() {
                    1 => format!("{y:04}"),
                    _ => format!("{y:04}-{:02}", nums[1]),
                });
            }
        }
    }
    None
}

// ---------- Open Library ----------

pub fn parse_openlibrary(body: &str) -> Result<Vec<Entry>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let Some(obj) = v.as_object() else { return Ok(vec![]) };
    let mut out = Vec::new();
    for (_, b) in obj {
        let mut e = Entry::new("Book", "");
        let authors: Vec<String> = b
            .get("authors")
            .and_then(|a| a.as_array())
            .map(|a| a.iter().filter_map(|x| x.get("name").and_then(|n| n.as_str())).map(|s| tex(&person(s))).collect())
            .unwrap_or_default();
        if !authors.is_empty() {
            set(&mut e, "author", &authors.join(" and "));
        }
        if let Some(t) = str_at(b, &["title"]) {
            set(&mut e, "title", &tex(t));
        }
        if let Some(t) = str_at(b, &["subtitle"]) {
            set(&mut e, "subtitle", &tex(t));
        }
        let first_name = |key: &str| {
            b.get(key).and_then(|a| a.as_array()).and_then(|a| a.first()).and_then(|x| x.get("name")).and_then(|n| n.as_str()).map(|s| tex(&isbd(s)))
        };
        if let Some(p) = first_name("publish_places") {
            set(&mut e, "location", &p);
        }
        if let Some(p) = first_name("publishers") {
            set(&mut e, "publisher", &p);
        }
        if let Some(d) = str_at(b, &["publish_date"]).and_then(year_in) {
            set(&mut e, "date", &d);
        }
        if let Some(n) = b.get("number_of_pages").and_then(|n| n.as_i64()) {
            set(&mut e, "pagetotal", &n.to_string());
        }
        if let Some(i) = b.get("identifiers").and_then(|i| i.get("isbn_13").or(i.get("isbn_10"))).and_then(|a| a.get(0)).and_then(|x| x.as_str()) {
            set(&mut e, "isbn", i);
        }
        if e.get_nonempty("title").is_some() {
            out.push(e);
        }
    }
    Ok(out)
}

// ---------- Google Books ----------

pub fn parse_google_books(body: &str) -> Result<Vec<Entry>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("bad response: {e}"))?;
    let mut out = Vec::new();
    for item in v.get("items").and_then(|i| i.as_array()).cloned().unwrap_or_default().iter().take(3) {
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
        if let Some(t) = str_at(vi, &["title"]) {
            set(&mut e, "title", &tex(t));
        }
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
            let pick = ids
                .iter()
                .find(|x| x.get("type").and_then(|t| t.as_str()) == Some("ISBN_13"))
                .or(ids.first())
                .and_then(|x| x.get("identifier"))
                .and_then(|x| x.as_str());
            if let Some(i) = pick {
                set(&mut e, "isbn", i);
            }
        }
        if let Some(l) = str_at(vi, &["language"]).and_then(langid) {
            set(&mut e, "langid", l);
        }
        if e.get_nonempty("title").is_some() {
            out.push(e);
        }
    }
    Ok(out)
}

// ---------- MARC 21 XML (K10plus / GVK SRU) ----------

#[derive(Debug, Default, Clone)]
struct MarcField {
    tag: String,
    ind2: String,
    subs: Vec<(char, String)>,
}

impl MarcField {
    fn sub(&self, c: char) -> Option<&str> {
        self.subs.iter().find(|(k, _)| *k == c).map(|(_, v)| v.as_str())
    }
    fn subs_of(&self, c: char) -> Vec<&str> {
        self.subs.iter().filter(|(k, _)| *k == c).map(|(_, v)| v.as_str()).collect()
    }
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(';') {
            Some(j) if j <= 10 => {
                let ent = &after[..j];
                let rep = match ent {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    _ if ent.starts_with("#x") => u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32),
                    _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
                    _ => None,
                };
                match rep {
                    Some(c) => {
                        out.push(c);
                        rest = &after[j + 1..];
                    }
                    None => {
                        out.push('&');
                        rest = after;
                    }
                }
            }
            _ => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn xml_attr(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=");
    let mut search = tag;
    while let Some(i) = search.find(&pat) {
        // must be preceded by whitespace (not part of another attribute name)
        let ok = i == 0 || search.as_bytes()[i - 1].is_ascii_whitespace();
        let rest = &search[i + pat.len()..];
        if ok {
            let q = rest.chars().next()?;
            if q == '"' || q == '\'' {
                let end = rest[1..].find(q)?;
                return Some(xml_unescape(&rest[1..1 + end]));
            }
        }
        search = rest;
    }
    None
}

/// Minimal MARC-XML reader: returns (001 control number, data fields) per record.
fn marc_records(xml: &str) -> Result<Vec<(Option<String>, Vec<MarcField>)>, String> {
    let mut records = Vec::new();
    let mut cur: Option<(Option<String>, Vec<MarcField>)> = None;
    let mut field: Option<MarcField> = None;
    let mut sub_code: Option<char> = None;
    let mut control_tag: Option<String> = None;
    let mut pos = 0;
    while let Some(lt) = xml[pos..].find('<') {
        let text = &xml[pos..pos + lt];
        let start = pos + lt;
        let Some(gt) = xml[start..].find('>') else { return Err("bad XML".into()) };
        let tag = &xml[start + 1..start + gt];
        pos = start + gt + 1;
        if tag.starts_with('?') || tag.starts_with('!') {
            continue;
        }
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let body = tag.trim_start_matches('/').trim_end_matches('/');
        let qname = body.split_whitespace().next().unwrap_or("");
        let local = qname.rsplit(':').next().unwrap_or(qname);
        let in_marc_record = qname == "record" || (qname.ends_with(":record") && !qname.starts_with("zs") && !qname.starts_with("srw"));
        if closing {
            match local {
                "subfield" => {
                    if let (Some(f), Some(c)) = (field.as_mut(), sub_code.take()) {
                        f.subs.push((c, xml_unescape(text).nfc().collect()));
                    }
                }
                "controlfield" => {
                    if control_tag.as_deref() == Some("001") {
                        if let Some(rec) = cur.as_mut() {
                            rec.0 = Some(xml_unescape(text).trim().to_string());
                        }
                    }
                    control_tag = None;
                }
                "datafield" => {
                    if let (Some(rec), Some(f)) = (cur.as_mut(), field.take()) {
                        rec.1.push(f);
                    }
                }
                "record" if in_marc_record => {
                    if let Some(rec) = cur.take() {
                        if !rec.1.is_empty() {
                            records.push(rec);
                        }
                    }
                }
                _ => {}
            }
            continue;
        }
        match local {
            "record" if in_marc_record && !self_closing => cur = Some((None, Vec::new())),
            "datafield" => {
                field = Some(MarcField {
                    tag: xml_attr(body, "tag").unwrap_or_default(),
                    ind2: xml_attr(body, "ind2").unwrap_or_default(),
                    subs: vec![],
                })
            }
            "subfield" => sub_code = xml_attr(body, "code").and_then(|c| c.chars().next()),
            "controlfield" => control_tag = xml_attr(body, "tag"),
            _ => {}
        }
    }
    Ok(records)
}

pub fn parse_marcxml(xml: &str) -> Result<Vec<Entry>, String> {
    let mut out = Vec::new();
    for (ppn, fields) in marc_records(xml)? {
        let fields = &fields;
        fn by_tag<'a>(fields: &'a [MarcField], tag: &'static str) -> impl Iterator<Item = &'a MarcField> + 'a {
            fields.iter().filter(move |f| f.tag == tag)
        }
        let get = |tag: &'static str| by_tag(fields, tag);
        let mut authors = Vec::new();
        let mut editors = Vec::new();
        let mut translators = Vec::new();
        for f in get("100").chain(get("700")) {
            let Some(name) = f.sub('a') else { continue };
            let role: String = f.subs_of('4').join(" ") + " " + &f.subs_of('e').join(" ").to_lowercase();
            let p = tex(&person(name));
            if role.contains("edt") || role.contains("hrsg") || role.contains("editor") || role.contains("herausgeber") {
                editors.push(p);
            } else if role.contains("trl") || role.contains("übers") || role.contains("translator") {
                translators.push(p);
            } else if f.tag == "100" || role.contains("aut") {
                authors.push(p);
            }
        }
        let mut e = Entry::new(if authors.is_empty() && !editors.is_empty() { "Collection" } else { "Book" }, "");
        if !authors.is_empty() {
            set(&mut e, "author", &authors.join(" and "));
        }
        if !editors.is_empty() {
            set(&mut e, "editor", &editors.join(" and "));
        }
        if !translators.is_empty() {
            set(&mut e, "translator", &translators.join(" and "));
        }
        if let Some(t) = get("245").next() {
            let mut title = t.sub('a').map(isbd).unwrap_or_default();
            for part in t.subs_of('n').into_iter().chain(t.subs_of('p')) {
                title.push_str(". ");
                title.push_str(&isbd(part));
            }
            set(&mut e, "title", &tex(&title.replace(['\u{98}', '\u{9c}'], "")));
            if let Some(b) = t.sub('b') {
                set(&mut e, "subtitle", &tex(&isbd(b).replace(['\u{98}', '\u{9c}'], "")));
            }
        }
        if let Some(ed) = get("250").next().and_then(|f| f.sub('a')) {
            let ed = isbd(ed);
            let n: String = ed.chars().take_while(|c| c.is_ascii_digit()).collect();
            set(&mut e, "edition", if !n.is_empty() && ed.len() < 16 { &n } else { &ed });
        }
        let pubf = get("264").find(|f| f.ind2 == "1").or_else(|| get("260").next()).or_else(|| get("264").next());
        if let Some(p) = pubf {
            if let Some(a) = p.sub('a') {
                set(&mut e, "location", &tex(&isbd(a)));
            }
            if let Some(b) = p.sub('b') {
                set(&mut e, "publisher", &tex(&isbd(b)));
            }
            if let Some(c) = p.sub('c').and_then(year_in) {
                set(&mut e, "date", &c);
            }
        }
        if e.get_nonempty("date").is_none() {
            if let Some(c) = get("264").find_map(|f| f.sub('c')).and_then(year_in) {
                set(&mut e, "date", &c);
            }
        }
        if let Some(ext) = get("300").next().and_then(|f| f.sub('a')) {
            // "206 pages", "xvi, 340 Seiten"
            let nums: Vec<&str> = ext.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).collect();
            if let Some(n) = nums.last() {
                if !ext.to_lowercase().contains("vol") && !ext.to_lowercase().contains("bände") {
                    set(&mut e, "pagetotal", n);
                }
            }
        }
        if let Some(s) = get("490").next().or_else(|| get("830").next()) {
            if let Some(a) = s.sub('a') {
                set(&mut e, "series", &tex(&isbd(a)));
            }
            if let Some(v) = s.sub('v') {
                let v = isbd(v);
                let v = v.trim_start_matches(|c: char| c.is_alphabetic() || c == '.' || c == ' ').to_string();
                set(&mut e, "number", &tex(if v.is_empty() { s.sub('v').unwrap() } else { &v }));
            }
        }
        if let Some(i) = get("020").find_map(|f| f.sub('a')).or_else(|| get("020").find_map(|f| f.sub('9'))) {
            let d: String = i.chars().filter(|c| c.is_ascii_digit() || *c == 'X').collect();
            set(&mut e, "isbn", &d);
        }
        if let Some(l) = get("041").next().and_then(|f| f.sub('a')) {
            let id = match l {
                "eng" => Some("english"),
                "ger" => Some("german"),
                "fre" => Some("french"),
                "ita" => Some("italian"),
                "san" => Some("sanskrit"),
                "hin" => Some("hindi"),
                "spa" => Some("spanish"),
                _ => None,
            };
            if let Some(id) = id {
                set(&mut e, "langid", id);
            }
        }
        if let Some(p) = ppn {
            set(&mut e, "ppn_gvk", &p);
        }
        if e.get_nonempty("title").is_some() {
            out.push(e);
        }
    }
    Ok(out)
}

/// Turn a fetched entry into one ready for the library.
pub fn finalize(mut e: Entry, use_latex: bool) -> Entry {
    if use_latex {
        for f in e.fields.iter_mut() {
            if f.name != "doi" && f.name != "url" && f.name != "isbn" {
                f.parts = vec![crate::bib::Part::Lit(unicode_to_latex(&f.editor_text()))];
            }
        }
    }
    e.raw = None;
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ids() {
        assert_eq!(detect("https://doi.org/10.1002/9781119144892.ch23"), Some(Ident::Doi("10.1002/9781119144892.ch23".into())));
        assert_eq!(detect("doi:10.1017/S1356186322000268."), Some(Ident::Doi("10.1017/S1356186322000268".into())));
        assert_eq!(detect("10.2307/602910"), Some(Ident::Doi("10.2307/602910".into())));
        assert_eq!(detect("ISBN 978-81-932319-3-7"), Some(Ident::Isbn("9788193231937".into())));
        assert_eq!(detect("0-306-40615-2"), Some(Ident::Isbn("0306406152".into())));
        assert_eq!(detect("hello"), None);
        assert_eq!(detect("10/hbph7k"), Some(Ident::Doi("10/hbph7k".into())));
        assert_eq!(detect("https://doi.org/10/hbph7k"), Some(Ident::Doi("10/hbph7k".into())));
        let handle = r#"{"responseCode":1,"handle":"10/hbph7k","values":[{"index":1,"type":"HS_ALIAS","data":{"format":"string","value":"10.1002/9781119144892.ch23"}}]}"#;
        assert_eq!(parse_handle_alias(handle).unwrap(), "10.1002/9781119144892.ch23");
        assert_eq!(detect("9788193231938"), None);
    }

    const CROSSREF: &str = r#"{"status":"ok","message-type":"work","message":{"edition-number":"1","publisher":"Wiley","isbn-type":[{"type":"print","value":"9781119144861"},{"type":"electronic","value":"9781119144892"}],"DOI":"10.1002\/9781119144892.ch23","type":"other","page":"399-413","title":["The Science of Medicine"],"author":[{"given":"Dominik","family":"Wujastyk","sequence":"first","affiliation":[]}],"container-title":["Wiley Blackwell Companion to Hinduism"],"language":"en","issued":{"date-parts":[[2022,5,7]]},"ISBN":["9781119144861","9781119144892"],"subtitle":[]}}"#;

    #[test]
    fn crossref() {
        let e = parse_crossref(CROSSREF).unwrap();
        assert_eq!(e.ty, "InCollection");
        assert_eq!(e.get("author").unwrap(), "Dominik Wujastyk");
        assert_eq!(e.get("booktitle").unwrap(), "Wiley Blackwell Companion to Hinduism");
        assert_eq!(e.get("pages").unwrap(), "399--413");
        assert_eq!(e.get("date").unwrap(), "2022-05");
        assert_eq!(e.get("isbn").unwrap(), "9781119144861");
        assert_eq!(e.get("publisher").unwrap(), "Wiley");
        assert_eq!(e.get("langid").unwrap(), "english");
        let art = r#"{"message":{"type":"journal-article","title":["The book of women\u2019s rituals: the Central Asian adaptation of the <i>\u02bfAq\u0101\u02beid al-nis\u0101\u02be<\/i>"],"author":[{"given":"Aziza","family":"Shanazarova"}],"container-title":["Journal of the Royal Asiatic Society"],"volume":"33","issue":"2","page":"317-333","published-print":{"date-parts":[[2023,4]]},"ISSN":["1356-1863","1474-0591"],"DOI":"10.1017\/S1356186322000268","publisher":"Cambridge University Press (CUP)","abstract":"<jats:title>Abstract<\/jats:title><jats:p>This article &amp; that.<\/jats:p>"}}"#;
        let e = parse_crossref(art).unwrap();
        assert_eq!(e.ty, "Article");
        assert_eq!(e.get("title").unwrap(), "The book of women’s rituals: the Central Asian adaptation of the \\emph{ʿAqāʾid al-nisāʾ}");
        assert_eq!(e.get("journaltitle").unwrap(), "Journal of the Royal Asiatic Society");
        assert_eq!(e.get("number").unwrap(), "2");
        assert_eq!(e.get("issn").unwrap(), "1356-1863");
        assert!(e.get("publisher").is_none());
        assert_eq!(e.get("abstract").unwrap(), "This article \\& that.");
    }

    #[test]
    fn openlibrary() {
        let body = r#"{"ISBN:9780195171464": {"title": "Manu's Code of Law", "subtitle": "a critical edition and translation of the Mānava-Dharmaśāstra", "authors": [{"name": "Patrick Olivelle"}], "publishers": [{"name": "Oxford University Press"}], "publish_places": [{"name": "New York"}], "publish_date": "2005", "number_of_pages": 1136, "identifiers": {"isbn_13": ["9780195171464"]}}}"#;
        let v = parse_openlibrary(body).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].get("author").unwrap(), "Patrick Olivelle");
        assert_eq!(v[0].get("location").unwrap(), "New York");
        assert_eq!(v[0].get("pagetotal").unwrap(), "1136");
        assert_eq!(v[0].get("subtitle").unwrap(), "a critical edition and translation of the Mānava-Dharmaśāstra");
    }

    #[test]
    fn google() {
        let body = r#"{"kind":"books#volumes","totalItems":1,"items":[{"volumeInfo":{"title":"The Wiley Blackwell Companion to Hinduism","authors":["Gavin Flood"],"publisher":"John Wiley & Sons","publishedDate":"2022-05-23","industryIdentifiers":[{"type":"ISBN_10","identifier":"1119144868"},{"type":"ISBN_13","identifier":"9781119144861"}],"pageCount":640,"language":"en"}}]}"#;
        let v = parse_google_books(body).unwrap();
        assert_eq!(v[0].get("publisher").unwrap(), "John Wiley \\& Sons");
        assert_eq!(v[0].get("date").unwrap(), "2022");
        assert_eq!(v[0].get("isbn").unwrap(), "9781119144861");
    }

    #[test]
    fn marc() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<zs:searchRetrieveResponse xmlns:zs="http://www.loc.gov/zing/srw/"><zs:version>1.1</zs:version><zs:numberOfRecords>1</zs:numberOfRecords><zs:records><zs:record><zs:recordSchema>marcxml</zs:recordSchema><zs:recordPacking>xml</zs:recordPacking><zs:recordData><record xmlns="http://www.loc.gov/MARC21/slim" type="Bibliographic">
<leader>00000cam a22000002c 4500</leader>
<controlfield tag="001">896719839</controlfield>
<datafield tag="020" ind1=" " ind2=" "><subfield code="a">9788193231937</subfield><subfield code="9">978-81-932319-3-7</subfield></datafield>
<datafield tag="041" ind1=" " ind2=" "><subfield code="a">eng</subfield></datafield>
<datafield tag="245" ind1="1" ind2="0"><subfield code="a">Vrātya culture in Vedic sources</subfield><subfield code="b">select papers from the Panel on "Vrātya Culture in Vedic Sources" at the 16th World Sanskrit Conference (28 June - 2 July 2015), Bangkok, Thailand</subfield><subfield code="c">edited by Tiziana Pontillo, Moreno Dore and Hans Henrich Hock</subfield></datafield>
<datafield tag="264" ind1=" " ind2="1"><subfield code="a">New Delhi</subfield><subfield code="b">DK Publishers Distributors Pvt. Ltd</subfield><subfield code="c">2016</subfield></datafield>
<datafield tag="300" ind1=" " ind2=" "><subfield code="a">206 pages</subfield><subfield code="c">24 cm</subfield></datafield>
<datafield tag="490" ind1="0" ind2=" "><subfield code="a">16th WSC</subfield><subfield code="v">volume 5</subfield></datafield>
<datafield tag="700" ind1="1" ind2=" "><subfield code="a">Pontillo, Tiziana</subfield><subfield code="e">HerausgeberIn</subfield><subfield code="4">edt</subfield></datafield>
<datafield tag="700" ind1="1" ind2=" "><subfield code="a">Dore, Moreno</subfield><subfield code="4">edt</subfield></datafield>
<datafield tag="700" ind1="1" ind2=" "><subfield code="a">Hock, Hans Henrich</subfield><subfield code="d">1938-</subfield><subfield code="4">edt</subfield></datafield>
</record></zs:recordData><zs:recordPosition>1</zs:recordPosition></zs:record></zs:records></zs:searchRetrieveResponse>"#;
        let v = parse_marcxml(xml).unwrap();
        assert_eq!(v.len(), 1);
        let e = &v[0];
        assert_eq!(e.ty, "Collection");
        assert_eq!(e.get("editor").unwrap(), "Tiziana Pontillo and Moreno Dore and Hans Henrich Hock");
        assert_eq!(e.get("title").unwrap(), "Vrātya culture in Vedic sources");
        assert_eq!(e.get("location").unwrap(), "New Delhi");
        assert_eq!(e.get("publisher").unwrap(), "DK Publishers Distributors Pvt. Ltd");
        assert_eq!(e.get("date").unwrap(), "2016");
        assert_eq!(e.get("pagetotal").unwrap(), "206");
        assert_eq!(e.get("series").unwrap(), "16th WSC");
        assert_eq!(e.get("number").unwrap(), "5");
        assert_eq!(e.get("isbn").unwrap(), "9788193231937");
        assert_eq!(e.get("ppn_gvk").unwrap(), "896719839");
        assert_eq!(e.get("langid").unwrap(), "english");
    }
}
