//! Integrity checks. Each issue may carry an automatic fix.

use crate::bib::{Entry, Library};
use crate::files::{parse_file_field, FileDirs};
use crate::latex::accents_to_unicode;
use crate::profile::required_fields;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Error => "Error",
            Severity::Warning => "Warning",
            Severity::Info => "Suggestion",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Issue {
    /// Index into `Library::items`; `None` for file-level problems.
    pub item: Option<usize>,
    pub key: String,
    pub field: Option<String>,
    pub severity: Severity,
    pub message: String,
    /// New value for `field`, if the problem can be fixed automatically.
    pub fix: Option<String>,
}

/// Full DOIs, plus shortDOIs (`10/abcd`).
static DOI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^10\.\d{4,9}/\S+$|^10/[a-z0-9]+$").unwrap());
static DOI_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^\s*(?:https?://)?(?:dx\.)?doi\.org/|^\s*doi:\s*").unwrap());
static DATE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let d = r"-?\d{4}(?:-\d{2}(?:-\d{2})?)?[?~%]?";
    Regex::new(&format!(r"^(?:{d})?(?:/(?:{d}|\.\.)?)?$")).unwrap()
});
static PAGES_HYPHEN_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\w)\s*(?:-|–|—)\s*(\w)").unwrap());
static HTML_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(?:amp|lt|gt|quot|#\d+|#x[0-9a-fA-F]+);|</?(?:i|b|em|sup|sub|scp)>").unwrap());
static KEY_BAD_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r##"[\s,{}"#%~\\'()=]"##).unwrap());

pub fn isbn_valid(s: &str) -> bool {
    let d: Vec<char> = s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
    match d.len() {
        10 => {
            let mut sum = 0;
            for (i, c) in d.iter().enumerate() {
                let v = if *c == 'X' || *c == 'x' {
                    if i != 9 {
                        return false;
                    }
                    10
                } else {
                    c.to_digit(10).unwrap()
                };
                sum += v * (10 - i as u32);
            }
            sum % 11 == 0
        }
        13 => {
            if d.iter().any(|c| !c.is_ascii_digit()) {
                return false;
            }
            let sum: u32 = d.iter().enumerate().map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 }).sum();
            sum % 10 == 0
        }
        _ => false,
    }
}

pub fn issn_valid(s: &str) -> bool {
    let d: Vec<char> = s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
    if d.len() != 8 {
        return false;
    }
    let mut sum = 0;
    for (i, c) in d.iter().enumerate() {
        let v = if *c == 'X' || *c == 'x' { 10 } else { c.to_digit(10).unwrap() };
        sum += v * (8 - i as u32);
    }
    sum % 11 == 0
}

/// Normalise ISBN text (several ISBNs may be separated by spaces, commas or semicolons).
pub fn split_isbns(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in s.split(|c: char| c == ';' || c == ',' || c == '/' || c.is_whitespace()) {
        let d: String = part.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
        if d.len() >= 10 {
            out.push(d.to_uppercase());
        }
    }
    out
}

/// Canonical ISBN-13 (converting ISBN-10) for duplicate detection.
pub fn isbn13(s: &str) -> Option<String> {
    let d: String = s.chars().filter(|c| c.is_ascii_digit() || *c == 'X' || *c == 'x').collect();
    match d.len() {
        13 => Some(d),
        10 => {
            let core = format!("978{}", &d[..9]);
            let sum: u32 =
                core.chars().enumerate().map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 }).sum();
            Some(format!("{core}{}", (10 - sum % 10) % 10))
        }
        _ => None,
    }
}

pub fn clean_doi(s: &str) -> String {
    DOI_URL_RE.replace(s.trim(), "").trim().to_string()
}

fn ctx_is_multivolume(ty: &str) -> bool {
    ty.starts_with("mv")
}

/// Entry types that are parts of a larger work.
fn is_part_type(ty: &str) -> bool {
    matches!(
        ty,
        "incollection" | "inbook" | "inproceedings" | "inreference" | "bookinbook" | "suppbook" | "suppcollection"
            | "article" | "review" | "suppperiodical"
    )
}

fn alias_value<'a>(e: &'a Entry, field: &str) -> Option<String> {
    e.get_nonempty(field)
}

struct Ctx<'a> {
    biblatex: bool,
    custom_req: HashMap<String, Vec<String>>,
    by_key: HashMap<String, &'a Entry>,
}

impl<'a> Ctx<'a> {
    fn new(lib: &'a Library) -> Ctx<'a> {
        let mut custom_req: HashMap<String, Vec<String>> = HashMap::new();
        for it in &lib.items {
            if let crate::bib::Item::Special { body, .. } = it {
                if let Some(rest) = body.trim().strip_prefix("jabref-entrytype:") {
                    if let Some((name, def)) = rest.split_once(':') {
                        if let Some(r) = def.split("req[").nth(1).and_then(|x| x.split(']').next()) {
                            custom_req.insert(
                                name.trim().to_lowercase(),
                                r.split(';').filter(|s| !s.is_empty()).map(|s| s.replace('/', "|")).collect(),
                            );
                        }
                    }
                }
            }
        }
        let by_key = lib.entries().map(|e| (e.key.to_lowercase(), e)).collect();
        Ctx { biblatex: lib.is_biblatex(), custom_req, by_key }
    }
}

/// Run all checks. `dirs` enables the missing-file check.
pub fn check(lib: &Library, dirs: Option<&FileDirs>) -> Vec<Issue> {
    let ctx = Ctx::new(lib);
    let mut issues = Vec::new();
    for e in &lib.errors {
        issues.push(Issue {
            item: None,
            key: String::new(),
            field: None,
            severity: Severity::Error,
            message: format!("Syntax error near line {}: {}. This text is kept unchanged but is not shown as an entry.", e.line, e.message),
            fix: None,
        });
    }
    let mut keys: HashMap<String, Vec<usize>> = HashMap::new();
    let mut dois: HashMap<String, Vec<usize>> = HashMap::new();
    let mut isbns: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, it) in lib.items.iter().enumerate() {
        let crate::bib::Item::Entry(e) = it else { continue };
        if !e.key.trim().is_empty() {
            keys.entry(e.key.clone()).or_default().push(idx);
        }
        for (d, i) in identifiers(e) {
            if let Some(d) = d {
                dois.entry(d).or_default().push(idx);
            }
            if let Some(i) = i {
                isbns.entry(i).or_default().push(idx);
            }
        }
        entry_issues(&ctx, idx, e, dirs, &mut issues);
    }
    for (k, v) in keys {
        if v.len() > 1 {
            for &idx in &v {
                issues.push(dup_key_issue(idx, &k, v.len()));
            }
        }
    }
    for (label, map) in [("DOI", dois), ("ISBN", isbns)] {
        for (val, v) in map {
            let uniq: Vec<usize> = {
                let mut u = v.clone();
                u.sort();
                u.dedup();
                u
            };
            if uniq.len() > 1 {
                for &idx in &uniq {
                    issues.push(dup_id_issue(lib, idx, label, &val, &uniq));
                }
            }
        }
    }
    // if most linked files are missing, the PDF folder is probably not set:
    // say that once instead of listing thousands of files
    let linked = lib.entries().filter(|e| e.get_nonempty("file").is_some()).count();
    let missing = issues.iter().filter(|i| i.message.starts_with("Linked file not found")).count();
    if missing > 50 && missing * 2 > linked {
        issues.retain(|i| !i.message.starts_with("Linked file not found"));
        let looked: Vec<String> = dirs.map(|d| d.dirs.iter().map(|p| p.display().to_string()).collect()).unwrap_or_default();
        issues.push(Issue {
            item: None,
            key: String::new(),
            field: Some("file".into()),
            severity: Severity::Warning,
            message: format!(
                "{missing} of {linked} linked files were not found (looked in: {}). Set the PDF folder in Preferences.",
                if looked.is_empty() { "nowhere".to_string() } else { looked.join(", ") }
            ),
            fix: None,
        });
    }
    issues.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.key.cmp(&b.key)));
    issues
}

/// Checks for one entry only (used by the editor).
pub fn check_entry(lib: &Library, idx: usize, dirs: Option<&FileDirs>) -> Vec<Issue> {
    let Some(crate::bib::Item::Entry(e)) = lib.items.get(idx) else { return vec![] };
    let ctx = Ctx::new(lib);
    let mut issues = Vec::new();
    entry_issues(&ctx, idx, e, dirs, &mut issues);
    let same_key = lib.entries().filter(|x| !e.key.is_empty() && x.key == e.key).count();
    if same_key > 1 {
        issues.push(dup_key_issue(idx, &e.key, same_key));
    }
    let mine = identifiers(e);
    if !mine.is_empty() {
        for (label, pick) in [("DOI", 0), ("ISBN", 1)] {
            let vals: Vec<String> = mine.iter().filter_map(|t| if pick == 0 { t.0.clone() } else { t.1.clone() }).collect();
            for val in vals {
                let mut hits: Vec<usize> = lib
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(i, it)| match it {
                        crate::bib::Item::Entry(x) => identifiers(x)
                            .iter()
                            .any(|t| if pick == 0 { t.0.as_ref() == Some(&val) } else { t.1.as_ref() == Some(&val) })
                            .then_some(i),
                        _ => None,
                    })
                    .collect();
                hits.dedup();
                if hits.len() > 1 {
                    issues.push(dup_id_issue(lib, idx, label, &val, &hits));
                }
            }
        }
    }
    issues.sort_by(|a, b| a.severity.cmp(&b.severity));
    issues
}

/// (DOI, ISBN-13) pairs used for duplicate detection.
fn identifiers(e: &Entry) -> Vec<(Option<String>, Option<String>)> {
    let mut out = Vec::new();
    if let Some(d) = e.get_nonempty("doi") {
        let c = clean_doi(&d);
        if DOI_RE.is_match(&c) {
            out.push((Some(c.to_lowercase()), None));
        }
    }
    if !is_part_type(&e.ty_lower()) {
        if let Some(v) = e.get_nonempty("isbn") {
            for i in split_isbns(&v) {
                if isbn_valid(&i) {
                    out.push((None, isbn13(&i)));
                }
            }
        }
    }
    out
}

fn dup_key_issue(idx: usize, key: &str, n: usize) -> Issue {
    Issue {
        item: Some(idx),
        key: key.to_string(),
        field: None,
        severity: Severity::Error,
        message: format!("Duplicate citation key ({n} entries)"),
        fix: None,
    }
}

fn dup_id_issue(lib: &Library, idx: usize, label: &str, val: &str, all: &[usize]) -> Issue {
    let key_of = |i: usize| if let Some(crate::bib::Item::Entry(e)) = lib.items.get(i) { e.key.clone() } else { String::new() };
    let key = key_of(idx);
    let others: Vec<String> = all.iter().filter(|&&i| i != idx).map(|&i| key_of(i)).collect();
    Issue {
        item: Some(idx),
        key,
        field: Some(label.to_lowercase()),
        severity: Severity::Warning,
        message: format!("Possible duplicate: same {label} {val} as {}", others.join(", ")),
        fix: None,
    }
}

fn entry_issues(ctx: &Ctx, idx: usize, e: &Entry, dirs: Option<&FileDirs>, issues: &mut Vec<Issue>) {
    let biblatex = ctx.biblatex;
    let ty = e.ty_lower();
    let mut push = |field: Option<&str>, sev: Severity, msg: String, fix: Option<String>| {
        issues.push(Issue { item: Some(idx), key: e.key.clone(), field: field.map(|s| s.to_string()), severity: sev, message: msg, fix })
    };

    if e.key.trim().is_empty() {
        push(None, Severity::Error, "Entry has no citation key".into(), None);
    } else if KEY_BAD_RE.is_match(&e.key) {
        push(None, Severity::Error, "Citation key contains characters LaTeX cannot handle (space, comma, braces, %, # …)".into(), None);
    }

    let parent = e.get_nonempty("crossref").and_then(|c| ctx.by_key.get(&c.trim().to_lowercase()).copied());
    if let Some(c) = e.get_nonempty("crossref") {
        if parent.is_none() {
            push(Some("crossref"), Severity::Warning, format!("Cross-referenced entry '{c}' is not in this library"), None);
        }
    }

    // fields required by a custom JabRef type definition are reported as suggestions
    let custom = ctx.custom_req.contains_key(&ty);
    let req: Vec<String> = ctx
        .custom_req
        .get(&ty)
        .cloned()
        .unwrap_or_else(|| required_fields(&ty, biblatex).iter().map(|s| s.to_string()).collect());
    for r in &req {
        let alts: Vec<&str> = r.split('|').collect();
        let have = alts.iter().any(|a| {
            alias_value(e, a).is_some()
                || parent.is_some_and(|p| alias_value(p, a).is_some() || (*a == "booktitle" && alias_value(p, "title").is_some()))
        });
        if !have {
            let label = alts.join(" or ");
            let sev = if custom { Severity::Info } else { Severity::Warning };
            push(Some(alts[0]), sev, format!("Missing required field: {label}"), None);
        }
    }
    for f in &e.fields {
        let name = f.name.to_lowercase();
        let v = f.editor_text();
        if v.trim().is_empty() {
            continue;
        }
        if v != v.trim() && !matches!(name.as_str(), "abstract" | "annotation" | "note" | "contents") {
            push(Some(&f.name), Severity::Info, "Leading or trailing spaces".into(), Some(v.trim().to_string()));
        }
        if HTML_RE.is_match(&v) && name != "url" && name != "file" {
            let fixed = v
                .replace("&amp;", "\\&")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&quot;", "\"")
                .replace("<i>", "\\emph{")
                .replace("</i>", "}")
                .replace("<em>", "\\emph{")
                .replace("</em>", "}")
                .replace("<b>", "\\textbf{")
                .replace("</b>", "}");
            let fix = if HTML_RE.is_match(&fixed) { None } else { Some(fixed) };
            push(Some(&f.name), Severity::Warning, "Contains HTML code (probably from an online import)".into(), fix);
        }
        if !matches!(name.as_str(), "url" | "file" | "doi" | "eprint") && crate::latex::has_latex_accents(&v) {
            push(Some(&f.name), Severity::Info, "LaTeX accent commands could be written as Unicode".into(), Some(accents_to_unicode(&v)));
        }
        match name.as_str() {
            "doi" => {
                let c = clean_doi(&v);
                if c != v.trim() && DOI_RE.is_match(&c) {
                    push(Some(&f.name), Severity::Warning, "DOI should be stored without the https://doi.org/ prefix".into(), Some(c.clone()));
                } else if !DOI_RE.is_match(&c) {
                    push(Some(&f.name), Severity::Warning, format!("'{v}' does not look like a DOI"), None);
                }
            }
            "isbn" => {
                let list = split_isbns(&v);
                if list.is_empty() {
                    push(Some(&f.name), Severity::Warning, format!("'{v}' does not look like an ISBN"), None);
                }
                for i in list {
                    if !isbn_valid(&i) {
                        push(Some(&f.name), Severity::Warning, format!("ISBN {i} has an invalid check digit"), None);
                    }
                }
            }
            "issn" => {
                for part in v
                    .split(|c: char| c == ';' || c == ',' || c.is_whitespace())
                    .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("issn") && !s.eq_ignore_ascii_case("issn:"))
                {
                    if !issn_valid(part) {
                        push(Some(&f.name), Severity::Info, format!("ISSN {part} has an invalid check digit"), None);
                    }
                }
            }
            "eprint" | "url"
                if v.matches("ark:/").count() > 1
                    || v.matches("http").count() > 1
                    || (name != "url" && v.trim().contains(|c: char| c.is_whitespace() || c == ',' || c == ';')) =>
            {
                let multi = ctx_is_multivolume(&ty);
                push(
                    Some(&f.name),
                    Severity::Warning,
                    if multi {
                        "More than one link in this field; only one can work. Give each volume its own entry (right-click → Add Volume)".into()
                    } else {
                        "More than one link in this field; only one can work. For several volumes, use a multi-volume entry with one entry per volume".into()
                    },
                    None,
                );
            }
            "eprint" => {
                if e.get_nonempty("eprinttype").is_none() && v.trim().starts_with("ark:") {
                    push(Some("eprinttype"), Severity::Info, "eprint is an ARK but eprinttype is empty".into(), Some("ark".into()));
                }
            }
            "pages" => {
                if v.contains('–') || v.contains('—') || (PAGES_HYPHEN_RE.is_match(&v) && !v.contains("--")) {
                    let fixed = PAGES_HYPHEN_RE.replace_all(&v, "$1--$2").to_string();
                    if fixed != v {
                        push(Some(&f.name), Severity::Info, "Page ranges should use a double hyphen (--)".into(), Some(fixed));
                    }
                }
            }
            "date" | "urldate" | "origdate" | "eventdate" if biblatex => {
                if !DATE_RE.is_match(v.trim()) {
                    push(Some(&f.name), Severity::Warning, format!("'{v}' is not a valid biblatex date (YYYY, YYYY-MM or YYYY-MM-DD; ranges with /)"), None);
                }
            }
            "year" => {
                if !v.trim().chars().all(|c| c.is_ascii_digit()) {
                    push(Some(&f.name), Severity::Info, format!("Year '{v}' is not a plain number"), None);
                }
                if let Some(d) = e.get_nonempty("date") {
                    if !d.trim().starts_with(v.trim()) && v.trim().chars().all(|c| c.is_ascii_digit()) {
                        push(Some(&f.name), Severity::Warning, format!("Year {v} disagrees with date {d}"), None);
                    }
                }
            }
            "url" => {
                let t = v.trim();
                let low = t.to_lowercase();
                if !(low.starts_with("http://") || low.starts_with("https://") || low.starts_with("ftp://") || low.starts_with("file:")) {
                    push(Some(&f.name), Severity::Warning, "URL should start with http:// or https://".into(), None);
                }
            }
            "author" | "editor" | "translator" => {
                if v.contains(';') || v.contains(" & ") {
                    push(Some(&f.name), Severity::Warning, "Names must be separated by 'and', not ';' or '&'".into(), Some(v.replace(" & ", " and ").replace(';', " and ").split_whitespace().collect::<Vec<_>>().join(" ")));
                }
            }
            "title" | "booktitle" | "journaltitle" => {
                let letters: String = v.chars().filter(|c| c.is_alphabetic()).collect();
                if letters.chars().count() > 12 && letters.chars().all(|c| !c.is_lowercase()) {
                    push(Some(&f.name), Severity::Info, "Written in capitals".into(), None);
                }
            }
            "citationkey" => {
                if v.trim() != e.key {
                    push(Some(&f.name), Severity::Warning, format!("citationkey field '{v}' differs from the entry key '{}'", e.key), Some(e.key.clone()));
                }
            }
            "file" => {
                if let Some(d) = dirs {
                    for lf in parse_file_field(&v) {
                        if lf.path.starts_with("http://") || lf.path.starts_with("https://") {
                            continue;
                        }
                        if d.resolve(&lf.path).is_none() {
                            push(Some(&f.name), Severity::Warning, format!("Linked file not found: {}", lf.path), None);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    // alias pairs with differing values
    for (a, b) in [("journal", "journaltitle"), ("address", "location"), ("school", "institution")] {
        if let (Some(x), Some(y)) = (e.get_nonempty(a), e.get_nonempty(b)) {
            if x.trim() != y.trim() {
                push(Some(a), Severity::Info, format!("Both '{a}' and '{b}' are set, with different values"), None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isbn() {
        assert!(isbn_valid("9781119144892"));
        assert!(isbn_valid("978-81-932319-3-7"));
        assert!(!isbn_valid("9781119144893"));
        assert!(isbn_valid("0-306-40615-2"));
        assert_eq!(isbn13("0306406152").unwrap(), "9780306406157");
        assert!(issn_valid("1356-1863"));
    }

    #[test]
    fn several_links_in_one_field() {
        let lib = Library::parse("@MvBook{m, editor={E}, title={T}, date={1900}, eprint={ark:/13960/a ark:/13960/b}, eprinttype={ark}}\n@Book{b, author={A}, title={T}, date={1900}, url={http://a.org http://b.org}}\n@Book{c, author={A}, title={T}, date={1900}, eprint={ark:/13960/a}}");
        let iss = check(&lib, None);
        assert_eq!(iss.iter().filter(|i| i.message.starts_with("More than one link")).count(), 2, "{iss:#?}");
    }

    #[test]
    fn date_re() {
        for d in ["2022", "2022-05", "2022-05-07", "1990/1995", "1990/", "-0300", "1990?", "2001-03-04/2001-03-06"] {
            assert!(DATE_RE.is_match(d), "{d}");
        }
        for d in ["2022-5", "May 2022", "1990-1995", "c. 1900"] {
            assert!(!DATE_RE.is_match(d), "{d}");
        }
    }

    #[test]
    fn checks() {
        let src = r#"@Book{a, author={X; Y}, title={T}, date={2020}, doi={https://doi.org/10.1000/xyz}, pages={1-10}, isbn={9781119144893}}
@Book{a, title={B}, date={May 2020}}
@InCollection{c, author={Z}, title={C}, crossref={p}, pages={3--4}}
@Collection{p, editor={E}, title={P}, date={2001}}
@Article{d, author={{\=A}nanda}, title={T}, journaltitle={J}, date={2001}, doi={10.1000/xyz}}
"#;
        let lib = Library::parse(src);
        let iss = check(&lib, None);
        let msgs: Vec<String> = iss.iter().map(|i| format!("{}:{}", i.key, i.message)).collect();
        let has = |s: &str| msgs.iter().any(|m| m.contains(s));
        assert!(has("Duplicate citation key"), "{msgs:#?}");
        assert!(has("separated by 'and'"));
        assert!(has("without the https://doi.org/"));
        assert!(has("double hyphen"));
        assert!(has("invalid check digit"));
        assert!(has("not a valid biblatex date"));
        assert!(has("Missing required field: author or editor"));
        assert!(has("Possible duplicate: same DOI"));
        assert!(has("Unicode"));
        // crossref inheritance satisfies booktitle and date
        assert!(!msgs.iter().any(|m| m.starts_with("c:")), "{msgs:#?}");
        let fix = iss.iter().find(|i| i.message.contains("double hyphen")).unwrap().fix.clone().unwrap();
        assert_eq!(fix, "1--10");
    }
}
