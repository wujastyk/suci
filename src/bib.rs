//! Lossless BibTeX / biblatex model, parser and writer.
//!
//! Unmodified entries are written back byte-for-byte, so opening and saving a
//! file never produces spurious changes. Modified entries are re-serialised in
//! JabRef's layout (two-space indent, aligned `=`, braced values).

use std::collections::HashMap;
use std::fmt::Write as _;

/// One part of a field value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Part {
    /// `{...}` or `"..."` literal; the string is the inner text.
    Lit(String),
    /// A bare word: a macro (`may`, a `@String` name) or a number.
    Bare(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub parts: Vec<Part>,
}

impl Field {
    pub fn new(name: &str, text: &str) -> Self {
        Field { name: name.to_string(), parts: vec![Part::Lit(text.to_string())] }
    }

    /// Text as shown in the editor, using JabRef's `#macro#` convention for bare words.
    pub fn editor_text(&self) -> String {
        let mut s = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(t) => s.push_str(t),
                Part::Bare(b) if b.chars().all(|c| c.is_ascii_digit()) && self.parts.len() == 1 => {
                    s.push_str(b)
                }
                Part::Bare(b) => {
                    s.push('#');
                    s.push_str(b);
                    s.push('#');
                }
            }
        }
        s
    }

    /// Parse editor text (with `#macro#` segments) back into parts.
    pub fn parts_from_editor(text: &str) -> Vec<Part> {
        let mut parts = Vec::new();
        let mut rest = text;
        while let Some(start) = rest.find('#') {
            // `\#` is a literal hash in LaTeX; skip it
            if start > 0 && rest.as_bytes()[start - 1] == b'\\' {
                let (a, b) = rest.split_at(start + 1);
                push_lit(&mut parts, a);
                rest = b;
                continue;
            }
            let after = &rest[start + 1..];
            match after.find('#') {
                Some(end)
                    if end > 0
                        && after[..end]
                            .chars()
                            .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == ':' || c == '.') =>
                {
                    push_lit(&mut parts, &rest[..start]);
                    parts.push(Part::Bare(after[..end].to_string()));
                    rest = &after[end + 1..];
                }
                _ => {
                    push_lit(&mut parts, &rest[..start + 1]);
                    rest = after;
                }
            }
        }
        push_lit(&mut parts, rest);
        if parts.is_empty() {
            parts.push(Part::Lit(String::new()));
        }
        parts
    }

    /// Value with macros expanded (months, @String definitions).
    pub fn expanded(&self, strings: &HashMap<String, String>) -> String {
        let mut s = String::new();
        for p in &self.parts {
            match p {
                Part::Lit(t) => s.push_str(t),
                Part::Bare(b) => {
                    let lb = b.to_lowercase();
                    if let Some(v) = strings.get(&lb) {
                        s.push_str(v)
                    } else if let Some(m) = month_number(&lb) {
                        let _ = write!(s, "{m}");
                    } else {
                        s.push_str(b)
                    }
                }
            }
        }
        s
    }

    fn write_value(&self, out: &mut String) {
        let mut first = true;
        for p in &self.parts {
            if !first {
                out.push_str(" # ");
            }
            first = false;
            match p {
                Part::Lit(t) => {
                    out.push('{');
                    out.push_str(t);
                    out.push('}');
                }
                Part::Bare(b) => out.push_str(b),
            }
        }
    }
}

fn push_lit(parts: &mut Vec<Part>, s: &str) {
    if s.is_empty() {
        return;
    }
    if let Some(Part::Lit(prev)) = parts.last_mut() {
        prev.push_str(s);
    } else {
        parts.push(Part::Lit(s.to_string()));
    }
}

pub fn month_number(m: &str) -> Option<u32> {
    const M: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    M.iter().position(|x| *x == m).map(|i| i as u32 + 1)
}

#[derive(Debug, Clone)]
pub struct Entry {
    /// Entry type as written (e.g. `InCollection`).
    pub ty: String,
    pub key: String,
    pub fields: Vec<Field>,
    /// Original source text; `None` once the entry has been modified.
    pub raw: Option<String>,
}

impl Entry {
    pub fn new(ty: &str, key: &str) -> Self {
        Entry { ty: ty.to_string(), key: key.to_string(), fields: Vec::new(), raw: None }
    }

    pub fn ty_lower(&self) -> String {
        self.ty.to_lowercase()
    }

    pub fn field(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Editor text of a field (empty if missing).
    pub fn get(&self, name: &str) -> Option<String> {
        self.field(name).map(|f| f.editor_text())
    }

    pub fn get_nonempty(&self, name: &str) -> Option<String> {
        self.get(name).filter(|s| !s.trim().is_empty())
    }

    /// Set a field from editor text. Empty text removes the field.
    /// Returns true if anything changed.
    pub fn set(&mut self, name: &str, text: &str) -> bool {
        if text.trim().is_empty() {
            return self.remove(name);
        }
        let parts = Field::parts_from_editor(text);
        if let Some(f) = self.fields.iter_mut().find(|f| f.name.eq_ignore_ascii_case(name)) {
            if f.parts == parts {
                return false;
            }
            f.parts = parts;
        } else {
            self.fields.push(Field { name: name.to_string(), parts });
        }
        self.raw = None;
        true
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.fields.len();
        self.fields.retain(|f| !f.name.eq_ignore_ascii_case(name));
        if self.fields.len() != before {
            self.raw = None;
            true
        } else {
            false
        }
    }

    pub fn set_key(&mut self, key: &str) {
        if self.key != key {
            self.key = key.to_string();
            self.raw = None;
            // keep the redundant `citationkey` field (written by some tools) in sync
            if self.field("citationkey").is_some() {
                self.set("citationkey", key);
            }
        }
    }

    pub fn set_type(&mut self, ty: &str) {
        if self.ty != ty {
            self.ty = ty.to_string();
            self.raw = None;
        }
    }

    pub fn is_modified(&self) -> bool {
        self.raw.is_none()
    }

    /// Serialise in JabRef style.
    pub fn to_bibtex(&self) -> String {
        if let Some(r) = &self.raw {
            return r.clone();
        }
        let mut out = String::new();
        let _ = write!(out, "@{}{{{},\n", self.ty, self.key);
        let width = self.fields.iter().map(|f| f.name.chars().count()).max().unwrap_or(0);
        for f in &self.fields {
            let _ = write!(out, "  {:<width$} = ", f.name, width = width);
            f.write_value(&mut out);
            out.push_str(",\n");
        }
        out.push('}');
        out
    }
}

#[derive(Debug, Clone)]
pub enum Item {
    Entry(Entry),
    /// `@String{name = value}`; raw source kept.
    StringDef { name: String, value: String, raw: String },
    /// `@Comment{...}`, `@Preamble{...}`; raw source kept.
    Special { kind: String, body: String, raw: String },
    /// Anything between entries (whitespace, stray text); written back verbatim.
    Junk(String),
    /// A deleted entry's slot (kept so that item indices stay stable).
    Deleted,
}

#[derive(Debug, Clone, Default)]
pub struct Library {
    pub items: Vec<Item>,
    pub strings: HashMap<String, String>,
    pub errors: Vec<ParseError>,
}

#[derive(Debug, Clone)]
pub struct ParseError {
    pub line: usize,
    pub message: String,
}

impl Library {
    pub fn parse(src: &str) -> Library {
        Parser::new(src).run()
    }

    pub fn to_bibtex(&self) -> String {
        let mut out = String::with_capacity(self.items.len() * 600);
        let mut skip_ws_junk = false;
        for (i, it) in self.items.iter().enumerate() {
            if skip_ws_junk {
                skip_ws_junk = false;
                if let Item::Junk(j) = it {
                    if j.trim().is_empty() {
                        continue;
                    }
                }
            }
            match it {
                Item::Deleted => {
                    skip_ws_junk = true;
                }
                Item::Entry(e) => {
                    let modified = e.raw.is_none();
                    out.push_str(&e.to_bibtex());
                    // a re-serialised entry needs a separating blank line if the next junk lacks one
                    if modified {
                        if let Some(Item::Junk(j)) = self.items.get(i + 1) {
                            if !j.starts_with('\n') {
                                out.push_str("\n\n");
                            }
                        } else {
                            out.push_str("\n\n");
                        }
                    }
                }
                Item::StringDef { raw, .. } => out.push_str(raw),
                Item::Special { raw, .. } => out.push_str(raw),
                Item::Junk(j) => out.push_str(j),
            }
        }
        out
    }

    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.items.iter().filter_map(|i| if let Item::Entry(e) = i { Some(e) } else { None })
    }

    /// JabRef `jabref-meta:` settings stored in @Comment blocks.
    pub fn jabref_meta(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for it in &self.items {
            if let Item::Special { kind, body, .. } = it {
                if kind.eq_ignore_ascii_case("comment") {
                    if let Some(rest) = body.trim().strip_prefix("jabref-meta:") {
                        if let Some((k, v)) = rest.trim().split_once(':') {
                            m.insert(k.trim().to_string(), v.trim().trim_end_matches(';').to_string());
                        }
                    }
                }
            }
        }
        m
    }

    pub fn is_biblatex(&self) -> bool {
        self.jabref_meta().get("databaseType").map(|v| v == "biblatex").unwrap_or(true)
    }
}

struct Parser<'a> {
    s: &'a str,
    b: &'a [u8],
    pos: usize,
    lib: Library,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str) -> Self {
        Parser { s, b: s.as_bytes(), pos: 0, lib: Library::default() }
    }

    fn line_of(&self, pos: usize) -> usize {
        self.s[..pos.min(self.s.len())].bytes().filter(|&c| c == b'\n').count() + 1
    }

    fn run(mut self) -> Library {
        let mut junk_start = 0;
        while self.pos < self.b.len() {
            // find next '@' that starts an entry: '@' ident ws* ( '{' | '(' )
            let Some(off) = self.s[self.pos..].find('@') else { break };
            let at = self.pos + off;
            if let Some((ty, open_pos)) = self.entry_head(at) {
                if at > junk_start {
                    self.lib.items.push(Item::Junk(self.s[junk_start..at].to_string()));
                }
                match self.parse_body(at, &ty, open_pos) {
                    Ok(end) => {
                        self.pos = end;
                        junk_start = end;
                    }
                    Err(msg) => {
                        let line = self.line_of(at);
                        self.lib.errors.push(ParseError { line, message: msg });
                        // treat as junk up to the next line starting with '@'
                        let next = self.s[at + 1..]
                            .find("\n@")
                            .map(|i| at + 1 + i + 1)
                            .unwrap_or(self.s.len());
                        self.lib.items.push(Item::Junk(self.s[at..next].to_string()));
                        self.pos = next;
                        junk_start = next;
                    }
                }
            } else {
                self.pos = at + 1;
            }
        }
        if junk_start < self.s.len() {
            self.lib.items.push(Item::Junk(self.s[junk_start..].to_string()));
        }
        self.lib
    }

    fn entry_head(&self, at: usize) -> Option<(String, usize)> {
        let mut p = at + 1;
        let st = p;
        while p < self.b.len() && (self.b[p].is_ascii_alphanumeric() || self.b[p] == b'_' || self.b[p] == b'-') {
            p += 1;
        }
        if p == st {
            return None;
        }
        let ty = self.s[st..p].to_string();
        while p < self.b.len() && self.b[p].is_ascii_whitespace() {
            p += 1;
        }
        if p < self.b.len() && (self.b[p] == b'{' || self.b[p] == b'(') {
            Some((ty, p))
        } else {
            None
        }
    }

    fn skip_ws(&mut self) {
        while self.pos < self.b.len() && self.b[self.pos].is_ascii_whitespace() {
            self.pos += 1;
        }
    }

    /// Returns position after a balanced `{...}` starting at `p` (which must be '{').
    fn balanced(&self, p: usize) -> Result<usize, String> {
        let mut depth = 0i32;
        let mut i = p;
        while i < self.b.len() {
            match self.b[i] {
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(i + 1);
                    }
                }
                // a new entry at line start while still inside braces => unbalanced
                b'\n' if i + 1 < self.b.len() && self.b[i + 1] == b'@' && depth > 0 && self.entry_head(i + 1).is_some() => {
                    return Err("unbalanced braces".into());
                }
                _ => {}
            }
            i += 1;
        }
        Err("unbalanced braces (reached end of file)".into())
    }

    fn parse_body(&mut self, at: usize, ty: &str, open: usize) -> Result<usize, String> {
        let close_ch = if self.b[open] == b'{' { b'}' } else { b')' };
        let lty = ty.to_ascii_lowercase();
        if lty == "comment" || lty == "preamble" {
            let end = if close_ch == b'}' {
                self.balanced(open)?
            } else {
                self.s[open..].find(')').map(|i| open + i + 1).ok_or("unclosed (")?
            };
            let raw = self.s[at..end].to_string();
            let body = self.s[open + 1..end - 1].to_string();
            self.lib.items.push(Item::Special { kind: ty.to_string(), body, raw });
            return Ok(end);
        }
        self.pos = open + 1;
        if lty == "string" {
            self.skip_ws();
            let name = self.ident();
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return Err("malformed @String".into());
            }
            self.pos += 1;
            let parts = self.value(close_ch)?;
            self.skip_ws();
            if self.peek() != Some(close_ch) {
                return Err("malformed @String".into());
            }
            self.pos += 1;
            let f = Field { name: name.clone(), parts };
            let value = f.expanded(&self.lib.strings);
            self.lib.strings.insert(name.to_lowercase(), value.clone());
            let raw = self.s[at..self.pos].to_string();
            self.lib.items.push(Item::StringDef { name, value, raw });
            return Ok(self.pos);
        }
        // key
        self.skip_ws();
        let ks = self.pos;
        while self.pos < self.b.len() && !matches!(self.b[self.pos], b',' | b'\n' | b'\r') && self.b[self.pos] != close_ch {
            self.pos += 1;
        }
        let key = self.s[ks..self.pos].trim().to_string();
        self.skip_ws();
        let mut fields = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(c) if c == close_ch => {
                    self.pos += 1;
                    break;
                }
                Some(b',') => {
                    self.pos += 1;
                    continue;
                }
                None => return Err(format!("entry '{key}' not closed")),
                _ => {}
            }
            let name = self.ident();
            if name.is_empty() {
                return Err(format!("entry '{key}': expected field name"));
            }
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return Err(format!("entry '{key}': expected '=' after '{name}'"));
            }
            self.pos += 1;
            let parts = self.value(close_ch).map_err(|e| format!("entry '{key}', field '{name}': {e}"))?;
            fields.push(Field { name, parts });
        }
        let raw = self.s[at..self.pos].to_string();
        self.lib.items.push(Item::Entry(Entry { ty: ty.to_string(), key, fields, raw: Some(raw) }));
        Ok(self.pos)
    }

    fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    fn ident(&mut self) -> String {
        let st = self.pos;
        while self.pos < self.b.len() {
            let c = self.b[self.pos];
            if c.is_ascii_whitespace() || matches!(c, b'=' | b',' | b'{' | b'}' | b'(' | b')' | b'"' | b'#') {
                break;
            }
            self.pos += 1;
        }
        self.s[st..self.pos].to_string()
    }

    fn value(&mut self, close_ch: u8) -> Result<Vec<Part>, String> {
        let mut parts = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'{') => {
                    let end = self.balanced(self.pos)?;
                    parts.push(Part::Lit(self.s[self.pos + 1..end - 1].to_string()));
                    self.pos = end;
                }
                Some(b'"') => {
                    let mut depth = 0;
                    let mut i = self.pos + 1;
                    loop {
                        match self.b.get(i) {
                            None => return Err("unterminated quoted value".into()),
                            Some(b'{') => depth += 1,
                            Some(b'}') => depth -= 1,
                            Some(b'"') if depth == 0 && self.b[i - 1] != b'\\' => break,
                            _ => {}
                        }
                        i += 1;
                    }
                    parts.push(Part::Lit(self.s[self.pos + 1..i].to_string()));
                    self.pos = i + 1;
                }
                Some(_) => {
                    let st = self.pos;
                    while self.pos < self.b.len() {
                        let c = self.b[self.pos];
                        if c.is_ascii_whitespace() || c == b',' || c == b'#' || c == close_ch {
                            break;
                        }
                        self.pos += 1;
                    }
                    if self.pos == st {
                        return Err("empty value".into());
                    }
                    parts.push(Part::Bare(self.s[st..self.pos].to_string()));
                }
                None => return Err("unexpected end of file".into()),
            }
            self.skip_ws();
            if self.peek() == Some(b'#') {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(parts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let src = "% comment\n@Book{k1,\n  author = {A and B},\n  month = may,\n  title=\"T {x}\" # jan,\n}\n\n@Comment{jabref-meta: databaseType:biblatex;}\n";
        let lib = Library::parse(src);
        assert!(lib.errors.is_empty(), "{:?}", lib.errors);
        let e = lib.entries().next().unwrap();
        assert_eq!(e.key, "k1");
        assert_eq!(e.get("author").unwrap(), "A and B");
        assert_eq!(e.get("month").unwrap(), "#may#");
        assert_eq!(e.get("title").unwrap(), "T {x}#jan#");
        assert_eq!(lib.to_bibtex(), src);
        assert!(lib.is_biblatex());
    }

    #[test]
    fn modify_and_write() {
        let src = "@Book{k1,\n  title = {T},\n}\n\n@Article{k2, title={U}}\n";
        let mut lib = Library::parse(src);
        if let Item::Entry(e) = &mut lib.items[0] {
            e.set("date", "2020");
        }
        let out = lib.to_bibtex();
        assert!(out.starts_with("@Book{k1,\n  title = {T},\n  date  = {2020},\n}\n\n@Article{k2, title={U}}"));
    }

    #[test]
    fn editor_roundtrip() {
        let p = Field::parts_from_editor("abc #jan# def");
        assert_eq!(p, vec![Part::Lit("abc ".into()), Part::Bare("jan".into()), Part::Lit(" def".into())]);
        let p = Field::parts_from_editor("C\\# and # not");
        assert_eq!(p, vec![Part::Lit("C\\# and # not".into())]);
    }

    #[test]
    fn recover_from_bad_entry() {
        let src = "@Book{a, title = {missing close}\n\n@Book{b, title = {ok}}\n";
        let lib = Library::parse(src);
        assert_eq!(lib.entries().count(), 1);
        assert_eq!(lib.to_bibtex(), src);
        let src = "@Book{a, title = {unbalanced\n@Book{b, title = {ok}}\n";
        let lib = Library::parse(src);
        assert_eq!(lib.errors.len(), 1);
        assert_eq!(lib.entries().count(), 1);
        assert_eq!(lib.to_bibtex(), src);
    }

    /// Round-trip the user's real library if available.
    #[test]
    fn roundtrip_real_file() {
        let Ok(path) = std::env::var("SUCI_TEST_BIB") else { return };
        let src = std::fs::read_to_string(&path).unwrap();
        let t = std::time::Instant::now();
        let lib = Library::parse(&src);
        eprintln!("parsed {} entries in {:?}; errors: {:?}", lib.entries().count(), t.elapsed(), lib.errors);
        assert_eq!(lib.to_bibtex(), src);
    }
}
