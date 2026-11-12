//! Right-click → Copy to Library: copy the selected entries into another .bib file.

use super::App;
use gtk::prelude::*;
use gtk::{gio, glib, pango};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use suci::bib::Entry;
use suci::doc::Document;
use suci::latex::to_plain;
use suci::transfer::{apply, plan, suggest_key, Action, Clash, Planned};

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

/// A folder for display, with the home folder as "~".
fn folder(p: &Path) -> String {
    let d = p.parent().map(|d| d.display().to_string()).unwrap_or_default();
    match dirs::home_dir().map(|h| h.display().to_string()) {
        Some(h) if !h.is_empty() && d.starts_with(&h) => format!("~{}", &d[h.len()..]),
        _ => d,
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| p.display().to_string())
}

/// "Wujastyk 2003, The Roots of Ayurveda"
fn summary(e: &Entry) -> String {
    let who = e
        .get_nonempty("author")
        .or_else(|| e.get_nonempty("editor"))
        .map(|a| suci::names::parse_names(&a).first().map(|n| to_plain(&n.last)).unwrap_or_default())
        .unwrap_or_default();
    let year: String = e.get_nonempty("date").or_else(|| e.get_nonempty("year")).unwrap_or_default().chars().take(4).collect();
    let title = to_plain(&e.get_nonempty("title").or_else(|| e.get_nonempty("name")).unwrap_or_default());
    let head = [who, year].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    let mut t: String = title.chars().take(90).collect();
    if title.chars().count() > 90 {
        t.push('…');
    }
    if head.is_empty() { t } else { format!("{head}, {t}") }
}

/// Fill the "Copy to Library" submenu from the recent files (all but the open one).
pub fn update_menu(app: &Rc<App>) {
    let menu = &app.w.copy_to_menu;
    menu.remove_all();
    let (recent, current): (Vec<PathBuf>, Option<PathBuf>) = {
        let st = app.state.borrow();
        (st.settings.recent_files.iter().filter(|p| p.exists()).cloned().collect(), st.doc.path.clone())
    };
    let same = |a: &Path, b: &Path| a == b || a.canonicalize().ok() == b.canonicalize().ok();
    let section = gio::Menu::new();
    for p in recent.iter().filter(|p| !current.as_deref().is_some_and(|c| same(c, p))) {
        let label = format!("{}  —  {}", file_name(p), folder(p));
        let item = gio::MenuItem::new(Some(&label), None);
        item.set_action_and_target_value(Some("win.copy-to"), Some(&p.display().to_string().to_variant()));
        section.append_item(&item);
    }
    menu.append_section(None, &section);
    let other = gio::Menu::new();
    other.append(Some("Other Library…"), Some("win.copy-to-other"));
    menu.append_section(None, &other);
}

pub fn choose_other(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder().title("Copy entries to library").modal(true).build();
    dialog.set_filters(Some(&super::bib_filters()));
    let me = app.clone();
    dialog.open(Some(&app.w.window), None::<&gio::Cancellable>, move |res| {
        if let Ok(f) = res {
            if let Some(p) = f.path() {
                copy_to(&me, &p);
            }
        }
    });
}

pub fn copy_to(app: &Rc<App>, path: &Path) {
    app.flush();
    let idxs = app.selected_indices();
    let entries: Vec<Entry> = {
        let st = app.state.borrow();
        idxs.iter().filter_map(|&i| st.doc.entry(i).cloned()).collect()
    };
    if entries.is_empty() {
        app.toast("Select the entries to copy first", None);
        return;
    }
    if app.state.borrow().doc.path.as_deref().is_some_and(|c| c == path || c.canonicalize().ok() == path.canonicalize().ok()) {
        app.toast("That is the library already open", None);
        return;
    }
    let mut target = {
        let st = app.state.borrow();
        match Document::open(path, Some(&st.settings.file_directory)) {
            Ok(mut d) => {
                d.track_modification_date = st.settings.track_modification_date;
                d.key_pattern = st.settings.key_pattern;
                d.owner = st.settings.owner.clone();
                d
            }
            Err(e) => {
                drop(st);
                app.error("Could not open the other library", &e);
                return;
            }
        }
    };
    if !target.lib.errors.is_empty() {
        app.error(
            "Not copied",
            &format!("{} has {} part(s) that Sūcī cannot read. Open it and check it first, so that nothing in it is lost.", file_name(path), target.lib.errors.len()),
        );
        return;
    }
    let planned = plan(&app.state.borrow().doc, &target, &entries);

    // suggested keys, distinct from the target's and from each other
    let mut taken: HashSet<String> = planned.iter().filter(|p| matches!(p.clash, Clash::None)).map(|p| p.entry.key.to_lowercase()).collect();
    let mut defaults = Vec::new();
    for p in &planned {
        let key = if matches!(p.clash, Clash::None) {
            p.entry.key.clone()
        } else {
            let k = suggest_key(&target, &p.entry.key, &taken);
            taken.insert(k.to_lowercase());
            k
        };
        defaults.push((key.clone(), p.default_action(&key)));
    }

    let needs_choice = planned.iter().any(|p| !matches!(p.clash, Clash::None) || p.same_work.is_some());
    if !needs_choice {
        let actions: Vec<Action> = defaults.into_iter().map(|(_, a)| a).collect();
        finish(app, path, &mut target, &planned, &actions, None);
        return;
    }
    show_choices(app, path, target, planned, defaults);
}

fn finish(app: &Rc<App>, path: &Path, target: &mut Document, planned: &[Planned], actions: &[Action], err_label: Option<&gtk::Label>) -> bool {
    let res = apply(&app.state.borrow().doc, target, planned, actions);
    let out = match res {
        Ok(o) => o,
        Err(e) => {
            match err_label {
                Some(l) => {
                    l.set_text(&e);
                    l.set_visible(true);
                }
                None => app.error("Not copied", &e),
            }
            return false;
        }
    };
    let name = file_name(path);
    if out.added.is_empty() && out.replaced.is_empty() {
        app.toast(&format!("Nothing copied to {name}"), None);
        return true;
    }
    if let Err(e) = target.save(false) {
        let msg = match e {
            suci::doc::SaveError::ChangedOnDisk => format!("{name} was changed by another program while copying. Nothing was written; please try again."),
            suci::doc::SaveError::Io(m) => m,
            suci::doc::SaveError::NoPath => "no file".into(),
        };
        app.error("Not copied", &msg);
        return false;
    }
    let renamed: Vec<String> = planned
        .iter()
        .zip(actions)
        .filter_map(|(p, a)| match a {
            Action::Add(k) if k.trim() != p.entry.key => Some(format!("{} as {}", p.entry.key, k.trim())),
            _ => None,
        })
        .collect();
    let mut msg = match out.added.len() {
        0 => String::new(),
        1 => format!("Copied {} to {name}", out.added[0]),
        n => format!("Copied {n} entries to {name}"),
    };
    if !renamed.is_empty() {
        msg.push_str(&format!(" ({})", renamed.join(", ")));
    }
    if !out.replaced.is_empty() {
        if msg.is_empty() {
            msg = format!("Replaced {} in {name}", out.replaced.join(", "));
        } else {
            msg.push_str(&format!("; replaced {}", out.replaced.join(", ")));
        }
    }
    if !out.skipped.is_empty() {
        msg.push_str(&format!("; {} not copied", out.skipped.len()));
    }
    let me = app.clone();
    let p = path.to_path_buf();
    app.toast_labelled(
        &msg,
        "Open",
        Some(Box::new(move || {
            let me2 = me.clone();
            let p = p.clone();
            me.confirm_discard(move || me2.open_file(&p));
        })),
    );
    true
}

struct Row {
    add: gtk::CheckButton,
    key: gtk::Entry,
    replace: Option<gtk::CheckButton>,
}

fn show_choices(app: &Rc<App>, path: &Path, target: Document, planned: Vec<Planned>, defaults: Vec<(String, Action)>) {
    let name = file_name(path);
    let win = gtk::Window::builder()
        .title(format!("Copy to {name}"))
        .transient_for(&app.w.window)
        .modal(true)
        .default_width(760)
        .default_height({
            let n = planned.iter().filter(|p| !matches!(p.clash, Clash::None) || p.same_work.is_some() || p.as_parent).count() as i32;
            (190 + 150 * n).min(760)
        })
        .build();
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
    outer.set_margin_top(16);
    outer.set_margin_bottom(14);
    outer.set_margin_start(16);
    outer.set_margin_end(16);
    let head = gtk::Label::new(None);
    let fine = planned.iter().filter(|p| matches!(p.clash, Clash::None) && p.same_work.is_none() && !p.as_parent).count();
    head.set_markup(&format!(
        "Copying to <b>{}</b> <span size='small'>({})</span>.{}",
        esc(&name),
        esc(&folder(path)),
        if fine > 0 { format!(" {fine} entr{} can be copied as they are; these need a decision:", if fine == 1 { "y" } else { "ies" }) } else { " These need a decision:".into() }
    ));
    head.set_wrap(true);
    head.set_xalign(0.0);
    outer.append(&head);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 14);
    let mut rows: Vec<Option<Row>> = Vec::new();
    for (p, (key, action)) in planned.iter().zip(&defaults) {
        let issue = match (&p.clash, &p.same_work) {
            (Clash::KeyTaken(theirs), same) => Some(format!(
                "{name} already has a <b>different</b> entry with the key <tt>{}</tt>: {}{}",
                esc(&p.entry.key),
                esc(&summary(theirs)),
                same.as_ref().map(|k| format!(". It also has this work already, as <tt>{}</tt>.", esc(k))).unwrap_or_default()
            )),
            (Clash::Identical, _) => Some(format!("{name} already has this entry (<tt>{}</tt>), with the same content.", esc(&p.entry.key))),
            (Clash::None, Some(k)) => Some(format!("{name} already has this work, as <tt>{}</tt>.", esc(k))),
            (Clash::None, None) if p.as_parent => Some(format!("The set that a copied volume belongs to (crossref); {name} does not have it yet, so it is copied too.")),
            _ => None,
        };
        let Some(issue) = issue else {
            rows.push(None);
            continue;
        };
        let card = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let t = gtk::Label::new(None);
        t.set_markup(&format!("<b><tt>{}</tt></b>  {}", esc(&p.entry.key), esc(&summary(&p.entry))));
        t.set_xalign(0.0);
        t.set_wrap(true);
        t.set_wrap_mode(pango::WrapMode::WordChar);
        card.append(&t);
        let i = gtk::Label::new(None);
        i.set_markup(&issue);
        i.set_xalign(0.0);
        i.set_wrap(true);
        i.set_wrap_mode(pango::WrapMode::WordChar);
        i.add_css_class("dim-label");
        card.append(&i);

        let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let add = gtk::CheckButton::with_label(if matches!(p.clash, Clash::None) { "Copy, with the key" } else { "Copy, with the new key" });
        let key_entry = gtk::Entry::builder().text(key).width_chars(24).build();
        add_row.append(&add);
        add_row.append(&key_entry);
        card.append(&add_row);
        let replace = match &p.clash {
            Clash::KeyTaken(_) => Some(gtk::CheckButton::with_label(&format!("Replace the entry {} in {name} with this one", p.entry.key))),
            Clash::Identical => None,
            Clash::None => None,
        };
        if let Some(r) = &replace {
            r.set_group(Some(&add));
            card.append(r);
        }
        let skip = gtk::CheckButton::with_label("Don't copy it");
        skip.set_group(Some(&add));
        card.append(&skip);
        match action {
            Action::Add(_) => add.set_active(true),
            Action::Replace => replace.as_ref().unwrap_or(&add).set_active(true),
            Action::Skip => skip.set_active(true),
        }
        {
            let k = key_entry.clone();
            add.connect_toggled(move |b| k.set_sensitive(b.is_active()));
            key_entry.set_sensitive(add.is_active());
        }
        {
            // typing a key means: copy it under that key
            let a = add.clone();
            key_entry.connect_changed(move |_| a.set_active(true));
        }
        list.append(&card);
        rows.push(Some(Row { add, key: key_entry, replace }));
    }
    let sc = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
    outer.append(&sc);

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("error");
    err.set_visible(false);
    outer.append(&err);

    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    bar.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let ok = gtk::Button::with_label("Copy");
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
    let path = path.to_path_buf();
    let target = std::cell::RefCell::new(Some(target));
    ok.connect_clicked(move |_| {
        let actions: Vec<Action> = rows
            .iter()
            .zip(&defaults)
            .map(|(r, (_, d))| match r {
                None => d.clone(),
                Some(r) if r.add.is_active() => Action::Add(r.key.text().trim().to_string()),
                Some(r) if r.replace.as_ref().is_some_and(|x| x.is_active()) => Action::Replace,
                Some(_) => Action::Skip,
            })
            .collect();
        // work on a copy, so that a refused choice can be corrected and retried
        let Some(mut t) = target.borrow().as_ref().map(|t| Document::open(t.path.as_deref().unwrap(), None)) .and_then(|r| r.ok()) else { return };
        if let Some(orig) = target.borrow().as_ref() {
            t.file_dirs = orig.file_dirs.clone();
            t.track_modification_date = orig.track_modification_date;
            t.key_pattern = orig.key_pattern;
            t.owner = orig.owner.clone();
        }
        if finish(&me, &path, &mut t, &planned, &actions, Some(&err)) {
            w.close();
        }
    });
    win.present();
}
