//! Diacritic folding and sort keys (Latin order and Sanskrit varṇamālā order).

use unicode_normalization::UnicodeNormalization;

fn is_mark(c: char) -> bool {
    matches!(c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF)
}

/// Lowercase, strip diacritics and transliteration apostrophes:
/// "Śābdabodha" → "sabdabodha", "ʿAqāʾid" → "aqaid".
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfd() {
        if is_mark(c) {
            continue;
        }
        match c {
            'ʿ' | 'ʾ' | 'ʼ' | 'ʽ' | '\u{2018}' | '\u{2019}' | '\'' => {}
            'ł' | 'Ł' => out.push('l'),
            'ø' | 'Ø' => out.push('o'),
            'æ' | 'Æ' => out.push_str("ae"),
            'œ' | 'Œ' => out.push_str("oe"),
            'ß' => out.push_str("ss"),
            'ı' => out.push('i'),
            'ð' | 'Ð' => out.push('d'),
            'þ' | 'Þ' => out.push_str("th"),
            '\u{00A0}' | '\u{2009}' => out.push(' '),
            '–' | '—' => out.push('-'),
            c => {
                for l in c.to_lowercase() {
                    out.push(l);
                }
            }
        }
    }
    out
}

/// Sort key: alphabetical ignoring diacritics, with the diacritic form as a
/// tie-breaker, so a < ā < b and s < ś < ṣ < t.
pub fn latin_key(s: &str) -> String {
    let f = fold(s);
    let mut k = String::with_capacity(f.len() * 2 + 2);
    for c in f.chars() {
        if c.is_alphanumeric() || c == ' ' {
            k.push(c);
        }
    }
    k.push('\u{1}');
    k.extend(s.nfd().flat_map(|c| c.to_lowercase()));
    k
}

/// Sanskrit (IAST) varṇamālā order:
/// a ā i ī u ū ṛ ṝ ḷ ḹ e ai o au ṃ ḥ k kh g gh ṅ c ch j jh ñ ṭ ṭh ḍ ḍh ṇ
/// t th d dh n p ph b bh m y r l v ś ṣ s h.
/// Letters outside IAST sort after h by their folded value.
pub fn sanskrit_key(s: &str) -> Vec<u32> {
    const ORDER: [&str; 49] = [
        "a", "ā", "i", "ī", "u", "ū", "ṛ", "ṝ", "ḷ", "ḹ", "e", "ai", "o", "au", "ṃ", "ḥ", "k", "kh", "g", "gh",
        "ṅ", "c", "ch", "j", "jh", "ñ", "ṭ", "ṭh", "ḍ", "ḍh", "ṇ", "t", "th", "d", "dh", "n", "p", "ph", "b",
        "bh", "m", "y", "r", "l", "v", "ś", "ṣ", "s", "h",
    ];
    let lower: String = s.nfc().flat_map(|c| c.to_lowercase()).collect::<String>();
    // treat ṁ (anusvāra variant) as ṃ
    let lower = lower.replace('ṁ', "ṃ");
    let chars: Vec<char> = lower.chars().collect();
    let mut key = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() || c == '-' {
            key.push(1);
            i += 1;
            continue;
        }
        if c.is_ascii_digit() {
            key.push(10 + c as u32 - '0' as u32);
            i += 1;
            continue;
        }
        if !c.is_alphabetic() {
            i += 1;
            continue;
        }
        // try two-char phoneme first
        if i + 1 < chars.len() {
            let two: String = [c, chars[i + 1]].iter().collect();
            if let Some(p) = ORDER.iter().position(|x| *x == two) {
                key.push(100 + p as u32);
                i += 2;
                continue;
            }
        }
        let one = c.to_string();
        if let Some(p) = ORDER.iter().position(|x| *x == one) {
            key.push(100 + p as u32);
        } else {
            // non-IAST letter: place after h, ordered by folded form
            let f = fold(&one);
            let base = f.chars().next().unwrap_or(c);
            key.push(1000 + base as u32);
        }
        i += 1;
    }
    key
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum SortMode {
    #[default]
    Latin,
    Sanskrit,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding() {
        assert_eq!(fold("Śābdabodha"), "sabdabodha");
        assert_eq!(fold("ʿAqāʾid"), "aqaid");
        assert_eq!(fold("Ṛgveda"), "rgveda");
        assert_eq!(fold("Hinüber"), "hinuber");
    }

    #[test]
    fn latin_order() {
        let mut v = vec!["Śiva", "Sutton", "Tagore", "saṃhitā", "Sarma", "ṣaṭ"];
        v.sort_by_key(|s| latin_key(s));
        assert_eq!(v, vec!["saṃhitā", "Sarma", "ṣaṭ", "Śiva", "Sutton", "Tagore"]);
    }

    #[test]
    fn sanskrit_order() {
        let mut v = vec!["kha", "ka", "ṛṣi", "agni", "ākāśa", "aiśvarya", "ojas", "hari", "śiva", "saṃhitā", "kṛṣṇa", "candra"];
        v.sort_by_key(|s| sanskrit_key(s));
        assert_eq!(
            v,
            vec!["agni", "ākāśa", "ṛṣi", "aiśvarya", "ojas", "ka", "kṛṣṇa", "kha", "candra", "śiva", "saṃhitā", "hari"]
        );
    }
}
