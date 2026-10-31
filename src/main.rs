// No console window behind the app on Windows (release builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod ui;

use gtk::prelude::*;
use gtk::{gio, glib};

pub const APP_ID: &str = "io.github.suci.Suci";
pub const APP_NAME: &str = "Sūcī";

fn main() -> glib::ExitCode {
    // `suci --check file.bib` prints an integrity report without opening a window
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 3 && args[1] == "--check" {
        return cli_check(&args[2]);
    }
 if args.len() >= 3 && args[1] == "--find" {
        return cli_find(&args[2..]);
    }
    if args.len() >= 2 && (args[1] == "--version" || args[1] == "-V") {
        println!("suci {}", env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }
    let app = gtk::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN | gio::ApplicationFlags::NON_UNIQUE)
        .build();
    app.connect_startup(|_| {
        ui::load_css();
        gtk::Window::set_default_icon_name(APP_ID);
    });
    app.connect_activate(|app| {
        ui::App::build(app, None);
    });
    app.connect_open(|app, files, _| {
        let path = files.first().and_then(|f| f.path());
        ui::App::build(app, path);
    });
    app.run()
}

/// `suci --find "title" [author] [year] [article|chapter|book]`:
/// look up DOIs/ISBNs in the terminal.
fn cli_find(args: &[String]) -> glib::ExitCode {
    use suci::lookup::{search, Kind, Query};
    let mut q = Query::from_text(&args[0]);
    for a in &args[1..] {
        match a.as_str() {
            "article" => q.kind = Kind::Article,
            "chapter" => q.kind = Kind::Chapter,
            "book" => q.kind = Kind::Book,
            y if y.len() == 4 && y.chars().all(|c| c.is_ascii_digit()) => q.year = y.parse().ok(),
            name => q.authors.push(name.to_string()),
        }
    }
    q.openalex_key = suci::settings::Settings::load().openalex_key;
    let (hits, errors) = search(&q);
    for h in &hits {
        let e = &h.entry;
        println!(
            "{:.2} {:14} {} | {} | {} | {} | {}{}",
            h.score,
            h.strength(),
            h.kind,
            e.get("author").unwrap_or_default(),
            e.get("date").unwrap_or_default(),
            e.get("title").unwrap_or_default(),
            h.doi().map(|d| format!("DOI {d}")).or(h.isbn().map(|i| format!("ISBN {i}"))).unwrap_or_default(),
            h.excluded.as_ref().map(|x| format!("  [SET ASIDE: {x}]")).unwrap_or_default()
        );
        println!("     from {}{}", h.source, if h.also_in.is_empty() { String::new() } else { format!(" + {}", h.also_in.join(", ")) });
        if let Some(ark) = h.entry.get_nonempty("eprint") {
            println!("     ARK {ark} · {} {}", h.entry.get("location").unwrap_or_default(), h.entry.get("publisher").unwrap_or_default());
        }
        if let Some(oa) = &h.open_access {
            println!("     OPEN ACCESS: {} {} {}", oa.note, oa.url, oa.pdf.as_deref().unwrap_or(""));
        }
    }
    for e in errors {
        eprintln!("! {e}");
    }
    glib::ExitCode::SUCCESS
}

fn cli_check(path: &str) -> glib::ExitCode {
    let settings = suci::settings::Settings::load();
    let doc = match suci::doc::Document::open(std::path::Path::new(path), Some(&settings.file_directory)) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return glib::ExitCode::FAILURE;
        }
    };
    let issues = suci::checks::check(&doc.lib, Some(&doc.file_dirs));
    for i in &issues {
        println!("{:<10} {:<24} {:<14} {}", i.severity.label(), i.key, i.field.clone().unwrap_or_default(), i.message);
    }
    eprintln!("{} entries, {} issues", doc.lib.entries().count(), issues.len());
    glib::ExitCode::SUCCESS
}
