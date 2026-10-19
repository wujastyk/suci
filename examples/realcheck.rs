use std::collections::BTreeMap;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let src = std::fs::read_to_string(&path).unwrap();
    let t = std::time::Instant::now();
    let lib = suci::bib::Library::parse(&src);
    let iss = suci::checks::check(&lib, None);
    eprintln!("checked in {:?}: {} issues", t.elapsed(), iss.len());
    let mut by: BTreeMap<String, (usize, Vec<String>)> = BTreeMap::new();
    for i in &iss {
        let m: String = i.message.split(|c: char| c == '\'' || c.is_ascii_digit()).next().unwrap().to_string();
        let k = format!("{:?} {}", i.severity, m);
        let e = by.entry(k).or_default();
        e.0 += 1;
        if e.1.len() < 3 { e.1.push(format!("{} [{}] {}", i.key, i.field.clone().unwrap_or_default(), i.message.chars().take(90).collect::<String>())); }
    }
    for (k, (n, ex)) in by { println!("{n:5}  {k}"); for e in ex { println!("         {e}"); } }
}
