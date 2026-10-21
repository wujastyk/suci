//! Title capitalisation and shortDOI actions.

use super::App;
use gtk::prelude::*;
use gtk::{gio, glib};
use std::rc::Rc;
use suci::bib::Entry;
use suci::titlecase;

impl App {
    /// Apply the "capitalise automatically" and "short DOIs" preferences to a
    /// new entry. DOI shortening happens in the background after adding.
    pub fn prepare_new_entry(&self, e: &mut Entry) {
        if self.state.borrow().settings.auto_title_case && titlecase::auto_applies(e) {
            titlecase::apply(e);
        }
    }

    pub fn after_entry_added(self: &Rc<Self>, idx: usize) {
        if self.state.borrow().settings.short_dois {
            self.shorten_dois(vec![idx], true);
        }
    }

    /// Capitalise titles of the selected entries. A single entry is always
    /// done (you asked for it); with several, non-English entries are skipped.
    pub fn titlecase_selected(self: &Rc<Self>) {
        self.flush();
        let sel = self.state.borrow().selected.clone();
        let single = sel.len() == 1;
        let (updates, skipped, warn): (Vec<(usize, Entry)>, usize, Option<String>) = {
            let st = self.state.borrow();
            let mut ups = Vec::new();
            let mut skipped = 0;
            let mut warn = None;
            for &i in &sel {
                let Some(mut e) = st.doc.entry(i).cloned() else { continue };
                if !single && !titlecase::auto_applies(&e) {
                    skipped += 1;
                    continue;
                }
                if single && titlecase::tagged_english(&e) == Some(false) {
                    warn = Some(format!("Note: {} is tagged as {}", e.key, e.get("langid").or_else(|| e.get("language")).unwrap_or_default()));
                }
                if titlecase::apply(&mut e) {
                    ups.push((i, e));
                }
            }
            (ups, skipped, warn)
        };
        let n = self.state.borrow_mut().doc.update_entries(updates);
        self.after_doc_change(None);
        let mut msg = match n {
            0 => "Titles already capitalised".to_string(),
            1 => "Capitalised the title".to_string(),
            n => format!("Capitalised titles of {n} entries"),
        };
        if skipped > 0 {
            msg.push_str(&format!(" ({skipped} non-English skipped)"));
        }
        if let Some(w) = warn {
            msg = format!("{msg}. {w}");
        }
        let me = self.clone();
        self.toast(&msg, if n > 0 { Some(Box::new(move || me.undo_public())) } else { None });
    }

    /// Capitalise one field in the editor (right-click on a title field).
    pub fn titlecase_field(self: &Rc<Self>, field: &str) {
        let value = {
            let st = self.state.borrow();
            let Some(e) = st.current.and_then(|i| st.doc.entry(i)) else { return };
            st.pending.get(field).cloned().unwrap_or_else(|| e.get(field).unwrap_or_default())
        };
        let new = titlecase::title_case(&value);
        if new == value {
            self.toast("Already capitalised", None);
            return;
        }
        self.state.borrow_mut().pending.insert(field.to_string(), new.clone());
        super::editor::set_field_widget(self, field, &new);
        self.flush();
    }

    /// Whole library, with a preview first.
    pub fn titlecase_library(self: &Rc<Self>) {
        self.flush();
        let (updates, examples, skipped) = {
            let st = self.state.borrow();
            let mut ups = Vec::new();
            let mut ex = Vec::new();
            let mut skipped = 0;
            for i in st.doc.entry_indices() {
                let e = st.doc.entry(i).unwrap();
                if !titlecase::auto_applies(e) {
                    skipped += 1;
                    continue;
                }
                let mut x = e.clone();
                if titlecase::apply(&mut x) {
                    if ex.len() < 60 {
                        let before = suci::latex::to_plain(&e.get("title").unwrap_or_default());
                        let after = suci::latex::to_plain(&x.get("title").unwrap_or_default());
                        if before != after {
                            ex.push((before, after));
                        }
                    }
                    ups.push((i, x));
                }
            }
            (ups, ex, skipped)
        };
        if updates.is_empty() {
            self.toast("All English titles are already capitalised", None);
            return;
        }
        let n = updates.len();
        let win = gtk::Window::builder()
            .title("Capitalise English titles")
            .transient_for(&self.w.window)
            .modal(true)
            .default_width(760)
            .default_height(560)
            .build();
        let outer = gtk::Box::new(gtk::Orientation::Vertical, 10);
        outer.set_margin_top(16);
        outer.set_margin_bottom(16);
        outer.set_margin_start(16);
        outer.set_margin_end(16);
        let head = gtk::Label::new(None);
        head.set_markup(&format!(
            "<b>{n} entries would change.</b>\n{skipped} entries are left alone: tagged as another language, containing Devanagari, Bengali etc., or not looking English.",
        ));
        head.set_wrap(true);
        head.set_xalign(0.0);
        outer.append(&head);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
        for (b, a) in &examples {
            let l1 = gtk::Label::new(Some(b));
            l1.set_wrap(true);
            l1.set_xalign(0.0);
            l1.add_css_class("dim-label");
            let l2 = gtk::Label::new(Some(&format!("→ {a}")));
            l2.set_wrap(true);
            l2.set_xalign(0.0);
            list.append(&l1);
            list.append(&l2);
        }
        let sc = gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        outer.append(&sc);
        let note = gtk::Label::new(Some(&format!("Showing {} examples. This is one step that Ctrl+Z undoes.", examples.len())));
        note.add_css_class("dim-label");
        note.set_xalign(0.0);
        outer.append(&note);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Cancel");
        let ok = gtk::Button::with_label(&format!("Capitalise {n} entries"));
        ok.add_css_class("suggested-action");
        buttons.append(&cancel);
        buttons.append(&ok);
        outer.append(&buttons);
        win.set_child(Some(&outer));
        let w2 = win.clone();
        cancel.connect_clicked(move |_| w2.close());
        let me = self.clone();
        let w2 = win.clone();
        let updates = std::cell::RefCell::new(Some(updates));
        ok.connect_clicked(move |_| {
            if let Some(u) = updates.borrow_mut().take() {
                let n = me.state.borrow_mut().doc.update_entries(u);
                me.after_doc_change(None);
                let me2 = me.clone();
                me.toast(&format!("Capitalised titles in {n} entries"), Some(Box::new(move || me2.undo_public())));
            }
            w2.close();
        });
        win.present();
    }

    /// Replace DOIs of these entries by shortDOIs (https://shortdoi.org), in the background.
    pub fn shorten_dois(self: &Rc<Self>, idxs: Vec<usize>, quiet: bool) {
        self.flush();
        let jobs: Vec<(usize, String)> = {
            let st = self.state.borrow();
            idxs.iter()
                .filter_map(|&i| {
                    let d = st.doc.entry(i)?.get_nonempty("doi")?;
                    let d = suci::checks::clean_doi(&d);
                    (!suci::fetch::is_short_doi(&d)).then_some((i, d))
                })
                .collect()
        };
        if jobs.is_empty() {
            if !quiet {
                self.toast("No full-length DOIs to shorten", None);
            }
            return;
        }
        let n = jobs.len();
        if !quiet {
            self.toast(&format!("Asking shortdoi.org for {n} short {}…", if n == 1 { "DOI" } else { "DOIs" }), None);
        }
        self.w.fetch_spinner.start();
        let me = self.clone();
        glib::spawn_future_local(async move {
            let results = gio::spawn_blocking(move || {
                let mut out = Vec::new();
                for (k, (i, d)) in jobs.into_iter().enumerate() {
                    if k > 0 {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                    }
                    out.push((i, d.clone(), suci::fetch::shorten_doi(&d)));
                }
                out
            })
            .await
            .unwrap_or_default();
            me.w.fetch_spinner.stop();
            me.flush();
            let mut errors = Vec::new();
            let updates: Vec<(usize, Entry)> = {
                let st = me.state.borrow();
                results
                    .into_iter()
                    .filter_map(|(i, long, r)| match r {
                        Ok(short) => {
                            let mut e = st.doc.entry(i)?.clone();
                            // only if the DOI was not edited meanwhile
                            if suci::checks::clean_doi(&e.get("doi").unwrap_or_default()) != long {
                                return None;
                            }
                            e.set("doi", &short);
                            Some((i, e))
                        }
                        Err(err) => {
                            errors.push(format!("{long}: {err}"));
                            None
                        }
                    })
                    .collect()
            };
            let done = me.state.borrow_mut().doc.update_entries(updates);
            if done > 0 {
                me.after_doc_change(None);
            }
            if !errors.is_empty() && (!quiet || done == 0) {
                me.error(&format!("{} DOI(s) could not be shortened", errors.len()), &errors.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
            } else if done > 0 && !quiet {
                me.toast(&format!("Shortened {done} {}", if done == 1 { "DOI" } else { "DOIs" }), None);
            }
        });
    }

    pub fn shorten_selected(self: &Rc<Self>) {
        let sel = self.state.borrow().selected.clone();
        self.shorten_dois(sel, false);
    }

    /// Add an entry for the next volume of the current multi-volume work (or of
    /// the set the current volume belongs to).
    pub fn add_volume(self: &Rc<Self>) {
        self.flush();
        let target = {
            let st = self.state.borrow();
            let Some(cur) = st.current.and_then(|i| st.doc.entry(i)) else { return };
            if super::editor::volume_type(&cur.ty_lower()).is_some() {
                Some(cur.clone())
            } else {
                cur.get_nonempty("crossref")
                    .and_then(|k| st.doc.lookup_entry(k.trim()))
                    .filter(|p| super::editor::volume_type(&p.ty_lower()).is_some())
            }
        };
        let Some(parent) = target else {
            self.toast("Volumes can be added to multi-volume entries (type MvBook, MvCollection …). Change the entry type first.", None);
            return;
        };
        let ty = super::editor::volume_type(&parent.ty_lower()).unwrap();
        let idx = {
            let mut st = self.state.borrow_mut();
            let kids = st.doc.children_of(&parent.key);
            let next = kids
                .iter()
                .filter_map(|&i| st.doc.entry(i).and_then(suci::doc::volume_number))
                .max()
                .unwrap_or(0)
                + 1;
            // key in the library's style: parent key + volume number (kavi-sadv2)
            let keys = st.doc.keys();
            let key = suci::keygen::unique_key(&format!("{}{}", parent.key, next), |k| keys.contains(k));
            let mut e = Entry::new(ty, &key);
            e.set("crossref", &parent.key);
            e.set("volume", &next.to_string());
            st.doc.add_entry(e)
        };
        self.w.search.set_text("");
        self.after_doc_change(Some(idx));
        let key = self.state.borrow().doc.entry(idx).map(|e| e.key.clone()).unwrap_or_default();
        self.toast(&format!("Added {key}. Fill in its ARK or URL (and its own title or date, if any)."), None);
        let me = self.clone();
        glib::idle_add_local_once(move || super::editor::focus_named_field(&me, "eprint"));
    }

    /// Right-click menu on the entry list.
    pub fn setup_list_context_menu(self: &Rc<Self>) {
        let menu = gio::Menu::new();
        let s1 = gio::Menu::new();
        s1.append(Some("Copy Reference"), Some("win.copy-ref"));
        s1.append(Some("Copy Citation"), Some("win.copy-cite"));
        s1.append(Some("Copy Key"), Some("win.copy-key"));
        s1.append(Some("Copy BibTeX"), Some("win.copy-bibtex"));
        menu.append_section(None, &s1);
        let s2 = gio::Menu::new();
        s2.append(Some("Capitalise Titles (English)"), Some("win.titlecase-sel"));
        s2.append(Some("Shorten DOI (shortDOI)"), Some("win.shorten-doi-sel"));
        s2.append(Some("LaTeX Accents → Unicode"), Some("win.to-unicode-sel"));
        menu.append_section(None, &s2);
        let s3 = gio::Menu::new();
        s3.append(Some("Open Link (URL / DOI)"), Some("win.open-link"));
        s3.append(Some("Add Volume…"), Some("win.add-volume"));
        s3.append(Some("Edit BibTeX Source"), Some("win.toggle-source"));
        s3.append(Some("Open PDF"), Some("win.open-pdf"));
        s3.append(Some("Duplicate"), Some("win.duplicate"));
        s3.append(Some("Delete"), Some("win.delete"));
        menu.append_section(None, &s3);

        let g = gtk::GestureClick::new();
        g.set_button(3);
        g.set_propagation_phase(gtk::PropagationPhase::Capture);
        let me = self.clone();
        g.connect_pressed(move |g, _, x, y| {
            let view = &me.w.view;
            // which row was clicked?
            if let Some(w) = view.pick(x, y, gtk::PickFlags::DEFAULT) {
                let mut cur = Some(w);
                let mut found = None;
                while let Some(c) = cur {
                    if let Some(l) = c.downcast_ref::<gtk::Label>() {
                        found = me.bound.borrow().iter().find_map(|(idx, v)| {
                            v.iter().any(|(_, wl)| wl.upgrade().is_some_and(|x| &x == l)).then_some(*idx)
                        });
                    }
                    if found.is_some() {
                        break;
                    }
                    cur = c.parent();
                    if cur.as_ref().is_some_and(|p| p == view.upcast_ref::<gtk::Widget>()) {
                        break;
                    }
                }
                if let Some(idx) = found {
                    if !me.state.borrow().selected.contains(&idx) {
                        me.select_entry(idx);
                    }
                }
            }
            let pop = gtk::PopoverMenu::from_model(Some(&menu));
            pop.set_parent(view);
            pop.set_has_arrow(false);
            pop.set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
            g.set_state(gtk::EventSequenceState::Claimed);
        });
        self.w.view.add_controller(g);
    }
}
