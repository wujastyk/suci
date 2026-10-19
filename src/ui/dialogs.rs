//! Dialogs: fetching, merge, integrity check, preferences, shortcuts, about.

use super::App;
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use std::cell::RefCell;
use std::rc::Rc;
use suci::bib::Entry;
use suci::checks::{check, Issue, Severity};
use suci::fetch::{detect, fetch, finalize, Candidate, Ident};

fn dialog_window(app: &App, title: &str, w: i32, h: i32) -> gtk::Window {
    gtk::Window::builder()
        .title(title)
        .transient_for(&app.w.window)
        .modal(true)
        .default_width(w)
        .default_height(h)
        .build()
}

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

// ---------- fetching ----------

/// Look up a DOI/ISBN. With `merge_into`, fill missing fields of that entry;
/// otherwise offer to add a new entry.
pub fn start_fetch(app: &Rc<App>, text: &str, merge_into: Option<usize>) {
    let Some(id) = detect(text) else {
        app.toast("That does not look like a DOI or an ISBN", None);
        return;
    };
    if merge_into.is_none() {
        let existing = app.state.borrow().doc.find_identifier(&id);
        if let Some(&first) = existing.first() {
            let key = app.state.borrow().doc.entry(first).map(|e| e.key.clone()).unwrap_or_default();
            let dlg = gtk::AlertDialog::builder()
                .modal(true)
                .message("Already in the library")
                .detail(format!("{} is already in your library as “{key}”.", id.label()))
                .buttons(["Show it", "Fetch anyway"])
                .cancel_button(0)
                .default_button(0)
                .build();
            let me = app.clone();
            let id2 = id.clone();
            dlg.choose(Some(&app.w.window), None::<&gio::Cancellable>, move |res| {
                if res == Ok(1) {
                    run_fetch(&me, id2.clone(), None);
                } else {
                    me.w.search.set_text("");
                    me.refilter(Some(first));
                    me.w.fetch_entry.set_text("");
                }
            });
            return;
        }
    }
    run_fetch(app, id, merge_into);
}

fn run_fetch(app: &Rc<App>, id: Ident, merge_into: Option<usize>) {
    app.w.fetch_spinner.start();
    app.w.fetch_entry.set_sensitive(false);
    app.toast(&format!("Looking up {}…", id.label()), None);
    let me = app.clone();
    let id2 = id.clone();
    glib::spawn_future_local(async move {
        let res = gio::spawn_blocking(move || fetch(&id2)).await;
        me.w.fetch_spinner.stop();
        me.w.fetch_entry.set_sensitive(true);
        me.w.toast_revealer.set_reveal_child(false);
        let (cands, errors) = res.unwrap_or_else(|_| (vec![], vec!["internal error".into()]));
        if cands.is_empty() {
            me.error(&format!("Nothing found for {}", id.label()), &errors.join("\n"));
            return;
        }
        me.w.fetch_entry.set_text("");
        match merge_into {
            Some(idx) => show_merge(&me, idx, cands),
            None => show_candidates(&me, &id, cands, errors),
        }
    });
}

fn preview_markup(app: &App, e: &Entry) -> String {
    let st = app.state.borrow();
    let style = st.settings.citation_style.clone();
    let lookup = |k: &str| st.doc.lookup_entry(k);
    match st.styles.render(&style, &[e], &lookup) {
        Ok(r) => r.bib_markup.join(" "),
        Err(err) => esc(&err),
    }
}

fn show_candidates(app: &Rc<App>, id: &Ident, cands: Vec<Candidate>, errors: Vec<String>) {
    let use_latex = app.state.borrow().settings.fetch_as_latex;
    let cands: Vec<Candidate> = cands.into_iter().map(|c| Candidate { source: c.source, entry: finalize(c.entry, use_latex) }).collect();
    // a single DOI result: add directly
    if cands.len() == 1 && matches!(id, Ident::Doi(_)) {
        app.add_fetched(cands[0].entry.clone());
        return;
    }
    let win = dialog_window(app, &format!("Results for {}", id.label()), 760, 560);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(14);
    outer.set_margin_bottom(14);
    outer.set_margin_start(14);
    outer.set_margin_end(14);
    let intro = gtk::Label::new(Some("Catalogues often differ, especially for Indian publications. Choose the best record; you can edit it afterwards."));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    outer.append(&intro);

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("boxed-list");
    for c in &cands {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        row.set_margin_top(8);
        row.set_margin_bottom(8);
        row.set_margin_start(8);
        row.set_margin_end(8);
        let src = gtk::Label::new(None);
        src.set_markup(&format!("<b>{}</b>  <span alpha=\"60%\">{} fields</span>", esc(&c.source), c.entry.fields.len()));
        src.set_xalign(0.0);
        row.append(&src);
        let p = gtk::Label::new(None);
        p.set_markup(&preview_markup(app, &c.entry));
        p.set_wrap(true);
        p.set_xalign(0.0);
        row.append(&p);
        let exp = gtk::Expander::new(Some("BibTeX"));
        let code = gtk::Label::new(Some(&c.entry.to_bibtex()));
        code.add_css_class("monospace");
        code.set_xalign(0.0);
        code.set_selectable(true);
        code.set_wrap(true);
        code.set_wrap_mode(pango::WrapMode::WordChar);
        exp.set_child(Some(&code));
        row.append(&exp);
        list.append(&row);
    }
    if let Some(r) = list.row_at_index(0) {
        list.select_row(Some(&r));
    }
    let sc = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    outer.append(&sc);
    if !errors.is_empty() {
        let l = gtk::Label::new(Some(&format!("Not available: {}", errors.join("; "))));
        l.add_css_class("dim-label");
        l.set_wrap(true);
        l.set_xalign(0.0);
        outer.append(&l);
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let add = gtk::Button::with_label("Add to library");
    add.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&add);
    outer.append(&buttons);
    win.set_child(Some(&outer));
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    let me = app.clone();
    let w2 = win.clone();
    let lst = list.clone();
    add.connect_clicked(move |_| {
        let i = lst.selected_row().map(|r| r.index() as usize).unwrap_or(0);
        if let Some(c) = cands.get(i) {
            me.add_fetched(c.entry.clone());
        }
        w2.close();
    });
    list.connect_row_activated(move |_, _| {
        add.emit_clicked();
    });
    win.present();
}

/// Show fetched values next to the entry's own, and let the user choose which to take.
fn show_merge(app: &Rc<App>, idx: usize, cands: Vec<Candidate>) {
    let Some(current) = app.state.borrow().doc.entry(idx).cloned() else { return };
    let use_latex = app.state.borrow().settings.fetch_as_latex;
    let cand = finalize(cands[0].entry.clone(), use_latex);
    let win = dialog_window(app, &format!("Update {} from {}", current.key, cands[0].source), 820, 560);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(14);
    outer.set_margin_bottom(14);
    outer.set_margin_start(14);
    outer.set_margin_end(14);
    let intro = gtk::Label::new(Some("Tick the fetched values you want. Empty fields are ticked by default; your existing values are never replaced unless you tick them."));
    intro.set_wrap(true);
    intro.set_xalign(0.0);
    outer.append(&intro);
    let grid = gtk::Grid::builder().column_spacing(12).row_spacing(6).build();
    for (c, t) in ["", "Field", "Your entry", "Fetched"].iter().enumerate() {
        let l = gtk::Label::new(None);
        l.set_markup(&format!("<b>{t}</b>"));
        l.set_xalign(0.0);
        grid.attach(&l, c as i32, 0, 1, 1);
    }
    let checks: Rc<RefCell<Vec<(gtk::CheckButton, String, String)>>> = Rc::new(RefCell::new(Vec::new()));
    let mut row = 1;
    for f in &cand.fields {
        let newv = f.editor_text();
        let oldv = current.get(&f.name).unwrap_or_default();
        if newv.trim() == oldv.trim() || newv.trim().is_empty() {
            continue;
        }
        let cb = gtk::CheckButton::new();
        cb.set_active(oldv.trim().is_empty());
        cb.set_valign(gtk::Align::Start);
        grid.attach(&cb, 0, row, 1, 1);
        let name = gtk::Label::new(Some(&f.name));
        name.set_xalign(0.0);
        name.set_valign(gtk::Align::Start);
        grid.attach(&name, 1, row, 1, 1);
        for (col, v) in [(2, &oldv), (3, &newv)] {
            let l = gtk::Label::new(Some(&v.chars().take(400).collect::<String>()));
            l.set_wrap(true);
            l.set_wrap_mode(pango::WrapMode::WordChar);
            l.set_xalign(0.0);
            l.set_hexpand(true);
            l.set_selectable(true);
            l.set_max_width_chars(40);
            if col == 2 && v.is_empty() {
                l.set_text("—");
                l.add_css_class("dim-label");
            }
            grid.attach(&l, col, row, 1, 1);
        }
        checks.borrow_mut().push((cb, f.name.clone(), newv));
        row += 1;
    }
    if row == 1 {
        app.toast("The fetched record adds nothing new", None);
        return;
    }
    let ty_check = if !cand.ty.eq_ignore_ascii_case(&current.ty) {
        let cb = gtk::CheckButton::with_label(&format!("Change entry type from {} to {}", current.ty, cand.ty));
        Some(cb)
    } else {
        None
    };
    let sc = gtk::ScrolledWindow::builder().child(&grid).vexpand(true).build();
    outer.append(&sc);
    if let Some(cb) = &ty_check {
        outer.append(cb);
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&apply);
    outer.append(&buttons);
    win.set_child(Some(&outer));
    let w2 = win.clone();
    cancel.connect_clicked(move |_| w2.close());
    let me = app.clone();
    let w2 = win.clone();
    let new_ty = cand.ty.clone();
    apply.connect_clicked(move |_| {
        let mut e = current.clone();
        for (cb, name, v) in checks.borrow().iter() {
            if cb.is_active() {
                e.set(name, v);
            }
        }
        if ty_check.as_ref().is_some_and(|c| c.is_active()) {
            e.set_type(&new_ty);
        }
        me.state.borrow_mut().doc.update_entry(idx, e);
        me.after_doc_change(Some(idx));
        w2.close();
    });
    win.present();
}

// ---------- integrity check ----------

pub fn show_checks(app: &Rc<App>) {
    app.flush();
    let win = dialog_window(app, "Check library", 900, 640);
    win.set_modal(false);
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 8);
    outer.set_margin_top(12);
    outer.set_margin_bottom(12);
    outer.set_margin_start(12);
    outer.set_margin_end(12);
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let filter = gtk::DropDown::from_strings(&["Errors and warnings", "Everything (incl. suggestions)", "Errors only", "Fixable automatically"]);
    let summary = gtk::Label::new(None);
    summary.set_hexpand(true);
    summary.set_xalign(0.0);
    let fix_all = gtk::Button::with_label("Fix all shown");
    fix_all.set_tooltip_text(Some("Apply every automatic fix in the list below (one undo step)"));
    top.append(&filter);
    top.append(&summary);
    top.append(&fix_all);
    outer.append(&top);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::None);
    let sc = gtk::ScrolledWindow::builder().child(&list).vexpand(true).build();
    outer.append(&sc);
    let hint = gtk::Label::new(Some("Click an entry key to show it in the main window."));
    hint.add_css_class("dim-label");
    hint.set_xalign(0.0);
    outer.append(&hint);
    win.set_child(Some(&outer));

    let issues: Rc<RefCell<Vec<Issue>>> = Rc::new(RefCell::new(Vec::new()));
    let fill: Rc<dyn Fn()> = {
        let app = app.clone();
        let list = list.clone();
        let filter = filter.clone();
        let summary = summary.clone();
        let issues = issues.clone();
        Rc::new(move || {
            let all = {
                let st = app.state.borrow();
                check(&st.doc.lib, Some(&st.doc.file_dirs))
            };
            let (ne, nw, ni) = all.iter().fold((0, 0, 0), |a, i| match i.severity {
                Severity::Error => (a.0 + 1, a.1, a.2),
                Severity::Warning => (a.0, a.1 + 1, a.2),
                Severity::Info => (a.0, a.1, a.2 + 1),
            });
            summary.set_text(&format!("{ne} errors · {nw} warnings · {ni} suggestions"));
            let shown: Vec<Issue> = all
                .into_iter()
                .filter(|i| match filter.selected() {
                    0 => i.severity != Severity::Info,
                    2 => i.severity == Severity::Error,
                    3 => i.fix.is_some(),
                    _ => true,
                })
                .collect();
            while let Some(c) = list.first_child() {
                list.remove(&c);
            }
            const MAX: usize = 1200;
            for i in shown.iter().take(MAX) {
                list.append(&issue_row(&app, i));
            }
            if shown.len() > MAX {
                list.append(&gtk::Label::new(Some(&format!("… and {} more. Fix these first, or choose another filter.", shown.len() - MAX))));
            }
            if shown.is_empty() {
                let l = gtk::Label::new(Some("Nothing to report."));
                l.set_margin_top(30);
                list.append(&l);
            }
            *issues.borrow_mut() = shown;
        })
    };
    fill();
    let f2 = fill.clone();
    filter.connect_selected_notify(move |_| f2());
    let me = app.clone();
    let f3 = fill.clone();
    fix_all.connect_clicked(move |_| {
        let fixes: Vec<(usize, String, String)> =
            issues.borrow().iter().filter_map(|i| Some((i.item?, i.field.clone()?, i.fix.clone()?))).collect();
        if fixes.is_empty() {
            me.toast("Nothing in the list can be fixed automatically", None);
            return;
        }
        let n = apply_fixes(&me, fixes);
        me.toast(&format!("Fixed {n} entries"), None);
        f3();
    });
    win.present();
}

fn apply_fixes(app: &Rc<App>, fixes: Vec<(usize, String, String)>) -> usize {
    let mut by_entry: std::collections::BTreeMap<usize, Entry> = std::collections::BTreeMap::new();
    {
        let st = app.state.borrow();
        for (idx, field, value) in fixes {
            let e = by_entry.entry(idx).or_insert_with(|| st.doc.entry(idx).cloned().unwrap_or_else(|| Entry::new("misc", "")));
            // re-check that the field still has the problem value shape (it may have been fixed twice)
            e.set(&field, &value);
        }
    }
    let n = app.state.borrow_mut().doc.update_entries(by_entry.into_iter().collect());
    app.after_doc_change(None);
    n
}

fn issue_row(app: &Rc<App>, i: &Issue) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    row.set_margin_top(3);
    row.set_margin_bottom(3);
    let icon = gtk::Image::from_icon_name(match i.severity {
        Severity::Error => "dialog-error-symbolic",
        Severity::Warning => "dialog-warning-symbolic",
        Severity::Info => "dialog-information-symbolic",
    });
    row.append(&icon);
    let key = gtk::Button::with_label(if i.key.is_empty() { "(file)" } else { &i.key });
    key.add_css_class("flat");
    key.set_size_request(150, -1);
    if let Some(c) = key.child().and_downcast::<gtk::Label>() {
        c.set_xalign(0.0);
        c.set_ellipsize(pango::EllipsizeMode::End);
        c.add_css_class("monospace");
    }
    if let Some(idx) = i.item {
        let me = app.clone();
        key.connect_clicked(move |_| {
            me.w.search.set_text("");
            me.refilter(Some(idx));
            me.w.window.present();
        });
    }
    row.append(&key);
    let field = gtk::Label::new(i.field.as_deref());
    field.set_size_request(100, -1);
    field.set_xalign(0.0);
    field.add_css_class("dim-label");
    row.append(&field);
    let msg = gtk::Label::new(Some(&i.message));
    msg.set_wrap(true);
    msg.set_xalign(0.0);
    msg.set_hexpand(true);
    row.append(&msg);
    if let (Some(idx), Some(f), Some(fix)) = (i.item, i.field.clone(), i.fix.clone()) {
        let b = gtk::Button::with_label("Fix");
        b.set_tooltip_text(Some(&format!("→ {}", fix.chars().take(200).collect::<String>())));
        let me = app.clone();
        b.connect_clicked(move |b| {
            apply_fixes(&me, vec![(idx, f.clone(), fix.clone())]);
            b.set_sensitive(false);
            b.set_label("Fixed");
        });
        row.append(&b);
    }
    row.upcast()
}

// ---------- preferences ----------

pub fn show_preferences(app: &Rc<App>) {
    let win = dialog_window(app, "Preferences", 640, 520);
    let grid = gtk::Grid::builder().column_spacing(14).row_spacing(12).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
    let mut row = 0;
    let mut add = |label: &str, w: &gtk::Widget, help: Option<&str>| {
        let l = gtk::Label::new(Some(label));
        l.set_xalign(1.0);
        l.set_valign(gtk::Align::Start);
        l.set_margin_top(6);
        grid.attach(&l, 0, row, 1, 1);
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 4);
        bx.append(w);
        if let Some(h) = help {
            let hl = gtk::Label::new(Some(h));
            hl.add_css_class("dim-label");
            hl.add_css_class("caption");
            hl.set_wrap(true);
            hl.set_xalign(0.0);
            bx.append(&hl);
        }
        bx.set_hexpand(true);
        grid.attach(&bx, 1, row, 1, 1);
        row += 1;
    };
    let st = app.state.borrow();

    // PDF folder
    let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let dir_entry = gtk::Entry::builder().text(&st.settings.file_directory).hexpand(true).placeholder_text("e.g. /home/you/Documents/PDFs").build();
    let dir_btn = gtk::Button::with_label("Choose…");
    dir_row.append(&dir_entry);
    dir_row.append(&dir_btn);
    let found = st.doc.file_dirs.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ");
    let help = format!(
        "Relative file links are looked up here, then in JabRef's own PDF folder and next to the .bib file. Currently searched: {}",
        if found.is_empty() { "nothing".to_string() } else { found }
    );
    add("PDF folder", dir_row.upcast_ref(), Some(&help));

    let key_dd = gtk::DropDown::from_strings(&["wuja-2022 (author + year)", "wuja-scie (author + title word)"]);
    key_dd.set_selected(if st.settings.key_pattern == suci::keygen::KeyPattern::Auth4Title4 { 1 } else { 0 });
    add("New citation keys", key_dd.upcast_ref(), Some("First four letters of the first author's surname, diacritics removed; a, b, c… added if taken."));

    let fetch_dd = gtk::DropDown::from_strings(&["Unicode (ā, ś, ṛ) — for XeLaTeX/LuaLaTeX", "LaTeX accents ({\\=a}) — for pdfLaTeX"]);
    fetch_dd.set_selected(if st.settings.fetch_as_latex { 1 } else { 0 });
    add("Store fetched data as", fetch_dd.upcast_ref(), None);

    let moddate = gtk::Switch::builder().active(st.settings.track_modification_date).halign(gtk::Align::Start).build();
    add("Record modification dates", moddate.upcast_ref(), Some("Keeps JabRef's creationdate / modificationdate fields up to date."));

    let owner = gtk::Entry::builder().text(&st.settings.owner).placeholder_text("(none)").build();
    add("Owner for new entries", owner.upcast_ref(), Some("JabRef's “owner” field. Leave empty to omit."));

    let tc = gtk::Switch::builder().active(st.settings.style_title_case).halign(gtk::Align::Start).build();
    add(
        "Title Case in citations",
        tc.upcast_ref(),
        Some("Off: titles appear as you typed them. On: the style capitalises English-style (this also changes German, French and Sanskrit titles)."),
    );

    let styles_btn = gtk::Button::with_label("Open styles folder");
    add("More citation styles", styles_btn.upcast_ref(), Some("Download .csl files from the Zotero Style Repository and put them in this folder; restart to load them."));
    drop(st);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&grid);
    let close = gtk::Button::with_label("Done");
    close.add_css_class("suggested-action");
    close.set_halign(gtk::Align::End);
    close.set_margin_end(18);
    close.set_margin_bottom(18);
    content.append(&close);
    win.set_child(Some(&content));

    let w2 = win.clone();
    let de = dir_entry.clone();
    dir_btn.connect_clicked(move |_| {
        let d = gtk::FileDialog::builder().title("PDF folder").modal(true).build();
        let de2 = de.clone();
        d.select_folder(Some(&w2), None::<&gio::Cancellable>, move |r| {
            if let Ok(f) = r {
                if let Some(p) = f.path() {
                    de2.set_text(&p.display().to_string());
                }
            }
        });
    });
    let me = app.clone();
    styles_btn.connect_clicked(move |_| {
        if let Some(d) = suci::cite::user_style_dir() {
            let _ = std::fs::create_dir_all(&d);
            me.launch_uri(&format!("file://{}", d.display()));
        }
    });
    let me = app.clone();
    let w3 = win.clone();
    close.connect_clicked(move |_| w3.close());
    win.connect_close_request(move |_| {
        {
            let mut st = me.state.borrow_mut();
            st.settings.file_directory = dir_entry.text().to_string();
            st.settings.key_pattern =
                if key_dd.selected() == 1 { suci::keygen::KeyPattern::Auth4Title4 } else { suci::keygen::KeyPattern::Auth4Year };
            st.settings.fetch_as_latex = fetch_dd.selected() == 1;
            st.settings.track_modification_date = moddate.is_active();
            st.settings.owner = owner.text().trim().to_string();
            st.settings.style_title_case = tc.is_active();
            let s = st.settings.clone();
            st.doc.track_modification_date = s.track_modification_date;
            st.doc.key_pattern = s.key_pattern;
            st.doc.owner = s.owner.clone();
            st.doc.refresh_file_dirs(Some(&s.file_directory));
            st.styles.set_title_case(s.style_title_case);
            s.save();
        }
        super::editor::rebuild(&me);
        me.refresh_citation();
        glib::Propagation::Proceed
    });
    win.present();
}

// ---------- help ----------

pub fn show_shortcuts(app: &Rc<App>) {
    let win = dialog_window(app, "Keyboard shortcuts", 520, 560);
    let grid = gtk::Grid::builder().column_spacing(24).row_spacing(8).margin_top(18).margin_bottom(18).margin_start(18).margin_end(18).build();
    let items = [
        ("Ctrl+F", "Search"),
        ("↓ in search", "Jump into the list"),
        ("Ctrl+L", "Add from DOI / ISBN"),
        ("Ctrl+N", "New entry"),
        ("Ctrl+Shift+V", "Paste BibTeX (or a DOI/ISBN) from clipboard"),
        ("Ctrl+C (in list)", "Copy formatted reference(s)"),
        ("Ctrl+Shift+C", "Copy formatted reference(s) from anywhere"),
        ("Ctrl+Shift+K", "Copy citation key(s)"),
        ("Ctrl+P", "Open linked PDF"),
        ("Enter (in list)", "Edit the selected entry"),
        ("Delete (in list)", "Delete selected entries"),
        ("Ctrl+D (in list)", "Duplicate entry"),
        ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
        ("Ctrl+S", "Save"),
        ("Ctrl+O", "Open library"),
        ("F7", "Check library"),
        ("Ctrl+,", "Preferences"),
    ];
    for (i, (k, d)) in items.iter().enumerate() {
        let kl = gtk::Label::new(None);
        kl.set_markup(&format!("<b>{}</b>", esc(k)));
        kl.set_xalign(1.0);
        let dl = gtk::Label::new(Some(d));
        dl.set_xalign(0.0);
        grid.attach(&kl, 0, i as i32, 1, 1);
        grid.attach(&dl, 1, i as i32, 1, 1);
    }
    win.set_child(Some(&grid));
    win.present();
}

pub fn show_about(app: &Rc<App>) {
    let about = gtk::AboutDialog::builder()
        .transient_for(&app.w.window)
        .modal(true)
        .program_name(crate::APP_NAME)
        .version(env!("CARGO_PKG_VERSION"))
        .comments("A fast BibTeX / biblatex manager for scholars of South Asia.\nSūcī (Skt.) — “list, index”; also “needle”.\n\nCitation styles from the Citation Style Language project (CC BY-SA 3.0).")
        .license_type(gtk::License::MitX11)
        .logo_icon_name("io.github.suci.Suci")
        .build();
    about.present();
}
