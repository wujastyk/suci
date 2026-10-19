//! BibTeX name lists ("A and B and others").

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Name {
    pub first: String,
    pub von: String,
    pub last: String,
    pub jr: String,
    /// Corporate or otherwise protected name, e.g. `{World Health Organization}`.
    pub literal: bool,
}

impl Name {
    /// "Wujastyk, Dominik" style.
    pub fn last_first(&self) -> String {
        let mut s = String::new();
        if !self.von.is_empty() {
            s.push_str(&self.von);
            s.push(' ');
        }
        s.push_str(&self.last);
        if !self.jr.is_empty() {
            s.push_str(", ");
            s.push_str(&self.jr);
        }
        if !self.first.is_empty() {
            s.push_str(", ");
            s.push_str(&self.first);
        }
        s
    }

    pub fn first_last(&self) -> String {
        let mut parts = Vec::new();
        for p in [&self.first, &self.von, &self.last] {
            if !p.is_empty() {
                parts.push(p.as_str());
            }
        }
        let mut s = parts.join(" ");
        if !self.jr.is_empty() {
            s.push_str(", ");
            s.push_str(&self.jr);
        }
        s
    }

    pub fn is_others(&self) -> bool {
        self.last == "others" && self.first.is_empty()
    }
}

/// Split on top-level `and`.
pub fn split_names(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let words: Vec<&str> = s.split_whitespace().collect();
    // re-tokenise while tracking depth
    for w in words {
        let d_before = depth;
        for c in w.chars() {
            match c {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
        }
        if d_before == 0 && w.eq_ignore_ascii_case("and") {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
}

fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if c == sep && depth == 0 {
            out.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

fn words_top(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
        if (c.is_whitespace() || c == '~') && depth == 0 {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_lower_word(w: &str) -> bool {
    // first letter outside braces / after commands
    let plain = crate::latex::to_plain(w);
    if w.starts_with('{') {
        return false;
    }
    plain.chars().find(|c| c.is_alphabetic()).map(|c| c.is_lowercase()).unwrap_or(false)
}

/// Parse one name; values stay in raw LaTeX form.
pub fn parse_name(s: &str) -> Name {
    let s = s.trim();
    if s.starts_with('{') && s.ends_with('}') && split_top(&s[1..s.len() - 1], ',').len() >= 1 {
        // whole name protected?
        let mut depth = 0;
        let mut closes_at_end = true;
        for (i, c) in s.char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 && i != s.len() - 1 {
                        closes_at_end = false;
                    }
                }
                _ => {}
            }
        }
        if closes_at_end {
            return Name { last: s[1..s.len() - 1].to_string(), literal: true, ..Default::default() };
        }
    }
    let parts = split_top(s, ',');
    let mut n = Name::default();
    match parts.len() {
        1 => {
            let w = words_top(&parts[0]);
            if w.len() == 1 {
                n.last = w[0].clone();
                return n;
            }
            // First von Last: last word is Last; von = first lowercase run after first word
            let last_idx = w.len() - 1;
            let mut von_start = None;
            let mut von_end = None;
            for (i, word) in w.iter().enumerate().take(last_idx) {
                if is_lower_word(word) {
                    if von_start.is_none() {
                        von_start = Some(i);
                    }
                    von_end = Some(i);
                }
            }
            match (von_start, von_end) {
                (Some(a), Some(b)) => {
                    n.first = w[..a].join(" ");
                    n.von = w[a..=b].join(" ");
                    n.last = w[b + 1..].join(" ");
                }
                _ => {
                    n.first = w[..last_idx].join(" ");
                    n.last = w[last_idx].clone();
                }
            }
        }
        _ => {
            let (vonlast, jr, first) = if parts.len() == 2 {
                (parts[0].clone(), String::new(), parts[1].clone())
            } else {
                (parts[0].clone(), parts[1].clone(), parts[2..].join(", "))
            };
            let w = words_top(&vonlast);
            let mut i = 0;
            while i + 1 < w.len() && is_lower_word(&w[i]) {
                i += 1;
            }
            n.von = w[..i].join(" ");
            n.last = w[i..].join(" ");
            n.jr = jr;
            n.first = first;
        }
    }
    n
}

pub fn parse_names(s: &str) -> Vec<Name> {
    split_names(s).iter().map(|x| parse_name(x)).collect()
}

/// Short display: "Wujastyk", "Epimakhov & Lubotsky", "Ajotikar et al."
pub fn short_display(s: &str) -> String {
    let names = parse_names(s);
    let lasts: Vec<String> = names
        .iter()
        .filter(|n| !n.is_others())
        .map(|n| {
            let mut l = String::new();
            if !n.von.is_empty() {
                l.push_str(&n.von);
                l.push(' ');
            }
            l.push_str(&n.last);
            crate::latex::to_plain(&l)
        })
        .collect();
    let others = names.iter().any(|n| n.is_others());
    match lasts.len() {
        0 => String::new(),
        1 if !others => lasts[0].clone(),
        2 if !others => format!("{} & {}", lasts[0], lasts[1]),
        _ => format!("{} et al.", lasts[0]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        let n = parse_name("Hans Henrich Hock");
        assert_eq!((n.first.as_str(), n.last.as_str()), ("Hans Henrich", "Hock"));
        let n = parse_name("Hinüber, Oskar von");
        assert_eq!((n.first.as_str(), n.last.as_str()), ("Oskar von", "Hinüber"));
        let n = parse_name("Ludwig van Beethoven");
        assert_eq!((n.first.as_str(), n.von.as_str(), n.last.as_str()), ("Ludwig", "van", "Beethoven"));
        let n = parse_name("van Buitenen, J. A. B.");
        assert_eq!((n.von.as_str(), n.last.as_str(), n.first.as_str()), ("van", "Buitenen", "J. A. B."));
        let n = parse_name("{World Health Organization}");
        assert!(n.literal);
        assert_eq!(n.last, "World Health Organization");
        let n = parse_name("Smith, Jr., John");
        assert_eq!((n.last.as_str(), n.jr.as_str(), n.first.as_str()), ("Smith", "Jr.", "John"));
        assert_eq!(split_names("A and {B and C} and D").len(), 3);
        assert_eq!(short_display("Tanuja P. Ajotikar and Anuja P. Ajotikar and Peter M. Scharf"), "Ajotikar et al.");
        assert_eq!(short_display("Andrey Epimakhov and Alexander Lubotsky"), "Epimakhov & Lubotsky");
        assert_eq!(short_display("A. Smith and others"), "Smith et al.");
        assert_eq!(short_display("Wuj{\\=a}styk, D."), "Wujāstyk");
    }
}
