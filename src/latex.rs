//! LaTeX <-> Unicode helpers.
//!
//! * [`to_plain`] turns a raw field value into readable Unicode text (for the
//!   list, search and previews).
//! * [`accents_to_unicode`] rewrites only accent/letter commands into Unicode,
//!   leaving all other LaTeX (`\emph`, `\cite`, …) untouched — safe to apply to
//!   stored data.
//! * [`unicode_to_latex`] does the reverse for non-ASCII letters.

use unicode_normalization::char::decompose_canonical;
use unicode_normalization::UnicodeNormalization;

/// Combining mark for a LaTeX accent command.
fn accent_mark(cmd: &str) -> Option<char> {
    Some(match cmd {
        "'" => '\u{0301}',
        "`" => '\u{0300}',
        "^" => '\u{0302}',
        "\"" => '\u{0308}',
        "~" => '\u{0303}',
        "=" => '\u{0304}',
        "." => '\u{0307}',
        "u" => '\u{0306}',
        "v" => '\u{030C}',
        "H" => '\u{030B}',
        "c" => '\u{0327}',
        "d" => '\u{0323}',
        "b" => '\u{0331}',
        "k" => '\u{0328}',
        "r" => '\u{030A}',
        "t" => '\u{0361}',
        _ => return None,
    })
}

fn mark_command(mark: char) -> Option<&'static str> {
    Some(match mark {
        '\u{0301}' => "'",
        '\u{0300}' => "`",
        '\u{0302}' => "^",
        '\u{0308}' => "\"",
        '\u{0303}' => "~",
        '\u{0304}' => "=",
        '\u{0307}' => ".",
        '\u{0306}' => "u",
        '\u{030C}' => "v",
        '\u{030B}' => "H",
        '\u{0327}' => "c",
        '\u{0323}' => "d",
        '\u{0331}' => "b",
        '\u{0328}' => "k",
        '\u{030A}' => "r",
        _ => return None,
    })
}

fn letter_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "i" => "ı",
        "j" => "ȷ",
        "ae" => "æ",
        "AE" => "Æ",
        "oe" => "œ",
        "OE" => "Œ",
        "o" => "ø",
        "O" => "Ø",
        "l" => "ł",
        "L" => "Ł",
        "ss" => "ß",
        "aa" => "å",
        "AA" => "Å",
        "dh" => "ð",
        "DH" => "Ð",
        "th" => "þ",
        "TH" => "Þ",
        "ng" => "ŋ",
        "NG" => "Ŋ",
        _ => return None,
    })
}

fn letter_to_command(c: char) -> Option<&'static str> {
    Some(match c {
        'ı' => "\\i",
        'ȷ' => "\\j",
        'æ' => "\\ae",
        'Æ' => "\\AE",
        'œ' => "\\oe",
        'Œ' => "\\OE",
        'ø' => "\\o",
        'Ø' => "\\O",
        'ł' => "\\l",
        'Ł' => "\\L",
        'ß' => "\\ss",
        'ð' => "\\dh",
        'Ð' => "\\DH",
        'þ' => "\\th",
        'Þ' => "\\TH",
        'ŋ' => "\\ng",
        'Ŋ' => "\\NG",
        _ => return None,
    })
}

fn symbol_command(cmd: &str) -> Option<&'static str> {
    Some(match cmd {
        "ldots" | "dots" | "textellipsis" => "…",
        "textendash" => "–",
        "textemdash" => "—",
        "textquotedblleft" => "“",
        "textquotedblright" => "”",
        "textquoteleft" => "‘",
        "textquoteright" => "’",
        "textquotesingle" => "'",
        "slash" => "/",
        "S" => "§",
        "P" => "¶",
        "copyright" | "textcopyright" => "©",
        "textregistered" => "®",
        "textdegree" => "°",
        "pounds" | "textsterling" => "£",
        "euro" => "€",
        "par" | "newline" | "linebreak" | "quad" | "qquad" | "space" | "noindent" | "smallskip" | "medskip"
        | "bigskip" => " ",
        "relax" | "nobreak" | "unskip" | "hfill" | "protect" | "/" | "-" | "item" => "",
        _ => return None,
    })
}

/// Commands whose argument is italic in output.
fn italic_command(cmd: &str) -> bool {
    matches!(cmd, "emph" | "textit" | "mkbibemph" | "mkbibitalic" | "textsl" | "it" | "em")
}

/// Commands whose only argument is dropped entirely.
fn drop_arg_command(cmd: &str) -> bool {
    matches!(cmd, "footnote" | "index" | "label" | "nocite" | "hyphenation")
}

struct Scanner<'a> {
    c: Vec<char>,
    i: usize,
    _s: &'a str,
}

impl<'a> Scanner<'a> {
    fn new(s: &'a str) -> Self {
        Scanner { c: s.chars().collect(), i: 0, _s: s }
    }
    fn peek(&self) -> Option<char> {
        self.c.get(self.i).copied()
    }
    fn next(&mut self) -> Option<char> {
        let c = self.c.get(self.i).copied();
        self.i += 1;
        c
    }
    /// Command name after a backslash.
    fn command(&mut self) -> String {
        let mut name = String::new();
        match self.peek() {
            Some(c) if c.is_ascii_alphabetic() => {
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphabetic() {
                        name.push(c);
                        self.i += 1;
                    } else {
                        break;
                    }
                }
                // a control word swallows following spaces
                while self.peek() == Some(' ') {
                    self.i += 1;
                }
            }
            Some(c) => {
                name.push(c);
                self.i += 1;
            }
            None => {}
        }
        name
    }
    /// Raw text of a balanced group starting at '{' (excluding braces).
    fn group(&mut self) -> String {
        debug_assert_eq!(self.peek(), Some('{'));
        self.i += 1;
        let mut depth = 1;
        let mut out = String::new();
        while let Some(c) = self.next() {
            match c {
                '\\' => {
                    out.push('\\');
                    if let Some(n) = self.next() {
                        out.push(n);
                    }
                    continue;
                }
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            out.push(c);
        }
        out
    }
    fn skip_spaces(&mut self) {
        while self.peek() == Some(' ') {
            self.i += 1;
        }
    }
    /// Argument for an accent command: `{x}`, `\i`, or a single char.
    fn accent_arg(&mut self) -> String {
        self.skip_spaces();
        match self.peek() {
            Some('{') => {
                let g = self.group();
                decode(&g, Mode::Plain)
            }
            Some('\\') => {
                self.i += 1;
                let cmd = self.command();
                if let Some(l) = letter_command(&cmd) {
                    l.to_string()
                } else if let Some(m) = accent_mark(&cmd) {
                    let arg = self.accent_arg();
                    apply_mark(&arg, m)
                } else {
                    String::new()
                }
            }
            Some(_) => self.next().map(|c| c.to_string()).unwrap_or_default(),
            None => String::new(),
        }
    }
}

fn apply_mark(arg: &str, mark: char) -> String {
    let mut chars = arg.chars();
    let Some(first) = chars.next() else { return mark.to_string() };
    // dotless i/j take accents as plain i/j
    let base = match first {
        'ı' => 'i',
        'ȷ' => 'j',
        c => c,
    };
    let mut s = String::new();
    s.push(base);
    s.push(mark);
    let composed: String = s.nfc().collect();
    let mut out = composed;
    out.extend(chars);
    out
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Readable text: no braces, no markup.
    Plain,
    /// For the citation engine: case-protecting braces kept, italics become
    /// protected groups, literal specials escaped.
    Protected,
    /// For Pango markup: italics as <i>, text escaped.
    Markup,
}

fn decode(s: &str, mode: Mode) -> String {
    let mut sc = Scanner::new(s);
    let mut out = String::with_capacity(s.len());
    let lit = |out: &mut String, c: char| match mode {
        Mode::Protected if matches!(c, '{' | '}' | '$' | '\\') => {
            out.push('\\');
            out.push(c);
        }
        Mode::Markup => match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            _ => out.push(c),
        },
        _ => out.push(c),
    };
    while let Some(c) = sc.next() {
        match c {
            '\\' => {
                let cmd = sc.command();
                if cmd.is_empty() {
                    continue;
                }
                if let Some(m) = accent_mark(&cmd) {
                    let arg = sc.accent_arg();
                    for ch in apply_mark(&arg, m).chars() {
                        lit(&mut out, ch);
                    }
                } else if let Some(l) = letter_command(&cmd) {
                    out.push_str(l);
                } else if let Some(sym) = symbol_command(&cmd) {
                    out.push_str(sym);
                } else if matches!(cmd.as_str(), "&" | "%" | "$" | "#" | "_" | "{" | "}") {
                    lit(&mut out, cmd.chars().next().unwrap());
                } else if cmd == " " || cmd == "," || cmd == ";" || cmd == "@" {
                    if cmd == " " || cmd == ";" {
                        out.push(' ');
                    } else if cmd == "," {
                        out.push('\u{2009}');
                    }
                } else {
                    // generic command: optional [..] args then {..} args
                    sc.skip_spaces();
                    while sc.peek() == Some('[') {
                        while let Some(c) = sc.next() {
                            if c == ']' {
                                break;
                            }
                        }
                    }
                    let mut args = Vec::new();
                    while sc.peek() == Some('{') {
                        args.push(sc.group());
                        if !matches!(cmd.as_str(), "href" | "cite" | "citet" | "citep" | "citealt" | "parencite" | "textcite" | "autocite" | "footcite")
                            || args.len() >= 2
                        {
                            break;
                        }
                    }
                    if drop_arg_command(&cmd) {
                        continue;
                    }
                    let is_cite = cmd.ends_with("cite") || cmd.starts_with("cite");
                    let content = if cmd == "href" && args.len() >= 2 {
                        args[1].clone()
                    } else if is_cite {
                        // \cite[143]{key} -> key
                        args.last().cloned().unwrap_or_default()
                    } else {
                        args.first().cloned().unwrap_or_default()
                    };
                    let inner = decode(&content, mode);
                    if italic_command(&cmd) {
                        match mode {
                            Mode::Protected => {
                                out.push('{');
                                out.push_str(&inner);
                                out.push('}');
                            }
                            Mode::Markup => {
                                out.push_str("<i>");
                                out.push_str(&inner);
                                out.push_str("</i>");
                            }
                            Mode::Plain => out.push_str(&inner),
                        }
                    } else if cmd == "textbf" && mode == Mode::Markup {
                        out.push_str("<b>");
                        out.push_str(&inner);
                        out.push_str("</b>");
                    } else if cmd == "textsc" && mode == Mode::Markup {
                        out.push_str("<span variant=\"smallcaps\">");
                        out.push_str(&inner);
                        out.push_str("</span>");
                    } else if mode == Mode::Protected
                        && matches!(cmd.as_str(), "dev" | "textsc" | "textup" | "textrm" | "mbox" | "hbox" | "textnormal")
                    {
                        out.push('{');
                        out.push_str(&inner);
                        out.push('}');
                    } else {
                        out.push_str(&inner);
                    }
                }
            }
            '{' => {
                sc.i -= 1;
                let g = sc.group();
                let inner = decode(&g, mode);
                if mode == Mode::Protected {
                    out.push('{');
                    out.push_str(&inner);
                    out.push('}');
                } else {
                    out.push_str(&inner);
                }
            }
            '}' => {}
            '~' => out.push('\u{00A0}'),
            '-' if sc.peek() == Some('-') => {
                sc.i += 1;
                if sc.peek() == Some('-') {
                    sc.i += 1;
                    out.push('—');
                } else {
                    out.push('–');
                }
            }
            '`' if sc.peek() == Some('`') => {
                sc.i += 1;
                out.push('“');
            }
            '\'' if sc.peek() == Some('\'') => {
                sc.i += 1;
                out.push('”');
            }
            '$' => {} // math delimiters dropped
            '\n' | '\r' | '\t' => {
                if !out.ends_with(' ') {
                    out.push(' ')
                }
            }
            c => lit(&mut out, c),
        }
    }
    let n: String = out.nfc().collect();
    // collapse runs of spaces
    let mut res = String::with_capacity(n.len());
    let mut prev_space = false;
    for ch in n.chars() {
        if ch == ' ' {
            if !prev_space {
                res.push(' ');
            }
            prev_space = true;
        } else {
            res.push(ch);
            prev_space = false;
        }
    }
    res.trim().to_string()
}

/// Readable Unicode text from a raw LaTeX value.
pub fn to_plain(s: &str) -> String {
    let needs_work = s.contains(['\\', '{', '}', '~', '$', '\n', '\r', '\t'])
        || s.contains("--")
        || s.contains("``")
        || s.contains("''")
        || s.contains("  ")
        || !s.is_ascii() && !unicode_normalization::is_nfc(s);
    if !needs_work {
        return s.trim().to_string();
    }
    decode(s, Mode::Plain)
}

/// Text for the citation engine: case protection kept as `{}`.
pub fn to_protected(s: &str) -> String {
    decode(s, Mode::Protected)
}

/// Pango markup with italics.
pub fn to_markup(s: &str) -> String {
    decode(s, Mode::Markup)
}

/// Rewrite LaTeX accent and special-letter commands as Unicode characters,
/// leaving every other command untouched. Redundant protective braces that
/// only wrapped an accent (`{\=a}`) are removed.
pub fn accents_to_unicode(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        // `{\=a}` style: brace group containing exactly one accent construct
        if chars[i] == '{' && chars.get(i + 1) == Some(&'\\') {
            if let Some((text, len)) = try_accent(&chars[i + 1..]) {
                let after = i + 1 + len;
                if chars.get(after) == Some(&'}') {
                    out.push_str(&text);
                    i = after + 1;
                    continue;
                }
            }
        }
        if chars[i] == '\\' {
            if let Some((text, len)) = try_accent(&chars[i..]) {
                out.push_str(&text);
                i += len;
                continue;
            }
            // keep escaped char pairs intact
            out.push('\\');
            if let Some(&n) = chars.get(i + 1) {
                out.push(n);
            }
            i += 2;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out.nfc().collect()
}

/// Try to read one accent or letter command at `c[0] == '\\'`.
/// Returns the Unicode text and number of chars consumed.
fn try_accent(c: &[char]) -> Option<(String, usize)> {
    if c.first() != Some(&'\\') {
        return None;
    }
    let mut i = 1;
    let mut name = String::new();
    match c.get(i) {
        Some(ch) if ch.is_ascii_alphabetic() => {
            while let Some(ch) = c.get(i) {
                if ch.is_ascii_alphabetic() {
                    name.push(*ch);
                    i += 1;
                } else {
                    break;
                }
            }
        }
        Some(ch) => {
            name.push(*ch);
            i += 1;
        }
        None => return None,
    }
    if let Some(l) = letter_command(&name) {
        // `\i` must not be followed by a letter (would be a different command)
        let mut j = i;
        // eat a following `{}` or single space terminator
        if c.get(j) == Some(&'{') && c.get(j + 1) == Some(&'}') {
            j += 2;
        } else if c.get(j) == Some(&' ') {
            j += 1;
        }
        return Some((l.to_string(), j));
    }
    let mark = accent_mark(&name)?;
    while c.get(i) == Some(&' ') {
        i += 1;
    }
    let (base, used) = match c.get(i) {
        Some('{') => {
            // find matching brace
            let mut depth = 0;
            let mut j = i;
            let mut inner = String::new();
            loop {
                let ch = *c.get(j)?;
                if ch == '{' {
                    depth += 1;
                    if depth > 1 {
                        inner.push(ch);
                    }
                } else if ch == '}' {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    inner.push(ch);
                } else {
                    inner.push(ch);
                }
                j += 1;
            }
            let base = accents_to_unicode(&inner);
            // only convert single-letter arguments
            if base.nfc().count() != 1 {
                return None;
            }
            (base, j + 1 - i)
        }
        Some('\\') => {
            let (t, len) = try_accent(&c[i..])?;
            (t, len)
        }
        // `\d h` needs the space; `\dh` is a different command (handled above)
        Some(ch) if ch.is_alphabetic() => (ch.to_string(), 1),
        _ => return None,
    };
    Some((apply_mark(&base, mark), i + used))
}

/// Replace non-ASCII letters by LaTeX accent commands (for BibTeX/pdfLaTeX).
/// ASCII text, including existing LaTeX, is left untouched.
pub fn unicode_to_latex(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 16);
    for ch in s.nfc() {
        if ch.is_ascii() {
            out.push(ch);
            continue;
        }
        if let Some(cmd) = letter_to_command(ch) {
            out.push('{');
            out.push_str(cmd);
            out.push('}');
            continue;
        }
        match ch {
            '–' => {
                out.push_str("--");
                continue;
            }
            '—' => {
                out.push_str("---");
                continue;
            }
            '\u{00A0}' => {
                out.push('~');
                continue;
            }
            _ => {}
        }
        let mut parts = Vec::new();
        decompose_canonical(ch, |c| parts.push(c));
        let base = parts[0];
        let marks = &parts[1..];
        if !marks.is_empty() && base.is_ascii_alphabetic() && marks.iter().all(|m| mark_command(*m).is_some()) {
            // build inside-out: marks closest to the base first
            let mut inner = match base {
                'i' if marks.iter().any(|m| !matches!(*m, '\u{0323}' | '\u{0331}' | '\u{0327}' | '\u{0328}')) => {
                    "\\i".to_string()
                }
                _ => base.to_string(),
            };
            for m in marks {
                let cmd = mark_command(*m).unwrap();
                inner = if cmd.chars().all(|c| c.is_ascii_alphabetic()) {
                    format!("\\{cmd}{{{inner}}}")
                } else if inner.chars().count() == 1 {
                    format!("\\{cmd}{inner}")
                } else {
                    format!("\\{cmd}{{{inner}}}")
                };
            }
            out.push('{');
            out.push_str(&inner);
            out.push('}');
        } else {
            out.push(ch);
        }
    }
    out
}

/// True if the text contains LaTeX accent commands that could be Unicode.
pub fn has_latex_accents(s: &str) -> bool {
    s.contains('\\') && accents_to_unicode(s) != s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain() {
        assert_eq!(to_plain(r"{\=a}yurveda"), "āyurveda");
        assert_eq!(to_plain(r"\'{e}t\'e"), "été");
        assert_eq!(to_plain(r"{\={\d{r}}}"), "ṝ");
        assert_eq!(to_plain(r"\d{r}\={\i}"), "ṛī");
        assert_eq!(to_plain(r"Wiley \& Sons"), "Wiley & Sons");
        assert_eq!(to_plain(r"\emph{aduruktav\=akyam} in its"), "aduruktavākyam in its");
        assert_eq!(to_plain(r"pp.~10--20"), "pp.\u{a0}10–20");
        assert_eq!(to_plain(r"\href{http://x}{link text}"), "link text");
        assert_eq!(to_plain(r"\cite[143]{bend-bmcat}"), "bend-bmcat");
        assert_eq!(to_plain(r"\dev{nāgeśa}. A Work"), "nāgeśa. A Work");
        assert_eq!(to_plain(r"{\ae}sop \ss"), "æsop ß");
    }

    #[test]
    fn protected() {
        assert_eq!(to_protected(r"The \emph{Carakasaṃhitā} and {DNA}"), "The {Carakasaṃhitā} and {DNA}");
        assert_eq!(to_protected(r"cost \$5"), r"cost \$5");
    }

    #[test]
    fn markup() {
        assert_eq!(to_markup(r"A \emph{B} & C"), "A <i>B</i> &amp; C");
    }

    #[test]
    fn to_unicode_preserves_other_latex() {
        assert_eq!(accents_to_unicode(r"\emph{{\=a}yurveda} \cite{x}"), r"\emph{āyurveda} \cite{x}");
        assert_eq!(accents_to_unicode(r"\'{e}t\'e \c c \c{s}"), "été ç ş");
        assert_eq!(accents_to_unicode(r"{\={\d{r}}}"), "ṝ");
        assert_eq!(accents_to_unicode(r"\={\i}"), "ī");
        assert_eq!(accents_to_unicode(r"Wiley \& Sons"), r"Wiley \& Sons");
        assert_eq!(accents_to_unicode(r"\url{http://x}"), r"\url{http://x}");
        assert_eq!(accents_to_unicode(r"\dev{x} \d{h}"), r"\dev{x} ḥ");
    }

    #[test]
    fn to_latex() {
        assert_eq!(unicode_to_latex("āyurveda"), r"{\=a}yurveda");
        assert_eq!(unicode_to_latex("ṛṝ"), r"{\d{r}}{\={\d{r}}}");
        assert_eq!(unicode_to_latex("ī"), r"{\={\i}}");
        assert_eq!(unicode_to_latex("Śiva"), r"{\'S}iva");
        assert_eq!(unicode_to_latex("ṃḥ"), r"{\d{m}}{\d{h}}");
        // round trip
        for w in ["Carakasaṃhitā", "Pāṇini", "Ṛgveda", "Mānava-Dharmaśāstra", "kḷpta", "ñāṇa", "Āṅgirasa"] {
            assert_eq!(accents_to_unicode(&unicode_to_latex(w)), w);
            assert_eq!(to_plain(&unicode_to_latex(w)), w);
        }
    }
}
