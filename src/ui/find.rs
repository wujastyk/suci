//! "Find DOI or ISBN": search by title, author and year, then use a result.

use super::App;
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use suci::bib::Entry;
use suci::fetch::{fetch, finalize, Ident};
use suci::latex::{to_markup, to_plain};
use suci::lookup::{search, Hit, Kind, Query};

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// For the selected entry: fill the form from it.
pub fn find_for_selected(app: &Rc<App>) {
    app.flush();
    let (idx, e) = {
        let st = app.state.borrow();
        match st.current.and_then(|i| st.doc.entry(i).cloned().map(|e| (i, e))) {
            Some(x) => x,
            None => {
                drop(st);
                show(app, None, Query::from_text(""));
                return;
            }
        }
    };
    // a volume of a multi-volume work: use the set's title and author if its own are missing
    let mut q = Query::from_entry(&e);
    if q.title.is_empty() || q.authors.is_empty() {
        if let Some(parent) = e.get_nonempty("crossref").and_then(|k| app.state.borrow().doc.lookup_entry(&k)) {
            let pq = Query::from_entry(&parent);
            if q.title.is_empty() {
                q.title = pq.title;
            }
            if q.authors.is_empty() {
                q.authors = pq.authors;
            }
        }
    }
    show(app, Some(idx), q);
}

/// Text typed in the Add box that is not a DOI or ISBN: search for it.
pub fn find_from_text(app: &Rc<App>, text: &str) {
    show(app, None, Query::from_text(text));
}

struct Form {
    title: gtk::Entry,
    author: gtk::Entry,
    year: gtk::Entry,
    container: gtk::Entry,
    kind: gtk::DropDown,
}

impl Form {
    fn query(&self, key: &str) -> Query {
        let year = self.year.text().trim().parse().ok();
        Query {
            title: self.title.text().trim().to_string(),
            authors: self.author.text().split([',', ';']).flat_map(|s| s.split(" and ")).map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            year,
            container: self.container.text().trim().to_string(),
            kind: Kind::ALL[self.kind.selected() as usize],
            openalex_key: key.to_string(),
        }
    }
}

fn show(app: &Rc<App>, target: Option<usize>, q: Query) {
    let key = target.and_then(|i| app.state.borrow().doc.entry(i).map(|e| e.key.clone())).unwrap_or_default();
    let title = if target.is_some() { format!("Find DOI or ISBN for {key}") } else { "Find a work by title".to_string() };
    let win = gtk::Window::builder().title(&title).transient_for(&app.w.window).modal(true).default_width(920).default_height(700).build();

    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(14);
    outer.set_margin_bottom(14);
    outer.set_margin_start(14);
    outer.set_margin_end(14);

    // ---- the search form ----
    let grid = gtk::Grid::builder().column_spacing(10).row_spacing(8).build();
    let label = |t: &str| {
        let l = gtk::Label::new(Some(t));
        l.set_xalign(1.0);
        l
    };
    let form = Rc::new(Form {
        title: gtk::Entry::builder().text(&q.title).hexpand(true).placeholder_text("Title (and subtitle)").build(),
        author: gtk::Entry::builder().text(q.authors.join(", ")).placeholder_text("Surname(s)").build(),
        year: gtk::Entry::builder().text(q.year.map(|y| y.to_string()).unwrap_or_default()).width_chars(6).max_width_chars(6).build(),
        container: gtk::Entry::builder().text(&q.container).placeholder_text("Journal or book it appeared in (optional)").build(),
        kind: gtk::DropDown::from_strings(&Kind::ALL.map(|k| k.label())),
    });
    form.kind.set_selected(Kind::ALL.iter().position(|k| *k == q.kind).unwrap_or(3) as u32);
    grid.attach(&label("Title"), 0, 0, 1, 1);
    grid.attach(&form.title, 1, 0, 5, 1);
    grid.attach(&label("Author"), 0, 1, 1, 1);
    grid.attach(&form.author, 1, 1, 1, 1);
    grid.attach(&label("Year"), 2, 1, 1, 1);
    grid.attach(&form.year, 3, 1, 1, 1);
    grid.attach(&label("Looking for"), 4, 1, 1, 1);
    grid.attach(&form.kind, 5, 1, 1, 1);
    grid.attach(&label("In"), 0, 2, 1, 1);
    grid.attach(&form.container, 1, 2, 3, 1);
    let search_btn = gtk::Button::with_label("Search");
    search_btn.add_css_class("suggested-action");
    search_btn.set_halign(gtk::Align::End);
    grid.attach(&search_btn, 4, 2, 2, 1);
    form.author.set_hexpand(true);
    outer.append(&grid);

    let note = gtk::Label::new(Some(
        "Searches Crossref and OpenAlex (DOIs, open-access copies) and, for books, K10plus/GVK, Google Books and Open Library (ISBNs, public-domain scans). Book reviews, and chapters when you want the whole book, are set aside. Open-access copies come first among equally good matches.",
    ));
    note.set_wrap(true);
    note.set_xalign(0.0);
    note.add_css_class("dim-label");
    note.add_css_class("caption");
    outer.append(&note);

    // ---- status ----
    let status_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let spinner = gtk::Spinner::new();
    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_hexpand(true);
    status.set_wrap(true);
    let show_aside = gtk::CheckButton::with_label("Show set-aside results");
    show_aside.set_visible(false);
    status_row.append(&spinner);
    status_row.append(&status);
    status_row.append(&show_aside);
    outer.append(&status_row);

    // ---- results ----
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    list.add_css_class("boxed-list");
    let sc = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    outer.append(&sc);
    let errors_lbl = gtk::Label::new(None);
    errors_lbl.set_xalign(0.0);
    errors_lbl.set_wrap(true);
    errors_lbl.add_css_class("dim-label");
    errors_lbl.add_css_class("caption");
    errors_lbl.set_visible(false);
    outer.append(&errors_lbl);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.set_halign(gtk::Align::End);
    let close = gtk::Button::with_label("Close");
    bar.append(&close);
    outer.append(&bar);
    win.set_child(Some(&outer));
    {
        let w = win.clone();
        close.connect_clicked(move |_| w.close());
    }
    let key_ctrl = gtk::EventControllerKey::new();
    {
        let w = win.clone();
        key_ctrl.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                w.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    win.add_controller(key_ctrl);

    let rows: Rc<RefCell<Vec<(gtk::ListBoxRow, bool)>>> = Rc::new(RefCell::new(Vec::new()));
    {
        let rows = rows.clone();
        show_aside.connect_toggled(move |cb| {
            for (r, aside) in rows.borrow().iter() {
                if *aside {
                    r.set_visible(cb.is_active());
                }
            }
        });
    }

    // ---- running a search ----
    let run: Rc<dyn Fn()> = {
        let app = app.clone();
        let form = form.clone();
        let list = list.clone();
        let spinner = spinner.clone();
        let status = status.clone();
        let errors_lbl = errors_lbl.clone();
        let show_aside = show_aside.clone();
        let search_btn = search_btn.clone();
        let rows = rows.clone();
        let win = win.clone();
        Rc::new(move || {
            let key = app.state.borrow().settings.openalex_key.clone();
            let q = form.query(&key);
            if q.title.trim().is_empty() {
                status.set_text("Type a title to search for.");
                form.title.grab_focus();
                return;
            }
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            rows.borrow_mut().clear();
            show_aside.set_visible(false);
            show_aside.set_active(false);
            errors_lbl.set_visible(false);
            spinner.start();
            search_btn.set_sensitive(false);
            status.set_text("Searching…");
            let app = app.clone();
            let list = list.clone();
            let spinner = spinner.clone();
            let status = status.clone();
            let errors_lbl = errors_lbl.clone();
            let show_aside = show_aside.clone();
            let search_btn = search_btn.clone();
            let rows = rows.clone();
            let win = win.clone();
            glib::spawn_future_local(async move {
                let q2 = q.clone();
                let (hits, errors) = gio::spawn_blocking(move || search(&q2)).await.unwrap_or_else(|_| (vec![], vec!["internal error".into()]));
                spinner.stop();
                search_btn.set_sensitive(true);
                let good = hits.iter().filter(|h| h.excluded.is_none()).count();
                let aside = hits.len() - good;
                let strong = hits.iter().filter(|h| h.excluded.is_none() && h.score >= suci::lookup::STRONG).count();
                status.set_markup(&match (good, aside) {
                    (0, 0) => "Nothing found. Try fewer words of the title, or check the spelling of the author's name.".to_string(),
                    (0, n) => format!(
                        "No usable DOI or ISBN found. <b>{n}</b> result{} set aside{}.",
                        if n == 1 { "" } else { "s" },
                        if hits.iter().all(|h| h.excluded.as_deref() == Some("no DOI or ISBN in this record")) { " (the work was found, but its records have no DOI or ISBN; older Indian books often have none)" } else { "" }
                    ),
                    (g, a) => format!(
                        "<b>{g}</b> result{}{}{}.",
                        if g == 1 { "" } else { "s" },
                        if strong > 0 { format!(", {strong} strong") } else { String::new() },
                        if a > 0 { format!("; {a} set aside (reviews, chapters, records without an identifier)") } else { String::new() }
                    ),
                });
                show_aside.set_visible(aside > 0);
                if good == 0 && aside > 0 {
                    show_aside.set_active(true);
                }
                let mut first = true;
                for h in hits {
                    let aside = h.excluded.is_some();
                    let best = first && !aside && h.score >= suci::lookup::STRONG;
                    first = false;
                    let row = hit_row(&app, &win, target, h, best);
                    row.set_visible(!aside || show_aside.is_active());
                    list.append(&row);
                    rows.borrow_mut().push((row, aside));
                }
                if !errors.is_empty() {
                    errors_lbl.set_text(&format!("Not reached: {}", errors.join("; ")));
                    errors_lbl.set_visible(true);
                }
            });
        })
    };
    {
        let run = run.clone();
        search_btn.connect_clicked(move |_| run());
    }
    for e in [&form.title, &form.author, &form.year, &form.container] {
        let run = run.clone();
        e.connect_activate(move |_| run());
    }
    win.present();
    if !q.title.trim().is_empty() {
        run();
    } else {
        form.title.grab_focus();
    }
}

fn hit_row(app: &Rc<App>, win: &gtk::Window, target: Option<usize>, h: Hit, best: bool) -> gtk::ListBoxRow {
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    let hb = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hb.set_margin_top(8);
    hb.set_margin_bottom(8);
    hb.set_margin_start(10);
    hb.set_margin_end(10);
    let vb = gtk::Box::new(gtk::Orientation::Vertical, 3);
    vb.set_hexpand(true);
    let e = &h.entry;

    let mut title = to_markup(&e.get("title").unwrap_or_default());
    if let Some(s) = e.get_nonempty("subtitle") {
        title = format!("{title}: {}", to_markup(&s));
    }
    let t = gtk::Label::new(None);
    t.set_markup(&format!("<b>{title}</b>"));
    t.set_wrap(true);
    t.set_wrap_mode(pango::WrapMode::WordChar);
    t.set_xalign(0.0);
    vb.append(&t);

    let people = e.get_nonempty("author").or_else(|| e.get_nonempty("editor").map(|ed| format!("{ed} (ed.)"))).map(|a| to_plain(&a).replace(" and ", "; "));
    let place = ["journaltitle", "booktitle"]
        .iter()
        .find_map(|f| e.get_nonempty(f))
        .map(|c| {
            let mut s = to_plain(&c);
            if let Some(v) = e.get_nonempty("volume") {
                s.push_str(&format!(" {v}"));
            }
            if let Some(p) = e.get_nonempty("pages") {
                s.push_str(&format!(", {}", p.replace("--", "–")));
            }
            s
        })
        .or_else(|| e.get_nonempty("publisher").map(|p| to_plain(&p)));
    let year = e.get_nonempty("date").map(|d| d.chars().take(4).collect::<String>());
    let mut sources = h.source.clone();
    if !h.also_in.is_empty() {
        sources.push_str(&format!(" + {}", h.also_in.join(", ")));
    }
    let meta: Vec<String> = [people, year, place, Some(h.kind.clone()), Some(sources)].into_iter().flatten().filter(|s| !s.is_empty()).collect();
    let m = gtk::Label::new(Some(&meta.join(" · ")));
    m.set_wrap(true);
    m.set_xalign(0.0);
    m.add_css_class("dim-label");
    vb.append(&m);

    let mut ids = Vec::new();
    if let Some(d) = h.doi() {
        ids.push(format!("DOI <tt>{}</tt>", esc(&d)));
    }
    if let Some(i) = h.isbn() {
        ids.push(format!("ISBN <tt>{}</tt>", esc(&i)));
    }
    if !ids.is_empty() {
        let l = gtk::Label::new(None);
        l.set_markup(&ids.join("     "));
        l.set_xalign(0.0);
        l.set_selectable(true);
        vb.append(&l);
    }

    let badges = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some(why) = &h.excluded {
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<span foreground='#c01c28'>Set aside: {}</span>", esc(why)));
        l.set_xalign(0.0);
        l.set_wrap(true);
        badges.append(&l);
    } else {
        let (colour, text) = if h.score >= suci::lookup::STRONG {
            ("#26a269", h.strength())
        } else if h.score >= suci::lookup::POSSIBLE {
            ("#c64600", h.strength())
        } else {
            ("#77767b", h.strength())
        };
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<span foreground='{colour}' weight='bold'>{text}</span>"));
        badges.append(&l);
    }
    if let Some(oa) = &h.open_access {
        let l = gtk::Label::new(None);
        let what = if oa.pdf.is_some() { "Open-access PDF" } else { "Free to read" };
        l.set_markup(&format!("<span foreground='#26a269' weight='bold'>🔓 {what}</span> <span size='small'>{}</span>", esc(&oa.note)));
        l.set_xalign(0.0);
        l.set_wrap(true);
        badges.append(&l);
    }
    vb.append(&badges);
    hb.append(&vb);

    // buttons
    let bb = gtk::Box::new(gtk::Orientation::Vertical, 6);
    bb.set_valign(gtk::Align::Center);
    let use_btn = gtk::Button::with_label(if target.is_some() { "Use…" } else { "Add…" });
    if best {
        use_btn.add_css_class("suggested-action");
    }
    bb.append(&use_btn);
    let view_url = h
        .doi()
        .map(|d| format!("https://doi.org/{d}"))
        .or_else(|| e.get_nonempty("ppn_gvk").map(|p| format!("https://kxp.k10plus.de/DB=2.1/PPNSET?PPN={p}")))
        .or_else(|| h.isbn().map(|i| format!("https://openlibrary.org/isbn/{i}")));
    if let Some(u) = view_url {
        let b = gtk::Button::with_label("View");
        b.set_tooltip_text(Some(&format!("Open {u} in your web browser")));
        let me = app.clone();
        b.connect_clicked(move |_| me.launch_uri(&u));
        bb.append(&b);
    }
    if let Some(oa) = &h.open_access {
        let b = gtk::Button::with_label("Read");
        let u = oa.pdf.clone().unwrap_or_else(|| oa.url.clone());
        b.set_tooltip_text(Some(&format!("Open the free copy: {u}")));
        let me = app.clone();
        b.connect_clicked(move |_| me.launch_uri(&u));
        bb.append(&b);
    }
    hb.append(&bb);
    row.set_child(Some(&hb));

    let me = app.clone();
    let parent = win.clone();
    use_btn.connect_clicked(move |_| show_use(&me, &parent, target, h.clone()));
    row
}

// ---------- using a result ----------

fn show_use(app: &Rc<App>, parent: &gtk::Window, target: Option<usize>, h: Hit) {
    let current: Option<Entry> = target.and_then(|i| app.state.borrow().doc.entry(i).cloned());
    let win = gtk::Window::builder()
        .title(if target.is_some() { "Use this record" } else { "Add this work" })
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .build();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(14);
    outer.set_margin_start(16);
    outer.set_margin_end(16);
    let head = gtk::Label::new(None);
    head.set_markup(&format!("<b>{}</b>", to_markup(&h.entry.get("title").unwrap_or_default())));
    head.set_wrap(true);
    head.set_xalign(0.0);
    outer.append(&head);

    let cur = |f: &str| current.as_ref().and_then(|e| e.get_nonempty(f)).map(|v| v.trim().to_string());
    let mut checks: Vec<(gtk::CheckButton, &'static str, String)> = Vec::new();
    let check = |label: &str, on: bool| {
        let c = gtk::CheckButton::with_label(label);
        c.set_active(on);
        if let Some(l) = c.last_child().and_then(|w| w.downcast::<gtk::Label>().ok()) {
            l.set_wrap(true);
            l.set_wrap_mode(pango::WrapMode::WordChar);
        }
        c
    };
    if target.is_some() {
        for (field, val) in [("doi", h.doi()), ("isbn", h.isbn())] {
            let Some(v) = val else { continue };
            let name = field.to_uppercase();
            match cur(field) {
                None => checks.push((check(&format!("Set {name} to {v}"), true), field, v)),
                Some(old) if old.eq_ignore_ascii_case(&v) || suci::checks::clean_doi(&old).eq_ignore_ascii_case(&v) => {}
                Some(old) => checks.push((check(&format!("Replace {name} {old} with {v}"), false), field, v)),
            }
        }
    }
    // an Internet Archive scan's ARK goes in eprint (as for other online copies)
    let ark = h.entry.get_nonempty("eprint").filter(|_| h.entry.get("eprinttype").as_deref() == Some("ark"));
    if let (Some(a), true) = (&ark, target.is_some()) {
        match cur("eprint") {
            None => checks.push((check(&format!("Set Eprint to the Internet Archive ARK {a}"), true), "eprint", a.clone())),
            Some(old) if old == *a => {}
            Some(old) => checks.push((check(&format!("Replace Eprint {old} with {a}"), false), "eprint", a.clone())),
        }
    }
    let oa = h.open_access.clone();
    // a doi.org link adds nothing to the DOI itself
    let just_doi = |u: &str| {
        let l = u.to_lowercase();
        l.starts_with("https://doi.org/") || l.starts_with("http://doi.org/") || l.starts_with("https://dx.doi.org/")
    };
    if let Some(o) = oa.as_ref().filter(|o| !(just_doi(&o.url) && o.pdf.is_none())) {
        let url = if just_doi(&o.url) { o.pdf.clone().unwrap_or_default() } else { o.url.clone() };
        match cur("url") {
            // with the ARK stored, the archive.org link is not needed as well
            None => checks.push((check(&format!("Set URL to the free copy: {url}"), ark.is_none()), "url", url)),
            Some(old) if old == url => {}
            Some(old) => checks.push((check(&format!("Replace URL {old} with the free copy: {url}"), false), "url", url)),
        }
    }
    let pdf_check = oa.as_ref().and_then(|o| o.pdf.clone()).map(|pdf| {
        let has_file = cur("file").is_some();
        (check(&format!("Download the open-access PDF and link it ({})", short_host(&pdf)), !has_file), pdf)
    });
    // the rest of the record: for a target, compare field by field afterwards
    let full_record = matches!(h.source.as_str(), "Crossref" | "K10plus/GVK");
    let merge_check = target.map(|_| check("Then compare the other fields with the full record and fill in what is missing", true));

    for (c, _, _) in &checks {
        outer.append(c);
    }
    if let Some((c, _)) = &pdf_check {
        outer.append(c);
    }
    if let Some(c) = &merge_check {
        outer.append(c);
    }
    if checks.is_empty() && pdf_check.is_none() && merge_check.is_none() {
        let what = match (h.doi(), h.isbn()) {
            (Some(d), _) => format!("The entry will be added with DOI {d}."),
            (None, Some(i)) => format!("The entry will be added with ISBN {i}."),
            _ => "The entry will be added.".to_string(),
        };
        let l = gtk::Label::new(Some(&what));
        l.set_xalign(0.0);
        outer.append(&l);
    }

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.set_halign(gtk::Align::End);
    bar.set_margin_top(6);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label(if target.is_some() { "Apply" } else { "Add Entry" });
    ok.add_css_class("suggested-action");
    bar.append(&cancel);
    bar.append(&ok);
    outer.append(&bar);
    win.set_child(Some(&outer));
    {
        let w = win.clone();
        cancel.connect_clicked(move |_| w.close());
    }
    let me = app.clone();
    let w = win.clone();
    let finder = parent.clone();
    ok.connect_clicked(move |_| {
        let chosen: Vec<(&'static str, String)> = checks.iter().filter(|(c, _, _)| c.is_active()).map(|(_, f, v)| (*f, v.clone())).collect();
        let pdf = pdf_check.as_ref().filter(|(c, _)| c.is_active()).map(|(_, u)| u.clone());
        let merge = merge_check.as_ref().is_some_and(|c| c.is_active());
        w.close();
        finder.close();
        match target {
            Some(idx) => apply_to(&me, idx, &h, chosen, pdf, merge, full_record),
            None => add_new(&me, &h, pdf),
        }
    });
    win.present();
}

fn short_host(url: &str) -> String {
    url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url).to_string()
}

fn apply_to(app: &Rc<App>, idx: usize, h: &Hit, chosen: Vec<(&'static str, String)>, pdf: Option<String>, merge: bool, full_record: bool) {
    if !chosen.is_empty() {
        let Some(mut e) = app.state.borrow().doc.entry(idx).cloned() else { return };
        for (f, v) in &chosen {
            e.set(f, v);
            if *f == "eprint" {
                e.set("eprinttype", "ark");
            }
        }
        app.state.borrow_mut().doc.update_entry(idx, e);
        app.after_doc_change(Some(idx));
        let what: Vec<String> = chosen.iter().map(|(f, _)| f.to_uppercase()).collect();
        app.toast(&format!("Set {}", what.join(", ")), None);
    }
    if let Some(url) = pdf {
        download_pdf(app, Some(idx), url);
    }
    if merge {
        if full_record {
            super::dialogs::merge_record(app, idx, &h.source, h.entry.clone());
        } else if let Some(id) = h.doi().or_else(|| h.isbn()) {
            super::dialogs::start_fetch(app, &id, Some(idx));
        } else {
            super::dialogs::merge_record(app, idx, &h.source, h.entry.clone());
        }
    }
}

fn add_new(app: &Rc<App>, h: &Hit, pdf: Option<String>) {
    // already in the library?
    for id in [h.doi().map(Ident::Doi), h.isbn().map(Ident::Isbn)].into_iter().flatten() {
        let found = app.state.borrow().doc.find_identifier(&id);
        if let Some(&i) = found.first() {
            let key = app.state.borrow().doc.entry(i).map(|e| e.key.clone()).unwrap_or_default();
            app.select_entry(i);
            app.toast(&format!("Already in the library as {key} ({})", id.label()), None);
            return;
        }
    }
    let use_latex = app.state.borrow().settings.fetch_as_latex;
    let oa_url = h.open_access.as_ref().map(|o| if o.url.is_empty() { o.pdf.clone().unwrap_or_default() } else { o.url.clone() });
    let finish = {
        let app = app.clone();
        move |mut e: Entry| {
            if let Some(u) = &oa_url {
                if e.get_nonempty("url").is_none() && e.get_nonempty("eprint").is_none() && !u.is_empty() && !u.to_lowercase().contains("doi.org/") {
                    e.set("url", u);
                }
            }
            let idx = app.add_fetched(finalize(e, use_latex));
            if let Some(url) = pdf {
                download_pdf(&app, Some(idx), url);
            }
        }
    };
    let full_record = matches!(h.source.as_str(), "Crossref" | "K10plus/GVK");
    let ident = h.doi().map(Ident::Doi).or_else(|| h.isbn().map(Ident::Isbn));
    match ident {
        // records from OpenAlex, Google Books and Open Library are brief: fetch the full one
        Some(id) if !full_record => {
            app.toast(&format!("Fetching the full record for {}…", id.label()), None);
            let fallback = h.entry.clone();
            glib::spawn_future_local(async move {
                let (cands, _) = gio::spawn_blocking(move || fetch(&id)).await.unwrap_or_default();
                finish(cands.into_iter().next().map(|c| c.entry).unwrap_or(fallback));
            });
        }
        _ => finish(h.entry.clone()),
    }
}

/// Where downloaded PDFs go: the first existing PDF folder, else beside the .bib.
fn pdf_folder(app: &App) -> Option<PathBuf> {
    let st = app.state.borrow();
    st.doc.file_dirs.dirs.iter().find(|d| d.is_dir()).cloned().or_else(|| st.doc.path.as_ref().and_then(|p| p.parent()).map(|p| p.to_path_buf()))
}

fn download_pdf(app: &Rc<App>, idx: Option<usize>, url: String) {
    let Some(idx) = idx else { return };
    let Some(dir) = pdf_folder(app) else {
        app.toast("Save the library first, or set a PDF folder in Preferences", None);
        return;
    };
    let key = app.state.borrow().doc.entry(idx).map(|e| e.key.clone()).unwrap_or_default();
    let base = if key.is_empty() { "download".to_string() } else { key.replace(['/', '\\', ':'], "_") };
    let mut path = dir.join(format!("{base}.pdf"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.pdf"));
        n += 1;
    }
    app.toast(&format!("Downloading PDF from {}…", short_host(&url)), None);
    let me = app.clone();
    let u2 = url.clone();
    glib::spawn_future_local(async move {
        let p2 = path.clone();
        let res = gio::spawn_blocking(move || -> Result<(), String> {
            let bytes = suci::fetch::download_bytes(&u2, 200 * 1024 * 1024)?;
            if !bytes.starts_with(b"%PDF") {
                return Err("the link gave a web page, not a PDF".into());
            }
            std::fs::write(&p2, &bytes).map_err(|e| format!("could not save {}: {e}", p2.display()))
        })
        .await
        .unwrap_or_else(|_| Err("internal error".into()));
        match res {
            Ok(()) => {
                me.select_entry(idx);
                super::editor::attach_files(&me, &[path.clone()]);
                me.toast(&format!("Saved and linked {}", path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()), None);
            }
            Err(e) => {
                let me2 = me.clone();
                let u3 = url.clone();
                me.toast(&format!("Could not download the PDF: {e}"), Some(Box::new(move || me2.launch_uri(&u3))));
            }
        }
    });
}
