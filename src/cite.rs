//! Formatted citations via the CSL engine in `hayagriva`.
//!
//! Entries are mapped to hayagriva's data model directly (rather than through
//! its own biblatex importer) so that subtitles, series, crossref inheritance
//! and LaTeX markup are handled the way biblatex would.

use crate::bib::Entry;
use crate::latex::{to_plain, to_protected};
use crate::names::parse_names;
use hayagriva::citationberg::{FontStyle, FontVariant, FontWeight, IndependentStyle, Locale, VerticalAlign};
use hayagriva::{BibliographyDriver, BibliographyRequest, CitationItem, CitationRequest, ElemChild, ElemChildren};
use serde_json::{json, Map, Value};
use std::path::PathBuf;

pub struct StyleInfo {
    pub id: String,
    pub title: String,
    xml: String,
}

const BUILTIN: &[(&str, &str)] = &[
    ("chicago-author-date-17th-edition", include_str!("../styles/chicago-author-date-17th-edition.csl")),
    ("new-harts-rules-author-date-publisher", include_str!("../styles/new-harts-rules-author-date-publisher.csl")),
    ("new-harts-rules-author-date", include_str!("../styles/new-harts-rules-author-date.csl")),
    ("new-harts-rules-notes-publisher", include_str!("../styles/new-harts-rules-notes-publisher.csl")),
    ("new-harts-rules-notes", include_str!("../styles/new-harts-rules-notes.csl")),
    ("american-medical-association", include_str!("../styles/american-medical-association.csl")),
];

/// Short names for the built-in styles.
fn short_title(id: &str, full: &str) -> String {
    match id {
        "chicago-author-date-17th-edition" => "Chicago 17th, author-date".into(),
        "new-harts-rules-author-date" => "New Hart's Rules 2nd, author-date".into(),
        "new-harts-rules-author-date-publisher" => "New Hart's Rules 2nd, author-date + publisher".into(),
        "new-harts-rules-notes" => "New Hart's Rules 2nd, notes".into(),
        "new-harts-rules-notes-publisher" => "New Hart's Rules 2nd, notes + publisher".into(),
        "american-medical-association" => "AMA 11th".into(),
        _ => full.to_string(),
    }
}

pub fn user_style_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("suci").join("styles"))
}

pub struct Styles {
    pub list: Vec<StyleInfo>,
    /// Capitalise English titles in formatted citations (with Sūcī's own
    /// language-aware rules; the style's generic Title Case is never used,
    /// as it mangles German, French and Sanskrit titles).
    pub title_case: bool,
    parsed: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<IndependentStyle>>>,
    locales: Vec<Locale>,
}

impl Styles {
    pub fn load() -> Styles {
        let mut list: Vec<StyleInfo> = BUILTIN
            .iter()
            .map(|(id, xml)| StyleInfo { id: id.to_string(), title: short_title(id, &csl_title(xml)), xml: xml.to_string() })
            .collect();
        // any extra .csl files the user drops into ~/.config/suci/styles
        if let Some(dir) = user_style_dir() {
            if let Ok(rd) = std::fs::read_dir(&dir) {
                let mut extra: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "csl")).collect();
                extra.sort();
                for p in extra {
                    if let Ok(xml) = std::fs::read_to_string(&p) {
                        let id = p.file_stem().unwrap().to_string_lossy().to_string();
                        if list.iter().any(|s| s.id == id) {
                            continue;
                        }
                        list.push(StyleInfo { id, title: csl_title(&xml), xml });
                    }
                }
            }
        }
        Styles { list, title_case: false, parsed: Default::default(), locales: english_locales() }
    }

    pub fn set_title_case(&mut self, on: bool) {
        self.title_case = on;
    }

    fn style(&self, id: &str) -> Result<std::rc::Rc<IndependentStyle>, String> {
        if let Some(s) = self.parsed.borrow().get(id) {
            return Ok(s.clone());
        }
        let info = self.list.iter().find(|s| s.id == id).ok_or_else(|| format!("unknown style {id}"))?;
        let mut xml = expand_substitutes(&info.xml);
        xml = xml.replace(" text-case=\"title\"", "");
        let st = IndependentStyle::from_xml(&xml).map_err(|e| format!("cannot read style {id}: {e} (dependent styles are not supported; use the parent style)"))?;
        let rc = std::rc::Rc::new(st);
        self.parsed.borrow_mut().insert(id.to_string(), rc.clone());
        Ok(rc)
    }

    /// Format entries. `lookup` resolves crossref keys.
    pub fn render(&self, style_id: &str, entries: &[&Entry], lookup: &dyn Fn(&str) -> Option<Entry>) -> Result<Rendered, String> {
        let style = self.style(style_id)?;
        let mut map = Map::new();
        let mut keys = Vec::new();
        for e in entries {
            let mut full = with_crossref(e, lookup);
            if self.title_case && crate::titlecase::auto_applies(&full) {
                crate::titlecase::apply(&mut full);
            }
            let key = if e.key.is_empty() { "nokey".to_string() } else { e.key.clone() };
            map.insert(key.clone(), to_hayagriva(&full));
            keys.push(key);
        }
        let lib: hayagriva::Library = match serde_json::from_value(Value::Object(map.clone())) {
            Ok(l) => l,
            Err(_) => {
                // retry with every number-like field reduced to a plain string or dropped
                let simplified: Map<String, Value> = map.into_iter().map(|(k, v)| (k, simplify_numbers(v))).collect();
                serde_json::from_value(Value::Object(simplified)).map_err(|e| format!("could not convert entry: {e}"))?
            }
        };
        let items: Vec<&hayagriva::Entry> = keys.iter().filter_map(|k| lib.get(k)).collect();
        let mut driver = BibliographyDriver::new();
        for it in &items {
            driver.citation(CitationRequest::from_items(vec![CitationItem::with_entry(*it)], &style, &self.locales));
        }
        let r = driver.finish(BibliographyRequest::new(&style, None, &self.locales));
        let mut out = Rendered::default();
        // glossary items have no bibliographic form; show name and description
        if !entries.is_empty() && entries.iter().all(|e| e.ty_lower() == "entry") {
            for e in entries {
                let (t, h, m) = glossary_item(e);
                out.citations.push(e.key.clone());
                out.bib_text.push(t);
                out.bib_html.push(h);
                out.bib_markup.push(m);
            }
            return Ok(out);
        }
        for c in &r.citations {
            let mut w = Writer::default();
            w.children(&c.citation);
            out.citations.push(w.text);
        }
        if let Some(b) = r.bibliography {
            for it in &b.items {
                let mut w = Writer::default();
                w.children(&it.content);
                out.bib_text.push(w.text);
                out.bib_html.push(w.html);
                out.bib_markup.push(w.markup);
            }
        }
        out.note_style = style.settings.class == hayagriva::citationberg::StyleClass::Note;
        Ok(out)
    }
}

/// CSL says a bare `<names variable="x"/>` inside `<substitute>` inherits the
/// `<name>`, `<et-al>` and `<label>` settings of the enclosing `<names>`.
/// hayagriva only does this for direct children, not for ones nested in
/// `<choose>`, so editors substituted for authors come out in the wrong form.
/// Make the inheritance explicit in the XML.
pub fn expand_substitutes(xml: &str) -> String {
    let re = regex::Regex::new(r#"<names(\s[^>]*?)\s*/>"#).unwrap();
    let mut out = String::with_capacity(xml.len() + 4096);
    let mut pos = 0;
    while let Some(off) = xml[pos..].find("<substitute>") {
        let sub_start = pos + off;
        let Some(sub_len) = xml[sub_start..].find("</substitute>") else { break };
        let sub_end = sub_start + sub_len;
        // enclosing <names ...> open tag
        let opts = xml[..sub_start]
            .rfind("<names")
            .and_then(|n| xml[n..sub_start].find('>').map(|g| (n + g + 1, n)))
            .filter(|(_, n)| !xml[*n..sub_start].contains("/>") || xml[*n..sub_start].find('>') != xml[*n..sub_start].find("/>").map(|i| i + 1))
            .map(|(open_end, _)| xml[open_end..sub_start].trim().to_string())
            .unwrap_or_default();
        // a <label> needs an explicit <name> before it, or it is printed first
        let opts = if !opts.is_empty() && !opts.contains("<name") { format!("<name/>{opts}") } else { opts };
        out.push_str(&xml[pos..sub_start]);
        let block = &xml[sub_start..sub_end];
        if opts.is_empty() {
            out.push_str(block);
        } else {
            out.push_str(&re.replace_all(block, |c: &regex::Captures| format!("<names{}>{}</names>", &c[1], opts)));
        }
        pos = sub_end;
    }
    out.push_str(&xml[pos..]);
    out
}

const NUMERIC_KEYS: &[&str] = &["volume", "issue", "edition", "chapter", "page-range", "volume-total", "page-total"];

fn simplify_numbers(v: Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.into_iter()
                .filter_map(|(k, v)| {
                    if NUMERIC_KEYS.contains(&k.as_str()) {
                        match &v {
                            Value::Number(_) => Some((k, v)),
                            _ => None,
                        }
                    } else {
                        Some((k, simplify_numbers(v)))
                    }
                })
                .collect(),
        ),
        other => other,
    }
}

/// Only the English locales (the styles' terms: "edited by", "eds", "n.d.").
/// Decoding all ~60 bundled locales would cost memory for nothing.
fn english_locales() -> Vec<Locale> {
    let mut out: Vec<Locale> = hayagriva::archive::LOCALES
        .iter()
        .filter(|b| {
            let b: &[u8] = b;
            b.windows(5).any(|w| w == b"en-US" || w == b"en-GB")
        })
        .filter_map(|b| ciborium::from_reader::<Locale, _>(*b).ok())
        .collect();
    if out.is_empty() {
        out = hayagriva::archive::locales();
    }
    out
}

fn csl_title(xml: &str) -> String {
    xml.split("<title>").nth(1).and_then(|s| s.split("</title>").next()).unwrap_or("Untitled style").replace("&amp;", "&")
}

#[derive(Debug, Default, Clone)]
pub struct Rendered {
    /// In-text citation or (for note styles) the first-note form.
    pub citations: Vec<String>,
    pub bib_text: Vec<String>,
    pub bib_html: Vec<String>,
    /// Pango markup for display.
    pub bib_markup: Vec<String>,
    pub note_style: bool,
}

#[derive(Default)]
struct Writer {
    text: String,
    html: String,
    markup: String,
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

impl Writer {
    fn children(&mut self, c: &ElemChildren) {
        for ch in &c.0 {
            self.child(ch);
        }
    }
    fn child(&mut self, c: &ElemChild) {
        match c {
            ElemChild::Text(f) => self.formatted(&f.text, &f.formatting, None),
            ElemChild::Elem(e) => self.children(&e.children),
            ElemChild::Markup(m) => {
                self.text.push_str(m);
                self.html.push_str(&esc(m));
                self.markup.push_str(&esc(m));
            }
            ElemChild::Link { text, url } => self.formatted(&text.text, &text.formatting, Some(url)),
            ElemChild::Transparent { .. } => {}
        }
    }
    fn formatted(&mut self, t: &str, f: &hayagriva::Formatting, url: Option<&String>) {
        self.text.push_str(t);
        let mut open_h = String::new();
        let mut close_h = String::new();
        let mut open_m = String::new();
        let mut close_m = String::new();
        if f.font_style == FontStyle::Italic {
            open_h.push_str("<i>");
            close_h.insert_str(0, "</i>");
            open_m.push_str("<i>");
            close_m.insert_str(0, "</i>");
        }
        if f.font_weight == FontWeight::Bold {
            open_h.push_str("<b>");
            close_h.insert_str(0, "</b>");
            open_m.push_str("<b>");
            close_m.insert_str(0, "</b>");
        }
        if f.font_variant == FontVariant::SmallCaps {
            open_h.push_str("<span style=\"font-variant: small-caps\">");
            close_h.insert_str(0, "</span>");
            open_m.push_str("<span variant=\"smallcaps\">");
            close_m.insert_str(0, "</span>");
        }
        match f.vertical_align {
            VerticalAlign::Sup => {
                open_h.push_str("<sup>");
                close_h.insert_str(0, "</sup>");
                open_m.push_str("<sup>");
                close_m.insert_str(0, "</sup>");
            }
            VerticalAlign::Sub => {
                open_h.push_str("<sub>");
                close_h.insert_str(0, "</sub>");
                open_m.push_str("<sub>");
                close_m.insert_str(0, "</sub>");
            }
            _ => {}
        }
        if let Some(u) = url {
            self.html.push_str(&format!("<a href=\"{}\">", esc(u).replace('"', "&quot;")));
        }
        self.html.push_str(&open_h);
        self.html.push_str(&esc(t));
        self.html.push_str(&close_h);
        if url.is_some() {
            self.html.push_str("</a>");
        }
        self.markup.push_str(&open_m);
        self.markup.push_str(&esc(t));
        self.markup.push_str(&close_m);
    }
}

/// Glossary items (@Entry): "Name: description" in plain text, HTML and Pango markup.
pub fn glossary_item(e: &Entry) -> (String, String, String) {
    let name = crate::latex::to_plain(&e.get("name").unwrap_or_default());
    let desc = crate::latex::to_plain(&e.get("description").unwrap_or_default());
    let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
    let text = if desc.is_empty() { name.clone() } else { format!("{name}: {desc}") };
    let html = if desc.is_empty() { format!("<b>{}</b>", esc(&name)) } else { format!("<b>{}</b>: {}", esc(&name), esc(&desc)) };
    (text, html.clone(), html)
}

/// Copy fields inherited from a crossref'd parent (biblatex rules, simplified).
pub fn with_crossref(e: &Entry, lookup: &dyn Fn(&str) -> Option<Entry>) -> Entry {
    let mut out = e.clone();
    let Some(pkey) = e.get_nonempty("crossref") else { return out };
    let Some(parent) = lookup(pkey.trim()) else { return out };
    let mut inherit = |from: &str, to: &str| {
        if out.get_nonempty(to).is_none() {
            if let Some(v) = parent.get_nonempty(from) {
                out.fields.push(crate::bib::Field::new(to, &v));
            }
        }
    };
    let pty = parent.ty_lower();
    let cty = e.ty_lower();
    if pty.starts_with("mv") {
        // biblatex: a multi-volume work's title becomes the volume's main title
        inherit("title", "maintitle");
        inherit("subtitle", "mainsubtitle");
        inherit("titleaddon", "maintitleaddon");
        if cty.starts_with("in") || cty == "bookinbook" || cty.starts_with("supp") {
            inherit("author", "bookauthor");
        } else {
            inherit("author", "author");
        }
    } else if pty == "periodical" {
        inherit("title", "journaltitle");
        inherit("subtitle", "journalsubtitle");
    } else {
        inherit("title", "booktitle");
        inherit("subtitle", "booksubtitle");
        inherit("author", "bookauthor");
    }
    // a parent that is itself a volume of a multi-volume work
    for f in ["maintitle", "mainsubtitle", "maintitleaddon"] {
        inherit(f, f);
    }
    for f in [
        "editor", "translator", "publisher", "location", "address", "date", "year", "series", "number", "edition",
        "isbn", "journaltitle", "journal", "institution", "organization", "langid",
    ] {
        inherit(f, f);
    }
    // "2 vols." describes the set, not one of its volumes
    if !(pty.starts_with("mv") && e.get_nonempty("volume").is_some()) {
        inherit("volumes", "volumes");
    }
    out.raw = None;
    out
}

fn prot(e: &Entry, f: &str) -> Option<String> {
    e.get_nonempty(f).map(|v| to_protected(&v))
}

fn title_with_sub(e: &Entry, t: &str, s: &str) -> Option<String> {
    let title = prot(e, t)?;
    match prot(e, s) {
        Some(sub) => {
            let sep = if title.ends_with(['?', '!', ':', '.']) { " " } else { ": " };
            Some(format!("{title}{sep}{sub}"))
        }
        None => Some(title),
    }
}

/// Title for a volume of a multi-volume work, Chicago style:
/// "Main Title" (with the volume number kept separately) when the volume has no
/// title of its own, else "Main Title. Vol. 2, Volume Title" (number consumed).
fn volume_title(e: &Entry, own_title: &str, own_sub: &str) -> Option<(String, bool)> {
    let main = title_with_sub(e, "maintitle", "mainsubtitle")?;
    match title_with_sub(e, own_title, own_sub) {
        Some(own) if crate::latex::to_plain(&own) != crate::latex::to_plain(&main) => {
            let vol = e.get_nonempty("volume").map(|v| to_plain(&v));
            Some(match vol {
                Some(v) => (format!("{main}. Vol. {v}, {own}"), true),
                None => (format!("{main}. {own}"), false),
            })
        }
        _ => Some((main, false)),
    }
}

fn persons(raw: &str) -> Vec<Value> {
    parse_names(raw)
        .into_iter()
        .filter(|n| !n.is_others())
        .map(|n| {
            if n.literal || n.first.is_empty() && n.von.is_empty() {
                return json!({ "name": to_plain(&n.last) });
            }
            let mut m = Map::new();
            m.insert("name".into(), json!(to_plain(&n.last)));
            if !n.first.is_empty() {
                m.insert("given-name".into(), json!(to_plain(&n.first)));
            }
            if !n.von.is_empty() {
                m.insert("prefix".into(), json!(to_plain(&n.von)));
            }
            if !n.jr.is_empty() {
                m.insert("suffix".into(), json!(to_plain(&n.jr)));
            }
            Value::Object(m)
        })
        .collect()
}

/// biblatex date → ISO date hayagriva accepts (first date of a range).
fn date_value(e: &Entry) -> Option<Value> {
    let raw = e.get_nonempty("date").or_else(|| e.get_nonempty("year"))?;
    let raw = to_plain(&raw);
    let first = raw.split('/').next().unwrap_or("").trim().trim_end_matches(['?', '~', '%']);
    let ok = |s: &str| {
        let parts: Vec<&str> = s.trim_start_matches('-').split('-').collect();
        !parts.is_empty()
            && parts[0].len() == 4
            && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            && parts.len() <= 3
    };
    let mut date = if ok(first) {
        first.to_string()
    } else {
        // find a 4-digit year anywhere
        let digits: Vec<&str> = raw.split(|c: char| !c.is_ascii_digit()).filter(|s| s.len() == 4).collect();
        digits.first()?.to_string()
    };
    // add month from the `month` field if the date is a bare year
    if date.len() == 4 {
        if let Some(m) = e.field("month") {
            let mv = m.expanded(&Default::default());
            if let Ok(n) = mv.trim().parse::<u32>() {
                if (1..=12).contains(&n) {
                    date = format!("{date}-{n:02}");
                }
            }
        }
    }
    Some(json!(date))
}

fn num(e: &Entry, f: &str) -> Option<Value> {
    let v = to_plain(&e.get_nonempty(f)?);
    Some(match v.trim().parse::<i64>() {
        Ok(n) => json!(n),
        Err(_) => json!(v),
    })
}

/// For hayagriva's strict numeric fields: plain integers only.
fn num_strict(e: &Entry, f: &str) -> Option<Value> {
    to_plain(&e.get_nonempty(f)?).trim().parse::<i64>().ok().map(|n| json!(n))
}

fn publisher(e: &Entry) -> Option<Value> {
    let name = prot(e, "publisher");
    let loc = prot(e, "location").or_else(|| prot(e, "address"));
    match (name, loc) {
        (None, None) => None,
        (Some(n), None) => Some(json!({ "name": n })),
        (None, Some(l)) => Some(json!({ "location": l })),
        (Some(n), Some(l)) => Some(json!({ "name": n, "location": l })),
    }
}

fn serials(e: &Entry, m: &mut Map<String, Value>, with_isbn: bool) {
    let mut s = Map::new();
    if let Some(d) = e.get_nonempty("doi") {
        s.insert("doi".into(), json!(crate::checks::clean_doi(&d)));
    }
    if with_isbn {
        if let Some(i) = e.get_nonempty("isbn") {
            s.insert("isbn".into(), json!(to_plain(&i)));
        }
    }
    if let Some(i) = e.get_nonempty("issn") {
        s.insert("issn".into(), json!(to_plain(&i)));
    }
    if !s.is_empty() {
        m.insert("serial-number".into(), Value::Object(s));
    }
}

fn url(e: &Entry, m: &mut Map<String, Value>) {
    if let Some(u) = e.get_nonempty("url") {
        let u = u.trim().replace("\\&", "&").replace("\\_", "_").replace("\\%", "%").replace("\\#", "#");
        if url_ok(&u) {
            let mut o = json!({ "value": u });
            if let Some(d) = e.get_nonempty("urldate") {
                let d = d.trim().to_string();
                if d.len() >= 4 && d[..4].chars().all(|c| c.is_ascii_digit()) {
                    o["date"] = json!(d.split('T').next().unwrap_or(&d));
                }
            }
            m.insert("url".into(), o);
        }
    }
}

fn url_ok(u: &str) -> bool {
    (u.starts_with("http://") || u.starts_with("https://")) && !u.contains(' ')
}

fn put(m: &mut Map<String, Value>, k: &str, v: Option<Value>) {
    if let Some(v) = v {
        m.insert(k.into(), v);
    }
}

fn put_s(m: &mut Map<String, Value>, k: &str, v: Option<String>) {
    if let Some(v) = v {
        m.insert(k.into(), json!(v));
    }
}

fn series_parent(e: &Entry, ty: &str) -> Option<Value> {
    let title = prot(e, "series")?;
    let mut s = Map::new();
    s.insert("type".into(), json!(ty));
    s.insert("title".into(), json!(title));
    put(&mut s, "volume", num(e, "number"));
    Some(Value::Object(s))
}

/// Build the hayagriva representation of an entry.
pub fn to_hayagriva(e: &Entry) -> Value {
    let ty = e.ty_lower();
    let mut m = Map::new();
    let authors = e.get_nonempty("author").map(|a| persons(&a)).filter(|v| !v.is_empty());
    let editors = e.get_nonempty("editor").map(|a| persons(&a)).filter(|v| !v.is_empty());
    let mut affiliated = Vec::new();
    if let Some(t) = e.get_nonempty("translator") {
        affiliated.push(json!({ "role": "translator", "names": persons(&t) }));
    }
    put_s(&mut m, "title", title_with_sub(e, "title", "subtitle"));
    put(&mut m, "date", date_value(e));
    put_s(&mut m, "note", prot(e, "note").filter(|n| n.chars().count() < 200));
    url(e, &mut m);

    let mut parent = Map::new();
    let kind: &str;
    match ty.as_str() {
        "article" | "review" | "periodical" => {
            kind = "article";
            put(&mut m, "author", authors.map(Value::Array));
            put(&mut m, "editor", editors.map(Value::Array));
            put(&mut m, "page-range", e.get_nonempty("pages").map(|p| json!(to_plain(&p))));
            parent.insert("type".into(), json!("periodical"));
            put_s(&mut parent, "title", title_with_sub(e, "journaltitle", "journalsubtitle").or_else(|| prot(e, "journal")));
            put(&mut parent, "volume", num(e, "volume"));
            put(&mut parent, "issue", num(e, "number").or_else(|| num(e, "issue")));
            serials(e, &mut m, false);
        }
        "incollection" | "inreference" | "suppcollection" | "inproceedings" | "inbook" | "bookinbook" | "suppbook" => {
            // hayagriva treats an Anthology/Proceedings/Reference parent as both the
            // container and the "volume title" (printing its title twice); a
            // Chapter in a Book is recognised as CSL `chapter` and renders
            // correctly in all the styles we ship.
            let (k, pty) = ("chapter", "book");
            kind = k;
            put(&mut m, "author", authors.map(Value::Array));
            put(&mut m, "page-range", e.get_nonempty("pages").map(|p| json!(to_plain(&p))));
            parent.insert("type".into(), json!(pty));
            let mut vol_used = false;
            match volume_title(e, "booktitle", "booksubtitle") {
                Some((t, used)) => {
                    parent.insert("title".into(), json!(t));
                    vol_used = used;
                }
                None => put_s(&mut parent, "title", title_with_sub(e, "booktitle", "booksubtitle")),
            }
            put(&mut parent, "editor", editors.map(Value::Array));
            if let Some(ba) = e.get_nonempty("bookauthor") {
                parent.insert("author".into(), Value::Array(persons(&ba)));
            }
            put(&mut parent, "publisher", publisher(e));
            put(&mut parent, "edition", num(e, "edition"));
            if !vol_used {
                put(&mut parent, "volume", num(e, "volume"));
            }
            put(&mut parent, "volume-total", num_strict(e, "volumes"));
            put(&mut parent, "date", date_value(e));
            let mut ps = Map::new();
            if let Some(i) = e.get_nonempty("isbn") {
                ps.insert("isbn".into(), json!(to_plain(&i)));
                parent.insert("serial-number".into(), Value::Object(ps));
            }
            if let Some(sp) = series_parent(e, "anthology") {
                parent.insert("parent".into(), sp);
            }
            serials(e, &mut m, false);
        }
        "thesis" | "phdthesis" | "mastersthesis" => {
            kind = "thesis";
            put(&mut m, "author", authors.map(Value::Array));
            put_s(&mut m, "organization", prot(e, "institution").or_else(|| prot(e, "school")));
            let genre = prot(e, "type").map(|t| match t.as_str() {
                "phdthesis" => "PhD thesis".to_string(),
                "mathesis" | "mastersthesis" => "MA thesis".to_string(),
                _ => t,
            });
            let genre = genre.or_else(|| match ty.as_str() {
                "phdthesis" => Some("PhD thesis".into()),
                "mastersthesis" => Some("MA thesis".into()),
                _ => None,
            });
            put_s(&mut m, "genre", genre);
            put_s(&mut m, "location", prot(e, "location").or_else(|| prot(e, "address")));
            serials(e, &mut m, false);
        }
        "report" | "techreport" => {
            kind = "report";
            put(&mut m, "author", authors.map(Value::Array));
            put(&mut m, "editor", editors.map(Value::Array));
            put_s(&mut m, "organization", prot(e, "institution"));
            put_s(&mut m, "genre", prot(e, "type"));
            put(&mut m, "publisher", publisher(e));
            if let Some(n) = e.get_nonempty("number") {
                m.insert("serial-number".into(), json!({ "serial": to_plain(&n) }));
            }
        }
        "online" | "www" | "electronic" => {
            kind = "web";
            put(&mut m, "author", authors.map(Value::Array));
            put(&mut m, "editor", editors.map(Value::Array));
            put_s(&mut m, "organization", prot(e, "organization"));
            serials(e, &mut m, false);
        }
        "unpublished" => {
            kind = "manuscript";
            put(&mut m, "author", authors.map(Value::Array));
            put_s(&mut m, "genre", prot(e, "howpublished"));
        }
        "manuscript" => {
            kind = "manuscript";
            put(&mut m, "author", authors.map(Value::Array));
            // CSL: archive = library, archive_location = shelfmark
            put_s(&mut m, "archive", prot(e, "library"));
            put_s(&mut m, "archive-location", prot(e, "shelfmark"));
            if !m.contains_key("title") {
                put_s(&mut m, "title", prot(e, "shorthand"));
            }
        }
        _ => {
            kind = match ty.as_str() {
                // collections are books with editors (hayagriva's Anthology type
                // mishandles editor-only entries)
                "book" | "mvbook" | "booklet" | "manual" | "collection" | "mvcollection" | "proceedings"
                | "mvproceedings" | "reference" | "mvreference" => "book",
                "audio" => "audio",
                "video" | "movie" => "video",
                "software" | "dataset" => "repository",
                _ => "misc",
            };
            put(&mut m, "author", authors.map(Value::Array));
            put(&mut m, "editor", editors.map(Value::Array));
            put(&mut m, "publisher", publisher(e).or_else(|| prot(e, "howpublished").map(|h| json!({ "name": h }))));
            put(&mut m, "edition", num(e, "edition"));
            let mut vol_used = false;
            if let Some((t, used)) = volume_title(e, "title", "subtitle") {
                m.insert("title".into(), json!(t));
                vol_used = used;
            }
            if !vol_used {
                put(&mut m, "volume", num(e, "volume"));
            }
            put(&mut m, "volume-total", num_strict(e, "volumes"));
            put(&mut m, "page-total", num_strict(e, "pagetotal"));
            serials(e, &mut m, true);
            // an Anthology parent gives the CSL collection title (series) without
            // hayagriva also printing the volume as a "part" ("Vol. 18, pt. 18")
            if let Some(sp) = series_parent(e, "anthology") {
                m.insert("parent".into(), sp);
            }
        }
    }
    if !affiliated.is_empty() {
        m.insert("affiliated".into(), Value::Array(affiliated));
    }
    m.insert("type".into(), json!(kind));
    if parent.len() > 1 {
        m.insert("parent".into(), Value::Object(parent));
    }
    Value::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::Library;

    fn render_all(src: &str, style: &str) -> Rendered {
        let lib = Library::parse(src);
        let styles = Styles::load();
        let entries: Vec<&Entry> = lib.entries().collect();
        let owned: Vec<Entry> = lib.entries().cloned().collect();
        let lookup = move |k: &str| owned.iter().find(|e| e.key == k).cloned();
        styles.render(style, &entries, &lookup).unwrap()
    }

    const SRC: &str = r#"
@InCollection{wuja-2022c,
  author = {Dominik Wujastyk}, booktitle = {The Wiley Blackwell Companion to Hinduism},
  editor = {Gavin D. Flood}, location = {Hoboken, NJ}, pages = {399--413},
  publisher = {Wiley \& Sons, Ltd.}, title = {The Science of Medicine}, edition = {2},
  doi = {10.1002/9781119144892.ch23}, date = {2022},
}
@Article{shan-2023, author = {Aziza Shanazarova}, title = {The Book of Women's Rituals: The Central Asian Adaptation of the \emph{ʿAqāʾid al-nisāʾ}},
  journaltitle = {Journal of the Royal Asiatic Society}, volume = {33}, number = {2}, pages = {317--333}, date = {2023-04}, doi = {10.1017/S1356186322000268}}
@Book{oliv-2005, author = {Patrick Olivelle}, title = {Manu's Code of Law}, subtitle = {A Critical Edition and Translation of the {M\=a}nava-Dharma{\'s}{\=a}stra}, location = {New York}, publisher = {Oxford University Press}, date = {2005}, series = {South Asia Research}}
@InCollection{epim-2023, author = {Andrey Epimakhov and Alexander Lubotsky}, title = {Fire and Water}, crossref = {kris-2023}, pages = {263--274}}
@Collection{kris-2023, editor = {Kristian Kristiansen and Guus Kroonen and Eske Willerslev}, title = {The Indo-European Puzzle Revisited}, publisher = {Cambridge University Press}, location = {Cambridge}, date = {2023}}
"#;

    #[test]
    fn chicago() {
        let r = render_all(SRC, "chicago-author-date-17th-edition");
        let all = r.bib_text.join("\n");
        eprintln!("{all}\n{:?}", r.citations);
        assert!(all.contains("Olivelle, Patrick. 2005. Manu's Code of Law: A Critical Edition and Translation of the Mānava-Dharmaśāstra. South Asia Research. New York: Oxford University Press."));
        assert!(all.contains("Wujastyk, Dominik. 2022. “The Science of Medicine.” In The Wiley Blackwell Companion to Hinduism, 2nd ed., edited by Gavin D. Flood, 399–413. Hoboken, NJ: Wiley & Sons, Ltd."), "{all}");
        assert!(all.contains("Kristiansen, Kristian, Guus Kroonen, and Eske Willerslev, eds. 2023. The Indo-European Puzzle Revisited."), "{all}");
        assert!(all.contains("ʿAqāʾid al-nisāʾ"), "{all}");
        assert!(all.contains("In The Indo-European Puzzle Revisited, edited by Kristian Kristiansen"), "{all}");
        assert_eq!(r.citations[0], "(Wujastyk 2022)");
        assert_eq!(r.citations[4], "(Kristiansen, Kroonen, and Willerslev 2023)");
        let html = r.bib_html.join("\n");
        assert!(html.contains("<i>Manu's Code of Law: A Critical Edition"));
        assert!(html.contains("Wiley &amp; Sons"));
    }

    #[test]
    fn all_styles_render() {
        let styles = Styles::load();
        for s in &styles.list {
            let r = render_all(SRC, &s.id);
            eprintln!("== {}\n{}\n{:?}", s.title, r.bib_text.join("\n"), r.citations);
            assert_eq!(r.citations.len(), 5);
            assert_eq!(r.bib_text.len(), 5, "{}", s.id);
        }
    }

    #[test]
    fn volumes_of_multivolume_works() {
        let src = r#"
@MvBook{kavi-sadv, editor={Sures Chandra Banerji}, title={Saduktikarṇāmṛta}, volumes={2}, location={Lahore}, publisher={Motilal}, date={1933/1936}}
@Book{kavi-sadv1, crossref={kavi-sadv}, volume={1}, date={1933}, eprint={ark:/13960/t0wq6n34h}, eprinttype={ark}}
@Book{kavi-sadv2, crossref={kavi-sadv}, volume={2}, title={Notes and Indexes}, date={1936}}
"#;
        let r = render_all(src, "chicago-author-date-17th-edition");
        let all = r.bib_text.join("\n");
        assert!(all.contains("Saduktikarṇāmṛta. 2 vols. Lahore: Motilal."), "{all}");
        let r = render_all(
            "@Book{falk, author={Harry Falk}, title={Aśokan Sites}, volume={18}, series={Monographien zur indischen Archäologie}, number={18}, location={Mainz}, publisher={Zabern}, date={2006}}",
            "chicago-author-date-17th-edition",
        );
        assert_eq!(r.bib_text[0], "Falk, Harry. 2006. Aśokan Sites. Vol. 18. Monographien zur indischen Archäologie 18. Mainz: Zabern.");
        assert!(all.contains("ed. 1933b. Saduktikarṇāmṛta. Vol. 1. Lahore: Motilal."), "{all}");
        assert!(all.contains("ed. 1936. Saduktikarṇāmṛta. Vol. 2, Notes and Indexes. Lahore: Motilal."), "{all}");
    }

    #[test]
    fn whole_library_renders() {
        let Ok(path) = std::env::var("SUCI_TEST_BIB") else { return };
        let src = std::fs::read_to_string(&path).unwrap();
        let lib = Library::parse(&src);
        let styles = Styles::load();
        let owned: Vec<Entry> = lib.entries().cloned().collect();
        let map: std::collections::HashMap<String, Entry> = owned.iter().map(|e| (e.key.clone(), e.clone())).collect();
        let lookup = |k: &str| map.get(k).cloned();
        let mut fails = 0;
        let t = std::time::Instant::now();
        for e in lib.entries() {
            match styles.render("chicago-author-date-17th-edition", &[e], &lookup) {
                Ok(r) if r.bib_text.len() == 1 => {}
                Ok(_) => {
                    fails += 1;
                    eprintln!("no output: {}", e.key)
                }
                Err(err) => {
                    fails += 1;
                    eprintln!("{}: {err}", e.key)
                }
            }
        }
        eprintln!("rendered {} entries in {:?}, {fails} failures", lib.entries().count(), t.elapsed());
        assert!(fails < 5);
    }
}
