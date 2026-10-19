//! English title capitalisation ("headline style", after Chicago 8.159).
//!
//! Conservative by design:
//! * only the first letter of a word is ever raised; the rest is untouched
//!   (McGill, iPhone, al-Bīrūnī stay as they are);
//! * articles, conjunctions and prepositions are lowered unless first, last,
//!   or after a colon / dash / full stop;
//! * acronyms, words with digits, URLs and abbreviations are left alone;
//! * anything in braces or inside a LaTeX command (`\emph{…}`, `\dev{…}`,
//!   `\textbengali{…}`) and math is copied verbatim;
//! * name particles (von, de, al …) are left as written.

use crate::bib::Entry;

const SMALL: &[&str] = &[
    "a", "an", "the", "and", "but", "or", "nor", "for", "as", "at", "by", "in", "of", "off", "on", "per", "to",
    "up", "via", "vs", "vs.", "v.", "from", "into", "onto", "upon", "with", "over", "under", "than", "till",
    "unto", "among", "amid", "amidst", "amongst", "about", "above", "across", "after", "against", "along",
    "around", "before", "behind", "below", "beneath", "beside", "besides", "between", "beyond", "during",
    "except", "inside", "outside", "through", "throughout", "toward", "towards", "within", "without", "versus",
    "vis-à-vis", "vis-a-vis",
];

fn starts_lower(s: &str) -> bool {
    s.chars().find(|c| c.is_alphabetic()).is_some_and(|c| c.is_lowercase())
}

/// Name particles and transliterated articles: never changed (except as first word).
const PARTICLES: &[&str] = &[
    "von", "van", "der", "den", "de", "del", "della", "di", "du", "da", "dos", "das", "des", "la", "le", "ter",
    "ten", "bin", "ibn", "ben", "al", "el", "zu", "y", "e", "et", "und",
];

/// Prefixes after which the second element of a compound is left alone.
const PREFIXES: &[&str] = &[
    "anti", "pre", "post", "non", "re", "co", "sub", "semi", "multi", "inter", "intra", "pseudo", "proto",
    "quasi", "neo", "mid", "proto", "ultra", "trans",
];

/// Fields treated as titles.
pub const TITLE_FIELDS: &[&str] = &[
    "title", "subtitle", "booktitle", "booksubtitle", "maintitle", "mainsubtitle", "journaltitle",
    "journalsubtitle", "issuetitle", "issuesubtitle", "series",
];

#[derive(Debug)]
enum Seg {
    Text(String),
    Protected(String),
}

/// Split raw LaTeX into plain text and verbatim (protected) parts.
fn segments(s: &str) -> Vec<Seg> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut text = String::new();
    let mut i = 0;
    let flush = |text: &mut String, out: &mut Vec<Seg>| {
        if !text.is_empty() {
            out.push(Seg::Text(std::mem::take(text)));
        }
    };
    let group_end = |start: usize| -> usize {
        // index after the matching '}' for '{' at start
        let mut depth = 0;
        let mut j = start;
        while j < c.len() {
            match c[j] {
                '\\' => j += 1,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return j + 1;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        c.len()
    };
    while i < c.len() {
        match c[i] {
            '{' => {
                flush(&mut text, &mut out);
                let e = group_end(i);
                out.push(Seg::Protected(c[i..e].iter().collect()));
                i = e;
            }
            '$' => {
                flush(&mut text, &mut out);
                let e = c[i + 1..].iter().position(|&x| x == '$').map(|p| i + p + 2).unwrap_or(c.len());
                out.push(Seg::Protected(c[i..e].iter().collect()));
                i = e;
            }
            '\\' => {
                let mut j = i + 1;
                if j < c.len() && c[j].is_ascii_alphabetic() {
                    while j < c.len() && c[j].is_ascii_alphabetic() {
                        j += 1;
                    }
                } else {
                    j = (j + 1).min(c.len());
                }
                let name: String = c[i + 1..j].iter().collect();
                // escaped punctuation (\&, \%) is ordinary text
                if matches!(name.as_str(), "&" | "%" | "#" | "_" | "$") {
                    text.extend(&c[i..j]);
                    i = j;
                    continue;
                }
                flush(&mut text, &mut out);
                // command with optional [..] and {..} arguments
                let mut k = j;
                loop {
                    let mut m = k;
                    while m < c.len() && c[m] == ' ' && name.chars().all(|x| x.is_ascii_alphabetic()) {
                        m += 1;
                    }
                    if m < c.len() && c[m] == '[' {
                        k = c[m..].iter().position(|&x| x == ']').map(|p| m + p + 1).unwrap_or(c.len());
                    } else if m < c.len() && c[m] == '{' {
                        k = group_end(m);
                    } else {
                        break;
                    }
                }
                out.push(Seg::Protected(c[i..k].iter().collect()));
                i = k;
            }
            ch => {
                text.push(ch);
                i += 1;
            }
        }
    }
    flush(&mut text, &mut out);
    out
}

fn is_open_punct(c: char) -> bool {
    matches!(c, '(' | '[' | '"' | '\'' | '“' | '‘' | '«' | '‹' | '`' | '¿' | '¡')
}

fn is_close_punct(c: char) -> bool {
    matches!(c, ')' | ']' | '"' | '\'' | '”' | '’' | '»' | '›' | ',' | ';' | ':' | '.' | '?' | '!' | '…')
}

fn upper_first(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut done = false;
    for ch in s.chars() {
        if !done && ch.is_alphabetic() {
            out.extend(ch.to_uppercase());
            done = true;
        } else {
            out.push(ch);
        }
    }
    out
}

fn lower_first(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut done = false;
    for ch in s.chars() {
        if !done && ch.is_alphabetic() {
            out.extend(ch.to_lowercase());
            done = true;
        } else {
            out.push(ch);
        }
    }
    out
}

/// Should this word be left exactly as written?
fn keep_as_is(core: &str) -> bool {
    let letters: Vec<char> = core.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() || core.chars().any(|c| c.is_ascii_digit()) {
        return true;
    }
    if core.contains("://") || core.starts_with("www.") || core.contains('@') {
        return true;
    }
    // e.g., i.e., U.S.
    if core.trim_end_matches('.').contains('.') {
        return true;
    }
    // acronyms and internal capitals: DNA, iPhone, McGill
    if letters.len() > 1 && letters.iter().all(|c| c.is_uppercase()) {
        return true;
    }
    if letters.iter().skip(1).any(|c| c.is_uppercase()) {
        return true;
    }
    // scripts without case (Devanagari, Bengali …)
    if !letters.iter().any(|c| c.is_lowercase() || c.is_uppercase()) {
        return true;
    }
    false
}

/// Capitalise one word (which may be hyphenated).
fn has_iast(s: &str) -> bool {
    s.chars().any(|c| "āīūṛṝḷḹṃṁḥṅñṭḍṇśṣĀĪŪṚṜḶṂḤṄÑṬḌṆŚṢ".contains(c))
}

fn cap_word(word: &str, first: bool, last: bool, keep_lower_iast: bool) -> String {
    // split off leading/trailing punctuation
    let start = word.char_indices().find(|(_, c)| !is_open_punct(*c)).map(|(i, _)| i).unwrap_or(word.len());
    let end = word
        .char_indices()
        .rev()
        .find(|(_, c)| !is_close_punct(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(start)
        .max(start);
    let (pre, core, post) = (&word[..start], &word[start..end], &word[end..]);
    if core.is_empty() || keep_as_is(core) {
        return word.to_string();
    }
    // cited Sanskrit stems ("bhojya-") and, in an already capitalised title,
    // lowercase Sanskrit terms were written that way on purpose
    if !first && (core.ends_with('-') && starts_lower(core) || keep_lower_iast && has_iast(core) && starts_lower(core)) {
        return word.to_string();
    }
    if SMALL.contains(&core.to_lowercase().as_str()) && core.contains('-') {
        return if first || last { format!("{pre}{}{post}", upper_first(core)) } else { format!("{pre}{}{post}", core.to_lowercase()) };
    }
    let parts: Vec<&str> = core.split('-').collect();
    let mut out = String::new();
    let first_lower = parts[0].to_lowercase();
    for (n, part) in parts.iter().enumerate() {
        if n > 0 {
            out.push('-');
        }
        let lower = part.to_lowercase();
        let is_first = first && n == 0;
        let is_last = last && n + 1 == parts.len();
        let new = if part.is_empty() || keep_as_is(part) {
            part.to_string()
        } else if n > 0 && PREFIXES.contains(&first_lower.as_str()) {
            part.to_string()
        } else if n > 0 && keep_lower_iast && starts_lower(part) {
            // the author chose lower case after the hyphen: Nyāya-vaiśeṣika,
            // Yoga-sūtras, Socio-cultural, Hsüan-tsang
            part.to_string()
        } else if PARTICLES.contains(&lower.as_str()) && !is_first {
            part.to_string()
        } else if SMALL.contains(&lower.as_str()) && is_last && !is_first && post.starts_with(':') {
            // "Review of: …" — a label, leave as written
            part.to_string()
        } else if SMALL.contains(&lower.as_str()) && !is_first && !is_last {
            // long prepositions are often adverbs or nouns ("the Inside of"):
            // leave them as written
            if lower.chars().count() > 4 { part.to_string() } else { lower_first(part) }
        } else {
            upper_first(part)
        };
        out.push_str(&new);
    }
    format!("{pre}{out}{post}")
}

/// Capitalise a title, in raw LaTeX form.
/// `continues` = the text continues a previous field (e.g. a subtitle is not
/// the start of a sentence for the purposes of this function).
pub fn title_case(s: &str) -> String {
    let segs = segments(s);
    // whole title in capitals → lower it first (but keep protected parts)
    let all_caps = {
        let letters: String = segs
            .iter()
            .filter_map(|g| if let Seg::Text(t) = g { Some(t.as_str()) } else { None })
            .flat_map(|t| t.chars())
            .filter(|c| c.is_alphabetic())
            .collect();
        letters.chars().count() > 12 && letters.chars().all(|c| !c.is_lowercase())
    };
    // collect word tokens with their segment positions
    struct Tok {
        seg: usize,
        start: usize,
        end: usize,
        /// glued to protected text before it (e.g. `{\=a}yurveda`)
        glued: bool,
    }
    let mut toks: Vec<Tok> = Vec::new();
    let mut texts: Vec<String> = segs
        .iter()
        .map(|g| match g {
            Seg::Text(t) => {
                if all_caps {
                    t.to_lowercase()
                } else {
                    t.clone()
                }
            }
            Seg::Protected(p) => p.clone(),
        })
        .collect();
    let mut prev_was_space = true;
    for (si, g) in segs.iter().enumerate() {
        match g {
            Seg::Protected(_) => prev_was_space = false,
            Seg::Text(_) => {
                let t = &texts[si];
                let mut idx = 0;
                let bytes: Vec<(usize, char)> = t.char_indices().collect();
                let mut k = 0;
                while k < bytes.len() {
                    let (bi, ch) = bytes[k];
                    if ch.is_whitespace() {
                        prev_was_space = true;
                        k += 1;
                        continue;
                    }
                    let start = bi;
                    let glued = !prev_was_space;
                    while k < bytes.len() && !bytes[k].1.is_whitespace() {
                        k += 1;
                    }
                    let end = if k < bytes.len() { bytes[k].0 } else { t.len() };
                    toks.push(Tok { seg: si, start, end, glued });
                    prev_was_space = false;
                    idx += 1;
                }
                let _ = idx;
                if t.ends_with(char::is_whitespace) {
                    prev_was_space = true;
                }
            }
        }
    }
    // a protected segment followed by text without a space glues the next word
    let n = toks.len();
    let mut new_words: Vec<Option<String>> = vec![None; n];
    // share of ordinary words that already start with a capital
    let mostly_capitalised = {
        let words: Vec<&str> = toks
            .iter()
            .map(|t| &texts[t.seg][t.start..t.end])
            .map(|w| w.trim_matches(|c: char| !c.is_alphabetic()))
            .filter(|w| w.chars().count() > 2 && !SMALL.contains(&w.to_lowercase().as_str()) && !PARTICLES.contains(&w.to_lowercase().as_str()))
            .collect();
        let caps = words.iter().filter(|w| w.starts_with(char::is_uppercase)).count();
        !words.is_empty() && caps * 10 >= words.len() * 7
    };
    let mut sentence_start = true;
    let mut last_seg = usize::MAX;
    for (ti, tok) in toks.iter().enumerate() {
        // protected text (e.g. an italic Sanskrit title) between words counts as a word
        if last_seg != usize::MAX && (last_seg + 1..tok.seg).any(|s| matches!(segs[s], Seg::Protected(_))) {
            sentence_start = false;
        }
        if last_seg == usize::MAX && (0..tok.seg).any(|s| matches!(segs[s], Seg::Protected(_))) {
            sentence_start = false;
        }
        last_seg = tok.seg;
        let word = &texts[tok.seg][tok.start..tok.end];
        // is there a protected segment directly after this word (no space)?
        let glued_after = tok.end == texts[tok.seg].len() && matches!(segs.get(tok.seg + 1), Some(Seg::Protected(_)));
        // last word of the title: nothing (not even protected text) follows
        let at_end = ti + 1 == n
            && texts[tok.seg][tok.end..].trim().is_empty()
            && !segs[tok.seg + 1..].iter().any(|g| matches!(g, Seg::Protected(p) if !p.trim().is_empty()));
        let last = at_end
            || word.ends_with(':')
            || word.ends_with('—')
            || word.ends_with('?')
            || word.ends_with('!');
        let first = sentence_start;
        // "…, The Golden Road": a capitalised article after a comma or an
        // opening parenthesis usually begins an embedded title; keep it
        let prev = if ti > 0 { &texts[toks[ti - 1].seg][toks[ti - 1].start..toks[ti - 1].end] } else { "" };
        let article = matches!(word.trim_start_matches(|c: char| is_open_punct(c)), "The" | "A" | "An");
        let keep_article = article && (prev.ends_with(',') || word.starts_with(|c: char| is_open_punct(c)));
        if !tok.glued && !glued_after && !keep_article && !matches!(word, "—" | "–" | "-" | "--" | "---" | "=") {
            new_words[ti] = Some(cap_word(word, first, last, mostly_capitalised));
        }
        let core = word.trim_end_matches(|c: char| is_close_punct(c));
        let abbreviation = {
            let letters = core.chars().filter(|c| c.is_alphabetic()).count();
            core.contains('.') || (letters > 0 && letters <= 3 && core.chars().next().is_some_and(|c| c.is_uppercase()))
        };
        sentence_start = word.ends_with(':')
            || word.ends_with('?')
            || word.ends_with('!')
            || word.ends_with('—')
            || word == "–"
            || word == "--"
            || word == "---"
            || word == "="
            || (word.trim_end_matches(['”', '’', '"', '\'', ')']).ends_with('.') && !word.ends_with("..") && !abbreviation);
    }
    // rebuild, replacing tokens from the back of each segment
    for ti in (0..n).rev() {
        if let Some(w) = &new_words[ti] {
            let tok = &toks[ti];
            texts[tok.seg].replace_range(tok.start..tok.end, w);
        }
    }
    texts.concat()
}

// ---------- which entries are English ----------

const ENGLISH_IDS: &[&str] =
    &["english", "american", "british", "canadian", "australian", "newzealand", "ukenglish", "usenglish", "en", "eng"];

/// Language tag of an entry: Some(true) English, Some(false) other, None untagged.
pub fn tagged_english(e: &Entry) -> Option<bool> {
    let check = |v: &str| {
        let v = crate::latex::to_plain(v).to_lowercase();
        let v = v.trim().trim_start_matches('{');
        if v.is_empty() || v == "unspecified" {
            return None;
        }
        let english = ENGLISH_IDS.iter().any(|id| {
            v == *id
                || v.starts_with(&format!("{id};"))
                || v.starts_with(&format!("{id},"))
                || v.starts_with(&format!("{id}-"))
                || v.starts_with("english")
                || v.contains("english")
        });
        Some(english)
    };
    if let Some(l) = e.get_nonempty("langid") {
        if let Some(r) = check(&l) {
            return Some(r);
        }
    }
    if let Some(l) = e.get_nonempty("language") {
        if let Some(r) = check(&l) {
            return Some(r);
        }
    }
    None
}

const INDIC_COMMANDS: &[&str] = &[
    "\\dev", "\\textdevanagari", "\\devanagari", "\\textbengali", "\\bengali", "\\textsanskrit", "\\texthindi",
    "\\texttamil", "\\texttelugu", "\\textkannada", "\\textmalayalam", "\\textgujarati", "\\textgurmukhi",
    "\\textoriya", "\\textsinhala", "\\texttibetan", "\\textmarathi", "\\textnepali", "\\skt", "\\cjk",
    "\\textarabic", "\\textchinese", "\\textjapanese",
];

/// Contains text in a script tag or an uncased script (Devanagari, Bengali, CJK …).
pub fn has_script_text(s: &str) -> bool {
    if INDIC_COMMANDS.iter().any(|c| {
        s.match_indices(c).any(|(i, _)| !s[i + c.len()..].starts_with(|ch: char| ch.is_ascii_alphabetic()))
    }) {
        return true;
    }
    s.chars().any(|c| matches!(c as u32, 0x0590..=0x08FF | 0x0900..=0x0DFF | 0x0F00..=0x0FFF | 0x3040..=0x9FFF | 0xAC00..=0xD7AF))
}

const EN_WORDS: &[&str] = &[
    "the", "of", "and", "in", "on", "to", "a", "an", "for", "with", "from", "by", "its", "as", "at", "is", "or",
    "into", "towards", "toward", "between", "among", "their", "his", "her", "new", "study", "studies", "history",
    "notes", "some", "early", "ancient", "medieval", "india", "indian", "what", "how", "why", "against", "after",
];
const FOREIGN_WORDS: &[&str] = &[
    "und", "der", "die", "das", "des", "dem", "den", "zur", "zum", "über", "uber", "eine", "ein", "einer", "im",
    "mit", "von", "zu", "et", "le", "la", "les", "du", "dans", "pour", "sur", "une", "un", "aux", "au", "il",
    "della", "delle", "degli", "dei", "di", "nel", "nella", "e", "del", "los", "las", "el", "y", "con", "het",
    "een", "en", "och", "ka", "kī", "kā", "ke", "aur", "evaṃ", "de", "da", "na", "em", "ao", "pela", "pelo",
    "sobre", "também", "auf", "aus", "bei", "nach", "vom", "zwischen", "über", "dans", "chez", "entre", "avec",
];

/// Heuristic for untagged titles: mostly English function words?
pub fn looks_english(title: &str) -> bool {
    let plain = crate::latex::to_plain(title).to_lowercase();
    let count = |s: &str| {
        let words: Vec<&str> = s.split(|c: char| !c.is_alphanumeric() && c != '\'' && c != '’').filter(|w| !w.is_empty()).collect();
        (words.iter().filter(|w| EN_WORDS.contains(w)).count(), words.iter().filter(|w| FOREIGN_WORDS.contains(w)).count())
    };
    // the main title (before a colon, bracket or parallel title) decides
    let main = plain.split([':', '[', '=', '.']).next().unwrap_or("");
    let (en_main, fo_main) = count(main);
    let (en, fo) = count(&plain);
    if fo_main > 0 {
        return en_main > fo_main;
    }
    en > 0 && en > fo
}

/// Should automatic (library-wide) capitalisation touch this entry?
pub fn auto_applies(e: &Entry) -> bool {
    if TITLE_FIELDS.iter().filter_map(|f| e.get_nonempty(f)).any(|t| has_script_text(&t)) {
        return false;
    }
    match tagged_english(e) {
        Some(english) => english,
        None => e.get_nonempty("title").is_some_and(|t| looks_english(&t)),
    }
}

/// Capitalise all title fields of an entry. Returns true if anything changed.
pub fn apply(e: &mut Entry) -> bool {
    let mut changed = false;
    for f in TITLE_FIELDS {
        if let Some(v) = e.get_nonempty(f) {
            let n = title_case(&v);
            if n != v {
                e.set(f, &n);
                changed = true;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bib::Library;

    #[test]
    fn basics() {
        let t = title_case;
        assert_eq!(t("the science of medicine"), "The Science of Medicine");
        assert_eq!(
            t("Fire and water: the bronze age of the southern urals and the rigveda"),
            "Fire and Water: The Bronze Age of the Southern Urals and the Rigveda"
        );
        assert_eq!(t("A work on variations in the sūtras of the aṣṭādhyāyī"), "A Work on Variations in the Sūtras of the Aṣṭādhyāyī");
        assert_eq!(t("Medicine In The Veda"), "Medicine in the Veda");
        assert_eq!(t("what is it good for?"), "What Is It Good For?");
        assert_eq!(t("women's rituals and megasthenes’ stories"), "Women's Rituals and Megasthenes’ Stories");
        assert_eq!(t("self-sustaining reactions"), "Self-Sustaining Reactions");
        assert_eq!(t("anti-intellectualism in the west"), "Anti-intellectualism in the West");
        assert_eq!(t("from iPhone to DNA, e.g. in the USA"), "From iPhone to DNA, e.g. in the USA");
        assert_eq!(t("studies in honour of oskar von hinüber"), "Studies in Honour of Oskar von Hinüber");
        assert_eq!(t("al-bīrūnī's india"), "Al-Bīrūnī's India");
        assert_eq!(t("the life of al-bīrūnī"), "The Life of al-Bīrūnī");
        assert_eq!(t("the 16th world sanskrit conference, vol. 5"), "The 16th World Sanskrit Conference, Vol. 5");
        assert_eq!(t("“framing” the end of social history"), "“Framing” the End of Social History");
        assert_eq!(t("on indian historical writing — the role of the vaṃśāvalīs"), "On Indian Historical Writing — The Role of the Vaṃśāvalīs");
    }

    #[test]
    fn protected_parts() {
        let t = title_case;
        assert_eq!(t(r"\emph{Carakasaṃhitā} and its commentaries"), r"\emph{Carakasaṃhitā} and Its Commentaries");
        assert_eq!(t(r"\dev{नागेश}. a work on the sūtras"), r"\dev{नागेश}. A Work on the Sūtras");
        assert_eq!(t(r"{DNA} sequencing in {\=a}yurveda"), r"{DNA} Sequencing in {\=a}yurveda");
        assert_eq!(t(r"the \emph{aduruktavākyam} in its broader context"), r"The \emph{aduruktavākyam} in Its Broader Context");
        assert_eq!(t(r"wiley \& sons"), r"Wiley \& Sons");
        assert_eq!(t(r"the \textbengali{বাংলা} text"), r"The \textbengali{বাংলা} Text");
        assert_eq!(t("THE HISTORY OF INDIAN LITERATURE"), "The History of Indian Literature");
        assert_eq!(t(r"changing patterns of kinship in {South Asia}"), r"Changing Patterns of Kinship in {South Asia}");
        assert_eq!(t("volume 2 -- the slave kings"), "Volume 2 -- The Slave Kings");
        assert_eq!(t("Exploring the Inside of the Body through Time"), "Exploring the Inside of the Body through Time");
        assert_eq!(t("A Note on bhojya- and bhakṣya-"), "A Note on bhojya- and bhakṣya-");
        assert_eq!(t("On the Use of the Dhātu √vṛt in the Mahābhārata"), "On the Use of the Dhātu √vṛt in the Mahābhārata");
        assert_eq!(t("Bhartṛhari's Saṃbandha-samuddeśa and the Nyāya-vaiśeṣika Theory"), "Bhartṛhari's Saṃbandha-samuddeśa and the Nyāya-vaiśeṣika Theory");
        assert_eq!(t("india vis-a-vis ethiopia"), "India vis-a-vis Ethiopia");
        assert_eq!(t("Review of William Dalrymple, The Golden Road"), "Review of William Dalrymple, The Golden Road");
        assert_eq!(t("a study of “The Golden Road” and the silk road"), "A Study of “The Golden Road” and the Silk Road");
        assert_eq!(t("Review of: Jun Takashima (ed.) Studies"), "Review of: Jun Takashima (Ed.) Studies");
        assert_eq!(t("Suśrutasaṃhitā = The Suçruta-Saṁhitā or the Hindū System"), "Suśrutasaṃhitā = The Suçruta-Saṁhitā or the Hindū System");
        assert_eq!(t("Notes on Ou-k'ong's Account of Kaçmīr"), "Notes on Ou-k'ong's Account of Kaçmīr");
        assert!(!looks_english("paralipomena zum Sarvasarvātmakatvavāda II: Sarvasarvātmakatvavāda and its Relation to the Vṛkṣāyurveda"));
        assert!(!looks_english("O ensino da medicina na Índia colonial portuguesa: fundação [Medical teaching in Portuguese colonial India: the creation and the earliest decades of the school]"));
        assert!(looks_english("Comparative Guts: Exploring the Inside of the Body"));
        assert!(looks_english("Studies in honour of J. W. de Jong"));
        assert_eq!(t("‘Verità e bellezza’ Essays in Honour of Raffaele Torella"), "‘Verità e Bellezza’ Essays in Honour of Raffaele Torella");
        assert_eq!(t("Early Medieval Commentaries on the dharmaśāstra Tradition"), "Early Medieval Commentaries on the dharmaśāstra Tradition");
        assert_eq!(t("early medieval commentaries on the dharmaśāstra tradition"), "Early Medieval Commentaries on the Dharmaśāstra Tradition");
    }

    #[test]
    fn language_rules() {
        let lib = Library::parse(
            r#"@Book{a, title={the history}, langid={german}}
@Book{b, title={the history}, langid={english}}
@Book{c, title={geschichte der indischen literatur}}
@Book{d, title={the history of medicine}}
@Book{e, title={\dev{x} the history}, langid={english}}
@Book{f, title={ārogyaviṣayako nibandhaḥ}}
@Book{g, title={the history}, language={en}}
@Book{h, title={the history}, langid={english;tibetan}}
"#,
        );
        let v: Vec<bool> = lib.entries().map(auto_applies).collect();
        assert_eq!(v, vec![false, true, false, true, false, false, true, true]);
    }

    /// Show what library-wide capitalisation would change in the real file.
    #[test]
    fn real_library_sample() {
        let Ok(path) = std::env::var("SUCI_TEST_BIB") else { return };
        let lib = Library::parse(&std::fs::read_to_string(path).unwrap());
        let mut n = 0;
        let mut eligible = 0;
        for e in lib.entries() {
            if !auto_applies(e) {
                continue;
            }
            eligible += 1;
            let mut x = e.clone();
            if apply(&mut x) {
                n += 1;
                if n % 25 == 1 {
                    eprintln!("{}:\n  {}\n  {}", e.key, e.get("title").unwrap_or_default(), x.get("title").unwrap_or_default());
                }
            }
        }
        eprintln!("{eligible} English entries, {n} would change");
    }
}

