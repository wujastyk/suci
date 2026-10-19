//! Entry editor and citation preview.

use super::App;
use gtk::prelude::*;
use gtk::{gdk, gio, glib, pango};
use std::collections::HashMap;
use std::rc::Rc;
use suci::bib::Entry;
use suci::checks::{check_entry, Severity};
use suci::files::{file_type_for, parse_file_field, serialize_file_field, LinkedFile};
use suci::profile::{layout, required_fields, BOOKKEEPING, MULTILINE};

/// biblatex entry types offered in the type selector, common ones first.
pub const TYPES: &[&str] = &[
    "Book", "Article", "InCollection", "Collection", "InBook", "MvBook", "Online", "Thesis", "Unpublished",
    "Manuscript", "InProceedings", "Proceedings", "Report", "Misc", "Review", "InReference", "Reference",
    "MvCollection", "BookInBook", "Booklet", "Manual", "Periodical", "Audio", "Video", "Software", "Dataset",
    "PhdThesis", "MastersThesis", "TechReport",
];

pub fn type_case(t: &str) -> String {
    TYPES.iter().find(|x| x.eq_ignore_ascii_case(t)).map(|s| s.to_string()).unwrap_or_else(|| {
        let mut c = t.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    })
}

pub fn type_display(t: &str) -> String {
    match t {
        "book" => "Book",
        "article" => "Article",
        "incollection" => "In collection",
        "collection" => "Collection",
        "inbook" => "In book",
        "mvbook" => "Multi-vol. book",
        "mvcollection" => "Multi-vol. coll.",
        "online" => "Online",
        "thesis" | "phdthesis" => "Thesis",
        "mastersthesis" => "MA thesis",
        "unpublished" => "Unpublished",
        "manuscript" => "Manuscript",
        "inproceedings" => "In proceedings",
        "proceedings" => "Proceedings",
        "report" | "techreport" => "Report",
        "misc" => "Misc",
        "review" => "Review",
        "inreference" => "In reference",
        "reference" => "Reference",
        "bookinbook" => "Book in book",
        "www" | "electronic" => "Web",
        other => return type_case(other),
    }
    .to_string()
}

fn field_label(name: &str) -> String {
    let n = name.to_lowercase();
    let s = match n.as_str() {
        "journaltitle" => "Journal",
        "booktitle" => "Book title",
        "booksubtitle" => "Book subtitle",
        "maintitle" => "Main title",
        "titleaddon" => "Title addon",
        "pagetotal" => "Total pages",
        "urldate" => "URL accessed",
        "langid" => "Language ID",
        "isbn" => "ISBN",
        "issn" => "ISSN",
        "doi" => "DOI",
        "url" => "URL",
        "eprinttype" => "Eprint type",
        "origdate" => "Original date",
        "origlanguage" => "Original language",
        "shorthand" => "Shorthand",
        "shorttitle" => "Short title",
        "shortlibrary" => "Short library",
        "relatedtype" => "Related type",
        "creationdate" => "Created",
        "modificationdate" => "Modified",
        "citationkey" => "Citation key field",
        "ppn_gvk" => "GVK PPN",
        "file" => "Files",
        "number" => "Number",
        "volumes" => "Volumes",
        "bookauthor" => "Book author",
        _ => "",
    };
    if !s.is_empty() {
        return s.to_string();
    }
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn clear(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

/// Rebuild the editor for the current selection.
pub fn rebuild(app: &Rc<App>) {
    app.state.borrow_mut().source_buf = None;
    let root = gtk::Box::new(gtk::Orientation::Vertical, 10);
    root.set_margin_top(12);
    root.set_margin_bottom(24);
    root.set_margin_start(14);
    root.set_margin_end(14);

    let (current, nsel) = {
        let st = app.state.borrow();
        (st.current, st.selected.len())
    };
    match current {
        None if nsel > 1 => build_multi(app, &root, nsel),
        None => {
            let l = gtk::Label::new(Some(if app.state.borrow().has_file { "Select an entry" } else { "" }));
            l.add_css_class("dim-label");
            l.set_margin_top(40);
            root.append(&l);
        }
        Some(idx) => build_entry(app, &root, idx),
    }
    // dropping a PDF onto the editor attaches it
    let dt = gtk::DropTarget::new(gdk::FileList::static_type(), gdk::DragAction::COPY);
    let me = app.clone();
    dt.connect_drop(move |_, v, _, _| {
        if let Ok(list) = v.get::<gdk::FileList>() {
            let paths: Vec<_> = list.files().iter().filter_map(|f| f.path()).collect();
            for p in paths {
                attach_file(&me, &p);
            }
            return true;
        }
        false
    });
    root.add_controller(dt);
    app.w.editor_scroller.set_child(Some(&root));
}

fn build_multi(app: &Rc<App>, root: &gtk::Box, n: usize) {
    let l = gtk::Label::new(Some(&format!("{n} entries selected")));
    l.add_css_class("title-3");
    l.set_halign(gtk::Align::Start);
    root.append(&l);
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    for (label, action) in [
        ("Copy references", "win.copy-ref"),
        ("Copy keys", "win.copy-key"),
        ("Copy \\cite{…}", "win.copy-latex"),
        ("Copy BibTeX", "win.copy-bibtex"),
    ] {
        let b = gtk::Button::with_label(label);
        b.set_action_name(Some(action));
        bx.append(&b);
    }
    root.append(&bx);
    let bx2 = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let conv = gtk::Button::with_label("LaTeX accents → Unicode");
    conv.set_action_name(Some("win.to-unicode-sel"));
    bx2.append(&conv);
    let del = gtk::Button::with_label("Delete");
    del.add_css_class("destructive-action");
    del.set_action_name(Some("win.delete"));
    bx2.append(&del);
    root.append(&bx2);
    let _ = app;
}

fn pending_or(app: &App, name: &str, e: &Entry) -> String {
    let st = app.state.borrow();
    st.pending.get(name).cloned().unwrap_or_else(|| e.get(name).unwrap_or_default())
}

/// Commit edits after the current event has been handled.
fn flush_later(app: &Rc<App>) {
    let me = app.clone();
    glib::idle_add_local_once(move || me.flush());
}

fn build_entry(app: &Rc<App>, root: &gtk::Box, idx: usize) {
    let (e, lay, biblatex, issues, dirs) = {
        let st = app.state.borrow();
        let Some(e) = st.doc.entry(idx).cloned() else { return };
        let present: Vec<String> = e.fields.iter().filter(|f| !f.editor_text().is_empty()).map(|f| f.name.clone()).collect();
        let biblatex = st.doc.lib.is_biblatex();
        let lay = layout(&st.doc.profile, &e.ty, &present, biblatex);
        let issues = check_entry(&st.doc.lib, idx, Some(&st.doc.file_dirs));
        (e, lay, biblatex, issues, st.doc.file_dirs.clone())
    };

    // ---- header: type, key, actions ----
    let header = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let type_names: Vec<String> = {
        let mut v: Vec<String> = TYPES.iter().map(|s| s.to_string()).collect();
        let cur = type_case(&e.ty);
        if !v.iter().any(|t| t.eq_ignore_ascii_case(&cur)) {
            v.insert(0, cur);
        }
        v
    };
    let type_dd = gtk::DropDown::from_strings(&type_names.iter().map(|s| s.as_str()).collect::<Vec<_>>());
    type_dd.set_tooltip_text(Some("Entry type"));
    if let Some(p) = type_names.iter().position(|t| t.eq_ignore_ascii_case(&e.ty)) {
        type_dd.set_selected(p as u32);
    }
    let me = app.clone();
    let names = type_names.clone();
    type_dd.connect_selected_notify(move |dd| {
        if let Some(t) = names.get(dd.selected() as usize) {
            let changed = {
                let st = me.state.borrow();
                st.current.and_then(|i| st.doc.entry(i)).is_some_and(|e| !e.ty.eq_ignore_ascii_case(t))
            };
            if changed {
                me.state.borrow_mut().pending_type = Some(t.clone());
                let me2 = me.clone();
                glib::idle_add_local_once(move || {
                    me2.flush();
                    rebuild(&me2);
                });
            }
        }
    });
    header.append(&type_dd);

    let key_entry = gtk::Entry::builder().text(&e.key).hexpand(true).tooltip_text("Citation key").build();
    key_entry.add_css_class("key-entry");
    key_entry.set_widget_name("key-entry");
    let me = app.clone();
    key_entry.connect_changed(move |k| {
        if let Ok(mut st) = me.state.try_borrow_mut() {
            st.pending_key = Some(k.text().to_string());
        }
    });
    connect_commit(app, key_entry.upcast_ref());
    let me = app.clone();
    key_entry.connect_activate(move |_| me.flush());
    header.append(&key_entry);

    let gen_btn = gtk::Button::builder().icon_name("view-refresh-symbolic").tooltip_text("Generate key from author and year").build();
    let me = app.clone();
    let ke = key_entry.clone();
    gen_btn.connect_clicked(move |_| {
        me.flush();
        let k = {
            let st = me.state.borrow();
            st.current.and_then(|i| st.doc.entry(i).map(|e| st.doc.generate_key(e, Some(i))))
        };
        if let Some(k) = k {
            ke.set_text(&k);
            me.flush();
        }
    });
    header.append(&gen_btn);

    let files = parse_file_field(&e.get("file").unwrap_or_default());
    let first_pdf = files.iter().find_map(|f| dirs.resolve(&f.path));
    let open_pdf = gtk::Button::builder().label("Open PDF").tooltip_text("Open the linked file (Ctrl+P)").build();
    open_pdf.set_sensitive(first_pdf.is_some());
    if first_pdf.is_none() && !files.is_empty() {
        open_pdf.set_tooltip_text(Some("The linked file was not found. Set the PDF folder in Preferences."));
    }
    open_pdf.set_action_name(Some("win.open-pdf"));
    header.append(&open_pdf);

    let attach = gtk::Button::builder().label("Attach…").tooltip_text("Link a PDF or other file to this entry (you can also drop a file here)").build();
    let me = app.clone();
    attach.connect_clicked(move |_| choose_attachment(&me));
    header.append(&attach);

    let more = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("More actions").build();
    let menu = gio::Menu::new();
    menu.append(Some("Edit BibTeX source (Ctrl+U)"), Some("win.toggle-source"));
    menu.append(Some("Copy BibTeX"), Some("win.copy-bibtex"));
    menu.append(Some("Copy \\cite{key}"), Some("win.copy-latex"));
    menu.append(Some("Duplicate"), Some("win.duplicate"));
    menu.append(Some("LaTeX accents → Unicode"), Some("win.to-unicode-sel"));
    menu.append(Some("Unicode → LaTeX accents"), Some("win.to-latex-sel"));
    menu.append(Some("Delete"), Some("win.delete"));
    more.set_menu_model(Some(&menu));
    header.append(&more);
    root.append(&header);

    // ---- Fields / BibTeX switch ----
    let show_source = app.state.borrow().show_source;
    let switch = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    switch.add_css_class("linked");
    switch.set_halign(gtk::Align::Start);
    switch.set_widget_name("view-switch");
    let fields_btn = gtk::ToggleButton::with_label("Fields");
    let source_btn = gtk::ToggleButton::with_label("BibTeX source");
    source_btn.set_group(Some(&fields_btn));
    source_btn.set_tooltip_text(Some("View and edit the raw biblatex text of this entry (Ctrl+U)"));
    fields_btn.set_active(!show_source);
    source_btn.set_active(show_source);
    let me = app.clone();
    source_btn.connect_toggled(move |b| {
        let want = b.is_active();
        if me.state.borrow().show_source != want {
            let me2 = me.clone();
            glib::idle_add_local_once(move || me2.set_source_mode(want));
        }
    });
    switch.append(&fields_btn);
    switch.append(&source_btn);
    root.append(&switch);

    // ---- issues for this entry ----
    let mut field_issues: HashMap<String, Vec<String>> = HashMap::new();
    for i in issues.iter().filter(|i| i.severity != Severity::Info || i.fix.is_some()) {
        if let Some(f) = &i.field {
            field_issues.entry(f.to_lowercase()).or_default().push(i.message.clone());
        }
    }
    let ib = gtk::Box::new(gtk::Orientation::Vertical, 2);
    ib.set_widget_name("issues-box");
    fill_issues(app, &ib, &issues);
    root.append(&ib);

    if show_source {
        build_source(app, root, &e);
        return;
    }


    // ---- fields ----
    let required: Vec<String> = required_fields(&e.ty_lower(), biblatex)
        .iter()
        .flat_map(|r| r.split('|').map(|s| s.to_string()).collect::<Vec<_>>())
        .collect();
    let extra = app.state.borrow().pending.keys().cloned().collect::<Vec<_>>();
    let mut main = lay.main.clone();
    for x in extra {
        if !main.iter().any(|m| m.eq_ignore_ascii_case(&x)) && !lay.other.iter().any(|m| m.eq_ignore_ascii_case(&x)) && !lay.bookkeeping.iter().any(|m| m.eq_ignore_ascii_case(&x)) {
            main.push(x);
        }
    }
    let grid = field_grid(app, &e, &main, &required, &field_issues, &dirs);
    root.append(&grid);

    if !lay.other.is_empty() {
        let t = gtk::Label::new(Some("Other fields in this entry"));
        t.add_css_class("section-title");
        t.set_halign(gtk::Align::Start);
        root.append(&t);
        root.append(&field_grid(app, &e, &lay.other, &required, &field_issues, &dirs));
    }

    // add a field
    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let add_entry = gtk::Entry::builder().placeholder_text("Add field…").width_chars(18).build();
    let add_menu = gtk::MenuButton::builder().icon_name("pan-down-symbolic").tooltip_text("Fields you use").build();
    let pop = gtk::Popover::new();
    let flow = gtk::FlowBox::builder().max_children_per_line(4).selection_mode(gtk::SelectionMode::None).build();
    let suggestions: Vec<String> = {
        let st = app.state.borrow();
        st.doc
            .profile
            .fields_by_frequency()
            .into_iter()
            .filter(|f| !e.fields.iter().any(|x| x.name.eq_ignore_ascii_case(f)) && !main.contains(f) && !BOOKKEEPING.contains(&f.as_str()))
            .take(32)
            .collect()
    };
    for s in suggestions {
        let b = gtk::Button::with_label(&s);
        b.add_css_class("flat");
        let me = app.clone();
        let pop2 = pop.clone();
        b.connect_clicked(move |_| {
            pop2.popdown();
            add_field(&me, &s);
        });
        flow.insert(&b, -1);
    }
    pop.set_child(Some(&flow));
    add_menu.set_popover(Some(&pop));
    let me = app.clone();
    add_entry.connect_activate(move |en| {
        let name: String = en.text().trim().to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-').collect();
        if !name.is_empty() {
            add_field(&me, &name);
        }
    });
    add_row.append(&add_entry);
    add_row.append(&add_menu);
    root.append(&add_row);

    if !lay.bookkeeping.is_empty() {
        let exp = gtk::Expander::new(Some("Bookkeeping"));
        exp.set_child(Some(&field_grid(app, &e, &lay.bookkeeping, &required, &field_issues, &dirs)));
        root.append(&exp);
    }
}

/// Set the Fields/BibTeX toggle without triggering a switch.
pub fn sync_view_toggle(app: &Rc<App>, source: bool) {
    if let Some(sw) = app.w.editor_scroller.child().and_then(|r| find_named(r, "view-switch")) {
        if let Some(src) = sw.last_child().and_downcast::<gtk::ToggleButton>() {
            if src.is_active() != source {
                src.set_active(source);
            }
        }
        if let Some(f) = sw.first_child().and_downcast::<gtk::ToggleButton>() {
            if f.is_active() == source {
                f.set_active(!source);
            }
        }
    }
}

/// Editable raw BibTeX of one entry.
fn build_source(app: &Rc<App>, root: &gtk::Box, e: &Entry) {
    let text = e.to_bibtex();
    let tv = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .vexpand(true)
        .build();
    tv.add_css_class("field-text");
    tv.buffer().set_text(&text);
    tv.set_size_request(-1, 360);
    let frame = gtk::Frame::new(None);
    frame.set_child(Some(&tv));
    frame.set_vexpand(true);
    let status = gtk::Label::new(Some("Edit the entry directly. Changes are applied when you press Apply (Ctrl+Enter), switch back to Fields, or select another entry."));
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.add_css_class("dim-label");
    status.add_css_class("caption");

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let revert = gtk::Button::with_label("Revert");
    revert.set_sensitive(false);
    let apply = gtk::Button::with_label("Apply");
    apply.add_css_class("suggested-action");
    apply.set_sensitive(false);
    buttons.append(&revert);
    buttons.append(&apply);

    app.state.borrow_mut().source_buf = Some((tv.buffer(), text.clone()));

    let (a2, r2) = (apply.clone(), revert.clone());
    let me = app.clone();
    tv.buffer().connect_changed(move |b| {
        let cur = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
        let dirty = me.state.try_borrow().ok().and_then(|st| st.source_buf.as_ref().map(|(_, o)| *o != cur)).unwrap_or(true);
        a2.set_sensitive(dirty);
        r2.set_sensitive(dirty);
    });
    let me = app.clone();
    let st2 = status.clone();
    let do_apply = Rc::new(move || match me.apply_source() {
        Ok(_) => rebuild(&me),
        Err(err) => {
            st2.set_text(&err);
            st2.remove_css_class("dim-label");
            st2.add_css_class("error");
        }
    });
    let da = do_apply.clone();
    apply.connect_clicked(move |_| da());
    let me = app.clone();
    revert.connect_clicked(move |_| rebuild(&me));
    // Ctrl+Enter applies
    let kc = gtk::EventControllerKey::new();
    let da = do_apply.clone();
    kc.connect_key_pressed(move |_, key, _, mods| {
        if mods.contains(gdk::ModifierType::CONTROL_MASK) && (key == gdk::Key::Return || key == gdk::Key::KP_Enter) {
            da();
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    tv.add_controller(kc);

    root.append(&frame);
    root.append(&status);
    root.append(&buttons);
    let tv2 = tv.clone();
    glib::idle_add_local_once(move || {
        tv2.grab_focus();
    });
}

fn fill_issues(app: &Rc<App>, ib: &gtk::Box, issues: &[suci::checks::Issue]) {
    clear(ib);
    for i in issues.iter().filter(|i| i.severity != Severity::Info || i.fix.is_some()) {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        row.add_css_class("issue-row");
        row.set_widget_name(&format!("issue:{}", i.message));
        let icon = gtk::Image::from_icon_name(match i.severity {
            Severity::Error => "dialog-error-symbolic",
            Severity::Warning => "dialog-warning-symbolic",
            Severity::Info => "dialog-information-symbolic",
        });
        row.append(&icon);
        let l = gtk::Label::new(Some(&i.message));
        l.set_xalign(0.0);
        l.set_wrap(true);
        l.set_hexpand(true);
        row.append(&l);
        if let (Some(fix), Some(field)) = (i.fix.clone(), i.field.clone()) {
            let b = gtk::Button::with_label("Fix");
            b.add_css_class("flat");
            b.set_tooltip_text(Some(&format!("Set {field} to: {fix}")));
            let me = app.clone();
            b.connect_clicked(move |_| {
                me.flush();
                let upd = {
                    let st = me.state.borrow();
                    st.current.and_then(|i| st.doc.entry(i).cloned().map(|mut e| {
                        e.set(&field, &fix);
                        (i, e)
                    }))
                };
                if let Some((i, e)) = upd {
                    me.state.borrow_mut().doc.update_entry(i, e);
                    super::list::refresh_row(&me, i);
                    me.update_title();
                    rebuild(&me);
                    me.refresh_citation();
                }
            });
            row.append(&b);
        }
        ib.append(&row);
    }
}

fn find_named(root: gtk::Widget, name: &str) -> Option<gtk::Widget> {
    let mut stack = vec![root];
    while let Some(w) = stack.pop() {
        if w.widget_name() == name {
            return Some(w);
        }
        let mut c = w.first_child();
        while let Some(ch) = c {
            stack.push(ch.clone());
            c = ch.next_sibling();
        }
    }
    None
}

fn add_field(app: &Rc<App>, name: &str) {
    app.flush();
    // a pending empty value makes the field appear; it is dropped if left empty
    app.state.borrow_mut().pending.insert(name.to_string(), String::new());
    rebuild(app);
    // focus the new field
    let name = name.to_string();
    let me = app.clone();
    glib::idle_add_local_once(move || focus_field(&me, &name));
}

fn focus_field(app: &Rc<App>, name: &str) {
    if let Some(root) = app.w.editor_scroller.child() {
        let mut stack = vec![root];
        while let Some(w) = stack.pop() {
            if w.widget_name() == format!("field-{name}") {
                w.grab_focus();
                return;
            }
            let mut c = w.first_child();
            while let Some(ch) = c {
                stack.push(ch.clone());
                c = ch.next_sibling();
            }
        }
    }
}

pub fn focus_first_field(app: &Rc<App>) {
    let first = {
        let st = app.state.borrow();
        st.current.and_then(|i| st.doc.entry(i)).map(|e| {
            let present: Vec<String> = e.fields.iter().map(|f| f.name.clone()).collect();
            layout(&st.doc.profile, &e.ty, &present, st.doc.lib.is_biblatex()).main.first().cloned().unwrap_or_else(|| "title".into())
        })
    };
    if let Some(f) = first {
        let me = app.clone();
        glib::idle_add_local_once(move || focus_field(&me, &f));
    }
}

fn connect_commit(app: &Rc<App>, w: &gtk::Widget) {
    let fc = gtk::EventControllerFocus::new();
    let me = app.clone();
    fc.connect_leave(move |_| flush_later(&me));
    w.add_controller(fc);
}

fn field_grid(
    app: &Rc<App>,
    e: &Entry,
    fields: &[String],
    required: &[String],
    issues: &HashMap<String, Vec<String>>,
    dirs: &suci::files::FileDirs,
) -> gtk::Grid {
    let grid = gtk::Grid::builder().column_spacing(10).row_spacing(6).build();
    for (row, name) in fields.iter().enumerate() {
        let lname = name.to_lowercase();
        let label = gtk::Label::new(Some(&field_label(name)));
        label.set_xalign(1.0);
        label.set_valign(gtk::Align::Start);
        label.set_margin_top(7);
        label.add_css_class("field-label");
        label.set_widget_name(&format!("label-{lname}"));
        if required.iter().any(|r| *r == lname) {
            label.add_css_class("required");
        }
        if let Some(msgs) = issues.get(&lname) {
            label.add_css_class("problem");
            label.set_tooltip_text(Some(&msgs.join("\n")));
        } else {
            label.set_tooltip_text(Some(name));
        }
        grid.attach(&label, 0, row as i32, 1, 1);

        let mut value = pending_or(app, name, e);
        // ARK is the usual eprint type here; it is saved once an eprint is entered
        if lname == "eprinttype" && value.trim().is_empty() {
            value = suci::files::DEFAULT_EPRINTTYPE.to_string();
        }
        let input: gtk::Widget = if MULTILINE.contains(&lname.as_str()) || value.chars().count() > 160 || value.contains('\n') {
            let tv = gtk::TextView::builder()
                .wrap_mode(gtk::WrapMode::WordChar)
                .accepts_tab(false)
                .top_margin(4)
                .bottom_margin(4)
                .left_margin(6)
                .right_margin(6)
                .build();
            tv.add_css_class("field-text");
            tv.buffer().set_text(&value);
            tv.set_widget_name(&format!("field-{lname}"));
            let me = app.clone();
            let n = name.clone();
            tv.buffer().connect_changed(move |b| {
                let t = b.text(&b.start_iter(), &b.end_iter(), false).to_string();
                if let Ok(mut st) = me.state.try_borrow_mut() {
                    st.pending.insert(n.clone(), t);
                }
            });
            connect_commit(app, tv.upcast_ref());
            let frame = gtk::Frame::new(None);
            frame.set_child(Some(&tv));
            frame.set_hexpand(true);
            tv.set_size_request(-1, if lname == "abstract" || lname == "annotation" || lname == "contents" { 110 } else { 56 });
            frame.upcast()
        } else {
            let en = gtk::Entry::builder().text(&value).hexpand(true).build();
            en.set_widget_name(&format!("field-{lname}"));
            let me = app.clone();
            let n = name.clone();
            en.connect_changed(move |en| {
                if let Ok(mut st) = me.state.try_borrow_mut() {
                    st.pending.insert(n.clone(), en.text().to_string());
                }
            });
            let me = app.clone();
            en.connect_activate(move |_| me.flush());
            connect_commit(app, en.upcast_ref());
            if matches!(lname.as_str(), "author" | "editor" | "translator" | "bookauthor") {
                en.set_tooltip_text(Some("Separate names with “and”: Patrick Olivelle and Suman Olivelle"));
            } else if lname == "date" {
                en.set_tooltip_text(Some("YYYY, YYYY-MM or YYYY-MM-DD; ranges as 1990/1995"));
            }
            en.upcast()
        };

        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        cell.append(&input);
        let value_trim = value.trim().to_string();
        match lname.as_str() {
            "doi" | "isbn" if !value_trim.is_empty() => {
                if lname == "doi" {
                    let b = icon_button("web-browser-symbolic", "Open at doi.org");
                    let me = app.clone();
                    let d = suci::checks::clean_doi(&value_trim);
                    b.connect_clicked(move |_| me.launch_uri(&format!("https://doi.org/{d}")));
                    cell.append(&b);
                }
                let b = icon_button("folder-download-symbolic", "Fetch data for this identifier and fill in missing fields");
                let me = app.clone();
                let v = value_trim.clone();
                b.connect_clicked(move |_| {
                    me.flush();
                    let target = me.state.borrow().current;
                    super::dialogs::start_fetch(&me, &v, target);
                });
                cell.append(&b);
            }
            "eprint" if !value_trim.is_empty() => {
                let ty = pending_or(app, "eprinttype", e);
                if let Some(u) = suci::files::eprint_url(&value_trim, &ty) {
                    let b = icon_button("web-browser-symbolic", &format!("Open {u}"));
                    let me = app.clone();
                    b.connect_clicked(move |_| me.launch_uri(&u));
                    cell.append(&b);
                }
            }
            "eprint" => {
                en_placeholder(&input, "ark:/13960/…  (or paste an n2t.net / archive.org ARK link)");
            }
            "url" if !value_trim.is_empty() => {
                let b = icon_button("web-browser-symbolic", "Open in browser");
                let me = app.clone();
                let u = value_trim.replace("\\&", "&").replace("\\_", "_").replace("\\%", "%");
                b.connect_clicked(move |_| me.launch_uri(&u));
                cell.append(&b);
            }
            "crossref" if !value_trim.is_empty() => {
                let b = icon_button("go-jump-symbolic", "Go to the cross-referenced entry");
                let me = app.clone();
                let k = value_trim.clone();
                b.connect_clicked(move |_| {
                    let i = me.state.borrow().doc.find_key(&k);
                    if let Some(i) = i {
                        me.w.search.set_text("");
                        me.refilter(Some(i));
                    }
                });
                cell.append(&b);
            }
            _ => {}
        }
        grid.attach(&cell, 1, row as i32, 1, 1);

        // linked files: one button per file below the raw field
        if lname == "file" && !value_trim.is_empty() {
            let fb = gtk::FlowBox::builder().selection_mode(gtk::SelectionMode::None).max_children_per_line(3).build();
            for lf in parse_file_field(&value_trim) {
                fb.insert(&file_button(app, &lf, dirs), -1);
            }
            // put the buttons in an extra row after the field
            let wrapper = gtk::Box::new(gtk::Orientation::Vertical, 4);
            cell.remove(&input);
            wrapper.append(&input);
            wrapper.append(&fb);
            wrapper.set_hexpand(true);
            cell.prepend(&wrapper);
        }
    }
    grid
}

fn en_placeholder(w: &gtk::Widget, text: &str) {
    if let Some(en) = w.downcast_ref::<gtk::Entry>() {
        en.set_placeholder_text(Some(text));
    }
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::builder().icon_name(icon).tooltip_text(tip).valign(gtk::Align::Start).build();
    b.add_css_class("flat");
    b
}

fn file_button(app: &Rc<App>, lf: &LinkedFile, dirs: &suci::files::FileDirs) -> gtk::Widget {
    let name = std::path::Path::new(&lf.path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or(lf.path.clone());
    let resolved = if lf.path.starts_with("http") { None } else { dirs.resolve(&lf.path) };
    let b = gtk::Button::new();
    let l = gtk::Label::new(Some(&name));
    l.set_ellipsize(pango::EllipsizeMode::Middle);
    l.set_max_width_chars(40);
    b.set_child(Some(&l));
    b.add_css_class("flat");
    match resolved {
        Some(p) => {
            b.set_tooltip_text(Some(&format!("Open {}", p.display())));
            let me = app.clone();
            b.connect_clicked(move |_| me.launch_file(&p));
        }
        None if lf.path.starts_with("http") => {
            let me = app.clone();
            let u = lf.path.clone();
            b.set_tooltip_text(Some(&u));
            b.connect_clicked(move |_| me.launch_uri(&u));
        }
        None => {
            l.add_css_class("missing-file");
            let dirs_text = dirs.dirs.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join("\n");
            b.set_tooltip_text(Some(&format!(
                "File not found: {}\nLooked in:\n{}\n(Set the PDF folder in Preferences.)",
                lf.path,
                if dirs_text.is_empty() { "(no folders configured)".into() } else { dirs_text }
            )));
        }
    }
    b.upcast()
}

fn choose_attachment(app: &Rc<App>) {
    let dialog = gtk::FileDialog::builder().title("Attach file").modal(true).build();
    let f = gtk::FileFilter::new();
    f.set_name(Some("Documents (PDF, DjVu, EPUB)"));
    for p in ["*.pdf", "*.PDF", "*.djvu", "*.epub"] {
        f.add_pattern(p);
    }
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&f);
    store.append(&all);
    dialog.set_filters(Some(&store));
    if let Some(d) = app.state.borrow().doc.file_dirs.dirs.first() {
        dialog.set_initial_folder(Some(&gio::File::for_path(d)));
    }
    let me = app.clone();
    dialog.open(Some(&app.w.window), None::<&gio::Cancellable>, move |res| {
        if let Ok(file) = res {
            if let Some(p) = file.path() {
                attach_file(&me, &p);
            }
        }
    });
}

pub fn attach_file(app: &Rc<App>, path: &std::path::Path) {
    app.flush();
    let upd = {
        let st = app.state.borrow();
        let Some(i) = st.current else { return };
        let Some(mut e) = st.doc.entry(i).cloned() else { return };
        let mut files = parse_file_field(&e.get("file").unwrap_or_default());
        let stored = st.doc.file_dirs.relativize(path);
        if files.iter().any(|f| f.path == stored) {
            return;
        }
        files.push(LinkedFile { description: String::new(), path: stored, file_type: file_type_for(path) });
        e.set("file", &serialize_file_field(&files));
        (i, e)
    };
    app.state.borrow_mut().doc.update_entry(upd.0, upd.1);
    super::list::refresh_row(app, upd.0);
    app.update_title();
    rebuild(app);
    app.toast(&format!("Attached {}", path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()), None);
}

/// Update widgets that show derived values (key) after a commit, without
/// rebuilding the whole editor (which would lose focus).
pub fn refresh_header(app: &Rc<App>) {
    let key = {
        let st = app.state.borrow();
        st.current.and_then(|i| st.doc.entry(i)).map(|e| e.key.clone())
    };
    let Some(key) = key else { return };
    let issues = {
        let st = app.state.borrow();
        st.current.map(|i| check_entry(&st.doc.lib, i, Some(&st.doc.file_dirs))).unwrap_or_default()
    };
    // Update the warning list in place without changing its size, so the
    // fields below don't jump while typing: solved problems are ticked off;
    // new ones are flagged on their field label (and listed on the next visit).
    if let Some(ib) = app.w.editor_scroller.child().and_then(|r| find_named(r, "issues-box")).and_downcast::<gtk::Box>() {
        let current: std::collections::HashSet<&str> = issues.iter().map(|i| i.message.as_str()).collect();
        let mut row = ib.first_child();
        while let Some(r) = row {
            if let Some(msg) = r.widget_name().strip_prefix("issue:") {
                let solved = !current.contains(msg);
                if let Some(icon) = r.first_child().and_downcast::<gtk::Image>() {
                    if solved {
                        icon.set_icon_name(Some("object-select-symbolic"));
                        r.add_css_class("dim-label");
                    }
                }
                if solved {
                    if let Some(btn) = r.last_child().and_downcast::<gtk::Button>() {
                        btn.set_visible(false);
                    }
                }
            }
            row = r.next_sibling();
        }
    }
    // show values that were tidied on commit (e.g. eprint links), without
    // touching the field the user is typing in
    let synced: Vec<(gtk::Entry, String, String)> = {
        let st = app.state.borrow();
        let e = st.current.and_then(|i| st.doc.entry(i));
        let mut v = Vec::new();
        if let (Some(e), Some(root)) = (e, app.w.editor_scroller.child()) {
            let mut stack = vec![root];
            while let Some(w) = stack.pop() {
                if let Some(f) = w.widget_name().strip_prefix("field-") {
                    if let Some(en) = w.downcast_ref::<gtk::Entry>() {
                        let stored = e.get(f).unwrap_or_default();
                        if !en.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) && !st.pending.contains_key(f) && !stored.is_empty() && en.text() != stored {
                            v.push((en.clone(), f.to_string(), stored));
                        }
                    }
                }
                let mut c = w.first_child();
                while let Some(ch) = c {
                    stack.push(ch.clone());
                    c = ch.next_sibling();
                }
            }
        }
        v
    };
    for (en, f, val) in synced {
        en.set_text(&val);
        app.state.borrow_mut().pending.remove(&f);
    }

    // re-mark field labels that have problems
    let mut by_field: HashMap<String, Vec<String>> = HashMap::new();
    for i in issues.iter().filter(|i| i.severity != Severity::Info || i.fix.is_some()) {
        if let Some(f) = &i.field {
            by_field.entry(f.to_lowercase()).or_default().push(i.message.clone());
        }
    }
    if let Some(root) = app.w.editor_scroller.child() {
        let mut stack = vec![root];
        while let Some(w) = stack.pop() {
            let name = w.widget_name();
            if let Some(f) = name.strip_prefix("label-") {
                match by_field.get(f) {
                    Some(m) => {
                        w.add_css_class("problem");
                        w.set_tooltip_text(Some(&m.join("\n")));
                    }
                    None => {
                        w.remove_css_class("problem");
                        w.set_tooltip_text(Some(f));
                    }
                }
            }
            let mut c = w.first_child();
            while let Some(ch) = c {
                stack.push(ch.clone());
                c = ch.next_sibling();
            }
        }
    }
    if let Some(root) = app.w.editor_scroller.child() {
        let mut stack = vec![root];
        while let Some(w) = stack.pop() {
            if w.widget_name() == "key-entry" {
                if let Some(en) = w.downcast_ref::<gtk::Entry>() {
                    if en.text() != key && !en.state_flags().contains(gtk::StateFlags::FOCUS_WITHIN) {
                        en.set_text(&key);
                        app.state.borrow_mut().pending_key = None;
                    }
                }
                return;
            }
            let mut c = w.first_child();
            while let Some(ch) = c {
                stack.push(ch.clone());
                c = ch.next_sibling();
            }
        }
    }
}

// ---------- citation preview ----------

pub fn refresh_citation(app: &Rc<App>) {
    let cb = &app.w.cite_box;
    if cb.first_child().is_none() {
        cb.add_css_class("cite-box");
        cb.set_margin_start(10);
        cb.set_margin_end(10);
        cb.set_margin_top(10);
        cb.set_margin_bottom(10);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        app.w.style_dd.set_hexpand(true);
        row.append(&app.w.style_dd);
        let copy = gtk::Button::builder().label("Copy reference").tooltip_text("Copy the formatted reference with italics, for email or a word processor (Ctrl+C in the list)").build();
        copy.add_css_class("suggested-action");
        copy.set_action_name(Some("win.copy-ref"));
        let copy_cite = gtk::Button::builder().label("Copy citation").tooltip_text("Copy the in-text citation, or the note form for notes styles").build();
        copy_cite.set_action_name(Some("win.copy-cite"));
        let copy_key = gtk::Button::builder().label("Key").tooltip_text("Copy the citation key (Ctrl+Shift+K)").build();
        copy_key.set_action_name(Some("win.copy-key"));
        row.append(&copy);
        row.append(&copy_cite);
        row.append(&copy_key);
        cb.append(&row);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        cb.append(&content);
    }
    let Some(content) = cb.last_child().and_downcast::<gtk::Box>() else { return };
    clear(&content);

    let (sel, style) = {
        let st = app.state.borrow();
        (st.selected.clone(), st.settings.citation_style.clone())
    };
    if sel.is_empty() {
        let l = gtk::Label::new(Some("The formatted reference of the selected entry appears here."));
        l.add_css_class("dim-label");
        l.set_xalign(0.0);
        content.append(&l);
        return;
    }
    let shown: Vec<usize> = sel.iter().copied().take(12).collect();
    let rendered = {
        let st = app.state.borrow();
        let entries: Vec<Entry> = shown.iter().filter_map(|&i| st.doc.entry(i).cloned()).collect();
        let refs: Vec<&Entry> = entries.iter().collect();
        let lookup = |k: &str| st.doc.lookup_entry(k);
        st.styles.render(&style, &refs, &lookup)
    };
    match rendered {
        Ok(r) => {
            for m in &r.bib_markup {
                let l = gtk::Label::new(None);
                l.set_markup(m);
                l.set_wrap(true);
                l.set_wrap_mode(pango::WrapMode::WordChar);
                l.set_xalign(0.0);
                l.set_selectable(true);
                l.add_css_class("cite-text");
                content.append(&l);
            }
            if sel.len() > shown.len() {
                let l = gtk::Label::new(Some(&format!("… and {} more", sel.len() - shown.len())));
                l.add_css_class("dim-label");
                l.set_xalign(0.0);
                content.append(&l);
            }
            if sel.len() == 1 {
                if let Some(c) = r.citations.first() {
                    let l = gtk::Label::new(Some(&format!("{}{}", if r.note_style { "Note: " } else { "In text: " }, c)));
                    l.add_css_class("cite-intext");
                    l.set_wrap(true);
                    l.set_xalign(0.0);
                    l.set_selectable(true);
                    content.append(&l);
                }
            }
        }
        Err(err) => {
            let l = gtk::Label::new(Some(&format!("Could not format: {err}")));
            l.set_wrap(true);
            l.set_xalign(0.0);
            content.append(&l);
        }
    }
}
