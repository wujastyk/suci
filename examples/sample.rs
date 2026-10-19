fn main() {
    let path = std::env::args().nth(1).unwrap();
    let style = std::env::args().nth(2).unwrap_or("chicago-author-date-17th-edition".into());
    let keys: Vec<String> = std::env::args().skip(3).collect();
    let src = std::fs::read_to_string(&path).unwrap();
    let lib = suci::bib::Library::parse(&src);
    let all: Vec<_> = lib.entries().cloned().collect();
    let map: std::collections::HashMap<String, suci::bib::Entry> = all.iter().map(|e| (e.key.clone(), e.clone())).collect();
    let lookup = |k: &str| map.get(k).cloned();
    let styles = suci::cite::Styles::load();
    let pick: Vec<&suci::bib::Entry> = if keys.is_empty() { all.iter().step_by(all.len() / 25).collect() } else { keys.iter().filter_map(|k| map.get(k)).collect() };
    for e in pick {
        let r = styles.render(&style, &[e], &lookup).unwrap();
        println!("[{}:{}] {}\n     {}", e.ty, e.key, r.bib_text.join(" | "), r.citations.join(" | "));
    }
}
