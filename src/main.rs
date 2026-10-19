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
