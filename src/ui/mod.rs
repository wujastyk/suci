//! GTK 4 interface.

mod dialogs;
mod editor;
mod list;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::rc::Rc;

use suci::bib::Entry;
use suci::cite::Styles;
use suci::doc::{Document, SaveError};
use suci::settings::Settings;

pub use list::Row;

const CSS: &str = r#"
.field-label { color: alpha(currentColor, 0.62); font-size: 0.92em; }
.field-label.required { color: currentColor; font-weight: bold; }
.field-label.problem { color: #c01c28; }
.cite-box { background: alpha(currentColor, 0.045); border-radius: 8px; padding: 10px 12px; }
.cite-text { font-size: 1.05em; }
.cite-intext { color: alpha(currentColor, 0.65); }
.section-title { font-weight: bold; font-size: 0.85em; color: alpha(currentColor, 0.55); margin-top: 6px; }
.toast { background: #303030; color: white; border-radius: 18px; padding: 6px 8px 6px 16px; margin-bottom: 18px; }
.toast button { color: white; }
.count-label { color: alpha(currentColor, 0.6); font-size: 0.9em; }
.missing-file { color: #c01c28; }
.issue-row { padding: 2px 4px; }
.key-entry { font-family: monospace; }
.placeholder-title { font-size: 1.6em; font-weight: bold; }
.fetch-entry { min-width: 260px; }
textview.field-text { padding: 4px; }
"#;

pub fn load_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(CSS);
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    }
}

pub struct State {
    pub doc: Document,
    pub settings: Settings,
    pub styles: Styles,
    pub rows: Vec<Option<Row>>,
    /// Item index shown in the editor.
    pub current: Option<usize>,
    /// Field edits not yet committed to the document.
    pub pending: BTreeMap<String, String>,
    pub pending_key: Option<String>,
    pub pending_type: Option<String>,
    /// Item indices of all selected entries.
    pub selected: Vec<usize>,
    pub has_file: bool,
    /// Show the entry as editable BibTeX source instead of fields.
    pub show_source: bool,
    /// The source editor's buffer and the text it was loaded with.
    pub source_buf: Option<(gtk::TextBuffer, String)>,
}

pub struct Widgets {
    pub window: gtk::ApplicationWindow,
    pub store: gtk::StringList,
    pub sort_model: gtk::SortListModel,
    pub selection: gtk::MultiSelection,
    pub view: gtk::ColumnView,
    pub search: gtk::SearchEntry,
    pub count: gtk::Label,
    pub editor_scroller: gtk::ScrolledWindow,
    pub cite_box: gtk::Box,
    pub style_dd: gtk::DropDown,
    pub fetch_entry: gtk::Entry,
    pub fetch_spinner: gtk::Spinner,
    pub toast_revealer: gtk::Revealer,
    pub toast_label: gtk::Label,
    pub toast_button: gtk::Button,
    pub stack: gtk::Stack,
    pub save_btn: gtk::Button,
}

pub struct App {
    pub state: RefCell<State>,
    pub w: Widgets,
    /// Labels currently showing each row (item index → column labels).
    pub bound: RefCell<HashMap<usize, Vec<(list::Col, glib::WeakRef<gtk::Label>)>>>,
    toast_action: RefCell<Option<Box<dyn Fn()>>>,
    toast_serial: Rc<std::cell::Cell<u32>>,
    /// An edit changed sort-relevant data; re-sort when the user moves on.
    needs_resort: std::cell::Cell<bool>,
}

impl App {
    pub fn build(app: &gtk::Application, open: Option<PathBuf>) -> Rc<App> {
        let settings = Settings::load();
        let mut styles = Styles::load();
        styles.set_title_case(settings.style_title_case);

        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(crate::APP_NAME)
            .default_width(settings.window_width)
            .default_height(settings.window_height)
            .build();

        // ---- header bar ----
        let header = gtk::HeaderBar::new();
        let open_btn = gtk::Button::builder().icon_name("document-open-symbolic").tooltip_text("Open library (Ctrl+O)").build();
        open_btn.set_action_name(Some("win.open"));
        let recent_btn = gtk::MenuButton::builder().icon_name("pan-down-symbolic").tooltip_text("Recent libraries").build();
        let open_group = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        open_group.add_css_class("linked");
        open_group.append(&open_btn);
        open_group.append(&recent_btn);
        let save_btn = gtk::Button::builder().icon_name("document-save-symbolic").tooltip_text("Save (Ctrl+S)").build();
        save_btn.set_action_name(Some("win.save"));
        let undo_btn = gtk::Button::builder().icon_name("edit-undo-symbolic").tooltip_text("Undo (Ctrl+Z)").build();
        undo_btn.set_action_name(Some("win.undo"));
        let redo_btn = gtk::Button::builder().icon_name("edit-redo-symbolic").tooltip_text("Redo (Ctrl+Shift+Z)").build();
        redo_btn.set_action_name(Some("win.redo"));
        let ur = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        ur.add_css_class("linked");
        ur.append(&undo_btn);
        ur.append(&redo_btn);
        header.pack_start(&open_group);
        header.pack_start(&save_btn);
        header.pack_start(&ur);

        let new_btn = gtk::MenuButton::builder().icon_name("list-add-symbolic").tooltip_text("New entry (Ctrl+N)").build();
        header.pack_start(&new_btn);

        // DOI / ISBN fetch box, front and centre
        let fetch_entry = gtk::Entry::builder()
            .placeholder_text("Add from DOI or ISBN…")
            .tooltip_text("Paste a DOI (10.xxxx/…, doi.org link) or an ISBN and press Enter (Ctrl+L)")
            .primary_icon_name("list-add-symbolic")
            .build();
        fetch_entry.add_css_class("fetch-entry");
        let fetch_spinner = gtk::Spinner::new();
        let fetch_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        fetch_box.append(&fetch_entry);
        fetch_box.append(&fetch_spinner);
        header.set_title_widget(Some(&fetch_box));

        let menu_btn = gtk::MenuButton::builder().icon_name("open-menu-symbolic").tooltip_text("Menu").build();
        menu_btn.set_menu_model(Some(&main_menu()));
        header.pack_end(&menu_btn);
        let check_btn = gtk::Button::builder().icon_name("emblem-ok-symbolic").tooltip_text("Check library integrity").label("Check").build();
        check_btn.set_action_name(Some("win.check"));
        header.pack_end(&check_btn);
        window.set_titlebar(Some(&header));

        // ---- entry list ----
        let store = gtk::StringList::new(&[]);
        let sort_model = gtk::SortListModel::new(Some(store.clone()), None::<gtk::Sorter>);
        let selection = gtk::MultiSelection::new(Some(sort_model.clone()));
        let view = gtk::ColumnView::new(Some(selection.clone()));
        view.set_show_row_separators(false);
        view.set_show_column_separators(false);
        view.add_css_class("data-table");
        let list_scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).hexpand(true).build();

        let search = gtk::SearchEntry::builder()
            .placeholder_text("Search  (e.g. olivelle  dharmasastra  author:pollock  year:1990..2000  has:file)")
            .hexpand(true)
            .build();
        search.set_tooltip_markup(Some(
            "Words match anywhere, ignoring case and diacritics (<i>sastra</i> finds <i>śāstra</i>).\n\
             <b>\"a phrase\"</b>  exact phrase\n\
             <b>author:</b>name  <b>title:</b>word  <b>key:</b>wuja  <b>type:</b>book  — search one field\n\
             <b>year:</b>1990..2000  <b>year:</b>..1950  — year range\n\
             <b>has:</b>file  <b>has:</b>doi  <b>-has:</b>isbn  — field present / absent\n\
             <b>-</b>word  — exclude",
        ));
        let count = gtk::Label::new(None);
        count.add_css_class("count-label");
        let search_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        search_row.set_margin_start(8);
        search_row.set_margin_end(8);
        search_row.set_margin_top(6);
        search_row.set_margin_bottom(6);
        search_row.append(&search);
        search_row.append(&count);
        let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
        left.append(&search_row);
        left.append(&list_scroll);

        // ---- right pane: citation + editor ----
        let style_dd = gtk::DropDown::from_strings(&styles.list.iter().map(|s| s.title.as_str()).collect::<Vec<_>>());
        style_dd.set_tooltip_text(Some("Citation style. Add more by putting .csl files in ~/.config/suci/styles"));
        if let Some(pos) = styles.list.iter().position(|s| s.id == settings.citation_style) {
            style_dd.set_selected(pos as u32);
        }
        let cite_box = gtk::Box::new(gtk::Orientation::Vertical, 6);
        let editor_scroller = gtk::ScrolledWindow::builder().vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let right = gtk::Box::new(gtk::Orientation::Vertical, 0);
        right.append(&cite_box);
        right.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        right.append(&editor_scroller);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&left));
        paned.set_end_child(Some(&right));
        paned.set_position(settings.pane_position);
        paned.set_resize_start_child(true);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);

        // ---- placeholder when no file is open ----
        let placeholder = gtk::Box::new(gtk::Orientation::Vertical, 12);
        placeholder.set_valign(gtk::Align::Center);
        placeholder.set_halign(gtk::Align::Center);
        let ptitle = gtk::Label::new(Some("No library open"));
        ptitle.add_css_class("placeholder-title");
        let popen = gtk::Button::with_label("Open a .bib file…");
        popen.add_css_class("suggested-action");
        popen.set_action_name(Some("win.open"));
        let pnew = gtk::Button::with_label("Start a new library");
        pnew.set_action_name(Some("win.new-library"));
        placeholder.append(&ptitle);
        placeholder.append(&popen);
        placeholder.append(&pnew);

        let stack = gtk::Stack::new();
        stack.add_named(&placeholder, Some("empty"));
        stack.add_named(&paned, Some("main"));

        // ---- toast ----
        let toast_label = gtk::Label::new(None);
        let toast_button = gtk::Button::with_label("Undo");
        toast_button.add_css_class("flat");
        let toast_close = gtk::Button::from_icon_name("window-close-symbolic");
        toast_close.add_css_class("flat");
        let toast = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        toast.add_css_class("toast");
        toast.append(&toast_label);
        toast.append(&toast_button);
        toast.append(&toast_close);
        let toast_revealer = gtk::Revealer::builder()
            .child(&toast)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .transition_type(gtk::RevealerTransitionType::SlideUp)
            .build();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&stack));
        overlay.add_overlay(&toast_revealer);
        window.set_child(Some(&overlay));

        let doc = Document::new_empty();
        let app_rc = Rc::new(App {
            state: RefCell::new(State {
                doc,
                settings,
                styles,
                rows: Vec::new(),
                current: None,
                pending: BTreeMap::new(),
                pending_key: None,
                pending_type: None,
                selected: Vec::new(),
                has_file: false,
                show_source: false,
                source_buf: None,
            }),
            w: Widgets {
                window: window.clone(),
                store,
                sort_model,
                selection,
                view,
                search,
                count,
                editor_scroller,
                cite_box,
                style_dd,
                fetch_entry,
                fetch_spinner,
                toast_revealer,
                toast_label,
                toast_button,
                stack,
                save_btn,
            },
            bound: RefCell::new(HashMap::new()),
            toast_action: RefCell::new(None),
            toast_serial: Rc::new(std::cell::Cell::new(0)),
            needs_resort: std::cell::Cell::new(false),
        });

        list::setup_columns(&app_rc);
        app_rc.setup_actions(app);
        app_rc.setup_signals(&new_btn, &recent_btn, &paned, &toast_close);
        app_rc.update_recent_menu(&recent_btn);

        window.present();

        // open the requested or last file
        let to_open = open.or_else(|| app_rc.state.borrow().settings.last_file.clone()).filter(|p| p.exists());
        match to_open {
            Some(p) => app_rc.open_file(&p),
            None => app_rc.w.stack.set_visible_child_name("empty"),
        }
        app_rc
    }

    fn setup_actions(self: &Rc<Self>, app: &gtk::Application) {
        let win = &self.w.window;
        let add = |name: &str, f: Box<dyn Fn(&Rc<App>)>| {
            let a = gio::SimpleAction::new(name, None);
            let me = self.clone();
            a.connect_activate(move |_, _| f(&me));
            win.add_action(&a);
        };
        add("open", Box::new(|a| a.choose_open()));
        add("save", Box::new(|a| a.save(false)));
        add("save-as", Box::new(|a| a.choose_save_as()));
        add("new-library", Box::new(|a| a.new_library()));
        add("undo", Box::new(|a| a.undo()));
        add("redo", Box::new(|a| a.redo()));
        add("check", Box::new(|a| dialogs::show_checks(a)));
        add("prefs", Box::new(|a| dialogs::show_preferences(a)));
        add("search", Box::new(|a| {
            a.w.search.grab_focus();
        }));
        add("fetch", Box::new(|a| {
            a.w.fetch_entry.grab_focus();
        }));
        add("new-entry", Box::new(|a| a.new_entry("Book")));
        add("delete", Box::new(|a| a.delete_selected()));
        add("copy-ref", Box::new(|a| a.copy_references()));
        add("copy-cite", Box::new(|a| a.copy_citations()));
        add("copy-key", Box::new(|a| a.copy_keys(false)));
        add("copy-latex", Box::new(|a| a.copy_keys(true)));
        add("copy-bibtex", Box::new(|a| a.copy_bibtex()));
        add("paste-bibtex", Box::new(|a| a.paste_bibtex()));
        add("to-unicode-sel", Box::new(|a| a.convert_selected(true)));
        add("to-latex-sel", Box::new(|a| a.convert_selected(false)));
        add("to-unicode-all", Box::new(|a| a.convert_all_to_unicode()));
        add("shortcuts", Box::new(|a| dialogs::show_shortcuts(a)));
        add("about", Box::new(|a| dialogs::show_about(a)));
        add("open-pdf", Box::new(|a| a.open_first_pdf()));
        add("duplicate", Box::new(|a| a.duplicate_selected()));
        add("toggle-source", Box::new(|a| {
            let on = !a.state.borrow().show_source;
            a.set_source_mode(on);
        }));

        // sort order as a stateful action
        let mode = self.state.borrow().settings.sort_mode;
        let sort_action = gio::SimpleAction::new_stateful(
            "sort-order",
            Some(glib::VariantTy::STRING),
            &(if mode == suci::collate::SortMode::Sanskrit { "sanskrit" } else { "latin" }).to_variant(),
        );
        let me = self.clone();
        sort_action.connect_activate(move |a, v| {
            if let Some(v) = v {
                a.set_state(v);
                let sanskrit = v.str() == Some("sanskrit");
                me.state.borrow_mut().settings.sort_mode =
                    if sanskrit { suci::collate::SortMode::Sanskrit } else { suci::collate::SortMode::Latin };
                me.state.borrow().settings.save();
                list::resort(&me);
            }
        });
        win.add_action(&sort_action);

        app.set_accels_for_action("win.open", &["<Control>o"]);
        app.set_accels_for_action("win.save", &["<Control>s"]);
        app.set_accels_for_action("win.save-as", &["<Control><Shift>s"]);
        app.set_accels_for_action("win.search", &["<Control>f"]);
        app.set_accels_for_action("win.fetch", &["<Control>l"]);
        app.set_accels_for_action("win.new-entry", &["<Control>n"]);
        app.set_accels_for_action("win.check", &["F7"]);
        app.set_accels_for_action("win.shortcuts", &["<Control>question", "F1"]);
        app.set_accels_for_action("win.prefs", &["<Control>comma"]);
        app.set_accels_for_action("win.open-pdf", &["<Control>p"]);
        app.set_accels_for_action("win.copy-ref", &["<Control><Shift>c"]);
        app.set_accels_for_action("win.copy-key", &["<Control><Shift>k"]);
        app.set_accels_for_action("win.paste-bibtex", &["<Control><Shift>v"]);
        app.set_accels_for_action("win.toggle-source", &["<Control>u"]);
    }

    fn setup_signals(self: &Rc<Self>, new_btn: &gtk::MenuButton, _recent: &gtk::MenuButton, paned: &gtk::Paned, toast_close: &gtk::Button) {
        // search
        let me = self.clone();
        self.w.search.connect_search_changed(move |_| {
            me.flush();
            me.refilter(None);
        });
        let me = self.clone();
        self.w.search.connect_stop_search(move |s| {
            s.set_text("");
            me.w.view.grab_focus();
        });
        // Down arrow from search jumps into the list
        let me = self.clone();
        let kc = gtk::EventControllerKey::new();
        kc.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Down {
                me.w.view.grab_focus();
                if me.w.selection.selection().size() == 0 && me.w.sort_model.n_items() > 0 {
                    me.w.selection.select_item(0, true);
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.w.search.add_controller(kc);

        // selection
        let me = self.clone();
        self.w.selection.connect_selection_changed(move |_, _, _| {
            me.on_selection_changed();
        });

        // fetch
        let me = self.clone();
        self.w.fetch_entry.connect_activate(move |e| {
            let t = e.text().to_string();
            dialogs::start_fetch(&me, &t, None);
        });

        // style
        let me = self.clone();
        self.w.style_dd.connect_selected_notify(move |dd| {
            let id = {
                let st = me.state.borrow();
                st.styles.list.get(dd.selected() as usize).map(|s| s.id.clone())
            };
            if let Some(id) = id {
                me.state.borrow_mut().settings.citation_style = id;
                me.state.borrow().settings.save();
                me.refresh_citation();
            }
        });

        // new-entry popover with the types used in this library
        let me = self.clone();
        new_btn.connect_notify_local(Some("active"), move |b, _| {
            if b.is_active() {
                b.set_popover(Some(&me.new_entry_popover()));
                if let Some(p) = b.popover() {
                    p.popup();
                }
            }
        });
        new_btn.set_popover(Some(&gtk::Popover::new()));

        // remember pane position & window size
        let me = self.clone();
        paned.connect_position_notify(move |p| {
            if let Ok(mut st) = me.state.try_borrow_mut() {
                st.settings.pane_position = p.position();
            }
        });

        let me = self.clone();
        toast_close.connect_clicked(move |_| me.w.toast_revealer.set_reveal_child(false));
        let me = self.clone();
        self.w.toast_button.connect_clicked(move |_| {
            me.w.toast_revealer.set_reveal_child(false);
            let f = me.toast_action.borrow_mut().take();
            if let Some(f) = f {
                f();
            }
        });

        // close: ask to save
        let me = self.clone();
        self.w.window.connect_close_request(move |w| {
            me.flush();
            {
                let mut st = me.state.borrow_mut();
                st.settings.window_width = w.width();
                st.settings.window_height = w.height();
                st.settings.save();
            }
            if me.state.borrow().doc.is_dirty() {
                let me2 = me.clone();
                me.ask_save_then(move || {
                    me2.state.borrow_mut().doc = Document::new_empty();
                    me2.w.window.destroy();
                });
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });

        // list keyboard shortcuts (local, so text fields keep their own Ctrl+C/Ctrl+Z)
        let sc = gtk::ShortcutController::new();
        let add_sc = |trigger: &str, action: &str| {
            sc.add_shortcut(gtk::Shortcut::new(
                gtk::ShortcutTrigger::parse_string(trigger),
                Some(gtk::NamedAction::new(action)),
            ));
        };
        add_sc("<Control>c", "win.copy-ref");
        add_sc("Delete", "win.delete");
        add_sc("<Control>z", "win.undo");
        add_sc("<Control><Shift>z|<Control>y", "win.redo");
        add_sc("<Control>d", "win.duplicate");
        self.w.view.add_controller(sc);
        // the same undo/redo shortcuts in the editor when no text field handles them
        let sc2 = gtk::ShortcutController::new();
        sc2.set_propagation_phase(gtk::PropagationPhase::Bubble);
        sc2.add_shortcut(gtk::Shortcut::new(gtk::ShortcutTrigger::parse_string("<Control>z"), Some(gtk::NamedAction::new("win.undo"))));
        sc2.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("<Control><Shift>z|<Control>y"),
            Some(gtk::NamedAction::new("win.redo")),
        ));
        self.w.window.add_controller(sc2);

        // Enter on list focuses the editor's first field
        let me = self.clone();
        self.w.view.connect_activate(move |_, _| {
            editor::focus_first_field(&me);
        });
    }

    fn new_entry_popover(self: &Rc<Self>) -> gtk::Popover {
        let pop = gtk::Popover::new();
        let bx = gtk::Box::new(gtk::Orientation::Vertical, 2);
        bx.set_margin_top(6);
        bx.set_margin_bottom(6);
        let mut types: Vec<String> = self.state.borrow().doc.profile.common_types();
        for t in ["book", "article", "incollection", "collection", "inbook", "online", "thesis", "mvbook", "manuscript", "unpublished", "misc"] {
            if !types.iter().any(|x| x == t) {
                types.push(t.into());
            }
        }
        types.retain(|t| t != "comment" && t != "string" && t != "preamble");
        for t in types.iter().take(14) {
            let label = editor::type_display(t);
            let b = gtk::Button::with_label(&label);
            b.add_css_class("flat");
            if let Some(c) = b.child() {
                c.set_halign(gtk::Align::Start);
            }
            let me = self.clone();
            let ty = editor::type_case(t);
            let pop2 = pop.clone();
            b.connect_clicked(move |_| {
                pop2.popdown();
                me.new_entry(&ty);
            });
            bx.append(&b);
        }
        bx.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let paste = gtk::Button::with_label("Paste BibTeX from clipboard");
        paste.add_css_class("flat");
        paste.set_action_name(Some("win.paste-bibtex"));
        let pop2 = pop.clone();
        paste.connect_clicked(move |_| pop2.popdown());
        bx.append(&paste);
        pop.set_child(Some(&bx));
        pop
    }

    pub fn update_recent_menu(self: &Rc<Self>, btn: &gtk::MenuButton) {
        let menu = gio::Menu::new();
        let recent = self.state.borrow().settings.recent_files.clone();
        let section = gio::Menu::new();
        for (i, p) in recent.iter().enumerate() {
            let name = p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            let label = format!("{name}  —  {}", p.parent().map(|d| d.display().to_string()).unwrap_or_default());
            section.append(Some(&label), Some(&format!("win.open-recent-{i}")));
            let a = gio::SimpleAction::new(&format!("open-recent-{i}"), None);
            let me = self.clone();
            let p = p.clone();
            a.connect_activate(move |_, _| {
                let p = p.clone();
                let me2 = me.clone();
                me.confirm_discard(move || me2.open_file(&p));
            });
            self.w.window.add_action(&a);
        }
        menu.append_section(None, &section);
        btn.set_menu_model(Some(&menu));
    }

    // ---------- file handling ----------

    pub fn open_file(self: &Rc<Self>, path: &std::path::Path) {
        let file_dir = self.state.borrow().settings.file_directory.clone();
        let t = std::time::Instant::now();
        match Document::open(path, Some(&file_dir)) {
            Ok(mut doc) => {
                {
                    let mut st = self.state.borrow_mut();
                    doc.track_modification_date = st.settings.track_modification_date;
                    doc.key_pattern = st.settings.key_pattern;
                    doc.owner = st.settings.owner.clone();
                    let errors = doc.lib.errors.len();
                    st.doc = doc;
                    st.has_file = true;
                    st.current = None;
                    st.pending.clear();
                    st.settings.remember_file(path);
                    st.settings.save();
                    if errors > 0 {
                        drop(st);
                        self.toast(&format!("{errors} part(s) of the file could not be read and are kept unchanged — see Check"), None);
                    }
                }
                list::rebuild_rows(self);
                self.w.stack.set_visible_child_name("main");
                self.w.search.set_text("");
                self.refilter(None);
                self.update_title();
                self.on_selection_changed();
                eprintln!("opened {} in {:?}", path.display(), t.elapsed());
            }
            Err(e) => self.error("Could not open file", &e),
        }
    }

    fn new_library(self: &Rc<Self>) {
        let me = self.clone();
        self.confirm_discard(move || {
            {
                let mut st = me.state.borrow_mut();
                st.doc = Document::new_empty();
                st.doc.track_modification_date = st.settings.track_modification_date;
                st.doc.key_pattern = st.settings.key_pattern;
                st.has_file = true;
                st.current = None;
            }
            list::rebuild_rows(&me);
            me.w.stack.set_visible_child_name("main");
            me.refilter(None);
            me.update_title();
            me.on_selection_changed();
        });
    }

    fn choose_open(self: &Rc<Self>) {
        let me = self.clone();
        self.confirm_discard(move || {
            let dialog = gtk::FileDialog::builder().title("Open library").modal(true).build();
            dialog.set_filters(Some(&bib_filters()));
            let me2 = me.clone();
            dialog.open(Some(&me.w.window), None::<&gio::Cancellable>, move |res| {
                if let Ok(f) = res {
                    if let Some(p) = f.path() {
                        me2.open_file(&p);
                    }
                }
            });
        });
    }

    fn choose_save_as(self: &Rc<Self>) {
        self.flush();
        let dialog = gtk::FileDialog::builder().title("Save library as").modal(true).initial_name("library.bib").build();
        dialog.set_filters(Some(&bib_filters()));
        let me = self.clone();
        dialog.save(Some(&self.w.window), None::<&gio::Cancellable>, move |res| {
            if let Ok(f) = res {
                if let Some(p) = f.path() {
                    let r = me.state.borrow_mut().doc.save_as(&p);
                    match r {
                        Ok(()) => {
                            let mut st = me.state.borrow_mut();
                            st.settings.remember_file(&p);
                            st.settings.save();
                            let dir = st.settings.file_directory.clone();
                            st.doc.refresh_file_dirs(Some(&dir));
                            drop(st);
                            me.update_title();
                            me.toast("Saved", None);
                        }
                        Err(e) => me.error("Could not save", &e.to_string()),
                    }
                }
            }
        });
    }

    pub fn save(self: &Rc<Self>, force: bool) {
        self.flush();
        if self.state.borrow().doc.path.is_none() {
            self.choose_save_as();
            return;
        }
        let r = self.state.borrow_mut().doc.save(force);
        match r {
            Ok(()) => {
                self.update_title();
                self.toast("Saved", None);
            }
            Err(SaveError::ChangedOnDisk) => {
                let dlg = gtk::AlertDialog::builder()
                    .modal(true)
                    .message("The file was changed by another program")
                    .detail("Since you opened it, the .bib file has been modified elsewhere (perhaps by JabRef or a sync service). Saving now will replace those changes. A backup (.bak) of the current file is kept either way.")
                    .buttons(["Cancel", "Overwrite"])
                    .cancel_button(0)
                    .default_button(0)
                    .build();
                let me = self.clone();
                dlg.choose(Some(&self.w.window), None::<&gio::Cancellable>, move |res| {
                    if res == Ok(1) {
                        me.save(true);
                    }
                });
            }
            Err(e) => self.error("Could not save", &e.to_string()),
        }
    }

    /// If there are unsaved changes, ask; then run `then` unless cancelled.
    pub fn confirm_discard(self: &Rc<Self>, then: impl Fn() + 'static) {
        self.flush();
        if self.state.borrow().doc.is_dirty() {
            self.ask_save_then(then);
        } else {
            then();
        }
    }

    fn ask_save_then(self: &Rc<Self>, then: impl Fn() + 'static) {
        let dlg = gtk::AlertDialog::builder()
            .modal(true)
            .message("Save changes?")
            .detail("The library has unsaved changes.")
            .buttons(["Cancel", "Discard", "Save"])
            .cancel_button(0)
            .default_button(2)
            .build();
        let me = self.clone();
        dlg.choose(Some(&self.w.window), None::<&gio::Cancellable>, move |res| match res {
            Ok(1) => then(),
            Ok(2) => {
                me.save(false);
                if !me.state.borrow().doc.is_dirty() {
                    then();
                }
            }
            _ => {}
        });
    }

    pub fn update_title(&self) {
        let st = self.state.borrow();
        let name = st
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into());
        let dirty = st.doc.is_dirty();
        self.w.window.set_title(Some(&format!("{}{} — {}", if dirty { "• " } else { "" }, name, crate::APP_NAME)));
        if let Some(a) = self.w.window.lookup_action("undo") {
            a.downcast::<gio::SimpleAction>().unwrap().set_enabled(st.doc.can_undo());
        }
        if let Some(a) = self.w.window.lookup_action("redo") {
            a.downcast::<gio::SimpleAction>().unwrap().set_enabled(st.doc.can_redo());
        }
        if dirty {
            self.w.save_btn.add_css_class("suggested-action");
        } else {
            self.w.save_btn.remove_css_class("suggested-action");
        }
    }

    // ---------- list / selection ----------

    /// Re-run the search and refill the list; optionally select an entry afterwards.
    pub fn refilter(self: &Rc<Self>, select: Option<usize>) {
        let q = self.w.search.text().to_string();
        let (items, total) = {
            let st = self.state.borrow();
            let hits = st.doc.search(&q);
            (hits, st.doc.lib.entries().count())
        };
        let strings: Vec<String> = items.iter().map(|i| i.to_string()).collect();
        let refs: Vec<&str> = strings.iter().map(|s| s.as_str()).collect();
        let n = self.w.store.n_items();
        self.w.store.splice(0, n, &refs);
        let shown = items.len();
        let name = self
            .state
            .borrow()
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into());
        self.w.count.set_text(&if q.trim().is_empty() {
            format!("{name} · {} entries", fmt_num(total))
        } else {
            format!("{name} · {} of {}", fmt_num(shown), fmt_num(total))
        });
        if let Some(idx) = select {
            self.select_entry(idx);
        } else if self.w.sort_model.n_items() > 0 && !q.trim().is_empty() {
            self.w.selection.select_item(0, true);
        }
    }

    pub fn select_entry(&self, idx: usize) {
        let want = idx.to_string();
        let n = self.w.sort_model.n_items();
        for pos in 0..n {
            if let Some(so) = self.w.sort_model.item(pos).and_downcast::<gtk::StringObject>() {
                if so.string() == want {
                    self.w.selection.select_item(pos, true);
                    self.w.view.scroll_to(pos, None, gtk::ListScrollFlags::NONE, None);
                    return;
                }
            }
        }
    }

    fn selected_indices(&self) -> Vec<usize> {
        let bs = self.w.selection.selection();
        let mut out = Vec::new();
        for i in 0..bs.size() {
            let pos = bs.nth(i as u32);
            if let Some(so) = self.w.sort_model.item(pos).and_downcast::<gtk::StringObject>() {
                if let Ok(idx) = so.string().parse::<usize>() {
                    out.push(idx);
                }
            }
        }
        out
    }

    fn on_selection_changed(self: &Rc<Self>) {
        self.flush();
        let sel = self.selected_indices();
        {
            let mut st = self.state.borrow_mut();
            st.current = if sel.len() == 1 { Some(sel[0]) } else { None };
            st.selected = sel;
        }
        editor::rebuild(self);
        self.refresh_citation();
        if self.needs_resort.replace(false) {
            let me = self.clone();
            glib::idle_add_local_once(move || {
                let keep = me.state.borrow().selected.clone();
                if let Some(s) = me.w.view.sorter() {
                    s.changed(gtk::SorterChange::Different);
                }
                if keep.len() == 1 {
                    me.select_entry(keep[0]);
                }
            });
        }
    }

    // ---------- editing ----------

    /// Commit pending edits from the editor to the document.
    /// Switch between the field editor and the BibTeX source editor.
    pub fn set_source_mode(self: &Rc<Self>, on: bool) {
        if !on {
            // leaving the source view: apply edits, but stay if they don't parse
            if let Err(err) = self.apply_source() {
                self.error("The BibTeX source has an error", &format!("{err}\n\nCorrect it, or press Revert to discard your changes."));
                editor::sync_view_toggle(self, true);
                return;
            }
        } else {
            self.flush();
        }
        self.state.borrow_mut().show_source = on;
        editor::rebuild(self);
    }

    /// Apply edited BibTeX source to the current entry.
    /// Ok(true) if the entry changed; Err with a message if the text is invalid.
    pub fn apply_source(self: &Rc<Self>) -> Result<bool, String> {
        let (idx, text) = {
            let st = self.state.borrow();
            let (Some(idx), Some((buf, orig))) = (st.current, st.source_buf.as_ref()) else { return Ok(false) };
            let text = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
            if text == *orig {
                return Ok(false);
            }
            (idx, text)
        };
        let lib = suci::bib::Library::parse(&text);
        if let Some(err) = lib.errors.first() {
            // point at the first line whose braces don't balance
            let hint = text
                .lines()
                .enumerate()
                .skip(1)
                .find(|(_, l)| {
                    let t = l.trim();
                    let net = t.matches('{').count() as i32 - t.matches("\\{").count() as i32
                        - (t.matches('}').count() as i32 - t.matches("\\}").count() as i32);
                    net != 0 && t != "}"
                })
                .map(|(i, l)| format!("\n\nCheck line {}: {}", i + 1, l.trim()))
                .unwrap_or_default();
            let msg = if err.message.contains("not closed") || err.message.contains("unbalanced") {
                "Unbalanced braces { } — every field value must be closed with } and followed by a comma.".to_string()
            } else {
                err.message.clone()
            };
            return Err(format!("{msg}{hint}"));
        }
        let entries: Vec<Entry> = lib.entries().cloned().collect();
        let junk: String = lib
            .items
            .iter()
            .filter_map(|it| if let suci::bib::Item::Junk(j) = it { Some(j.trim()) } else { None })
            .collect();
        let mut e = match entries.len() {
            0 => return Err("No entry found. The text must look like @Type{key, field = {value}, …}".into()),
            1 => entries.into_iter().next().unwrap(),
            n => return Err(format!("Found {n} entries; only one entry can be edited here. Use “Paste BibTeX” to add new ones.")),
        };
        if !junk.is_empty() {
            return Err(format!("Text outside the entry: “{}”", junk.chars().take(60).collect::<String>()));
        }
        if e.key.trim().is_empty() {
            return Err("The entry has no citation key.".into());
        }
        e.raw = None;
        let changed = self.state.borrow_mut().doc.update_entry(idx, e);
        if let Some((_, orig)) = self.state.borrow_mut().source_buf.as_mut() {
            *orig = text;
        }
        if changed {
            self.needs_resort.set(true);
            list::refresh_row(self, idx);
            self.update_title();
            self.refresh_citation();
        }
        Ok(changed)
    }

    pub fn flush(self: &Rc<Self>) {
        if self.state.try_borrow().is_ok_and(|st| st.source_buf.is_some()) {
            match self.apply_source() {
                Ok(_) => {}
                Err(err) => {
                    // keep the user's text: put it on the clipboard
                    let text = {
                        let st = self.state.borrow();
                        st.source_buf.as_ref().map(|(b, _)| b.text(&b.start_iter(), &b.end_iter(), false).to_string())
                    };
                    if let Some(t) = text {
                        self.w.window.clipboard().set_text(&t);
                    }
                    self.toast(&format!("BibTeX edit not applied ({err}). Your text is on the clipboard."), None);
                }
            }
        }
        let changed = {
            let Ok(mut st) = self.state.try_borrow_mut() else { return };
            if st.pending.is_empty() && st.pending_key.is_none() && st.pending_type.is_none() {
                return;
            }
            let Some(idx) = st.current else {
                st.pending.clear();
                return;
            };
            let Some(mut e) = st.doc.entry(idx).cloned() else { return };
            let pending = std::mem::take(&mut st.pending);
            for (f, v) in pending {
                e.set(&f, &v);
            }
            if let Some(k) = st.pending_key.take() {
                let k = k.trim().to_string();
                if !k.is_empty() {
                    e.set_key(&k);
                }
            }
            if let Some(t) = st.pending_type.take() {
                e.set_type(&t);
            }
            // eprint: tidy pasted resolver links, default the type to "ark"
            if let Some(ep) = e.get_nonempty("eprint") {
                let ty = e.get("eprinttype").unwrap_or_default();
                let norm = suci::files::normalize_eprint(&ep, &ty);
                if norm != ep {
                    e.set("eprint", &norm);
                }
                if ty.trim().is_empty() {
                    e.set("eprinttype", suci::files::DEFAULT_EPRINTTYPE);
                }
            }
            // provisional keys get replaced once there is enough data
            if e.key.starts_with("new-") || e.key.is_empty() {
                let k = st.doc.generate_key(&e, Some(idx));
                if !k.starts_with('-') && !k.ends_with('-') {
                    e.set_key(&k);
                }
            }
            if st.doc.update_entry(idx, e) {
                Some(idx)
            } else {
                None
            }
        };
        if let Some(idx) = changed {
            self.needs_resort.set(true);
            list::refresh_row(self, idx);
            self.update_title();
            self.refresh_citation();
            editor::refresh_header(self);
        }
    }

    pub fn after_doc_change(self: &Rc<Self>, select: Option<usize>) {
        list::rebuild_rows(self);
        let keep = select.or_else(|| self.state.borrow().current);
        self.refilter(keep);
        self.update_title();
        editor::rebuild(self);
        self.refresh_citation();
    }

    fn undo(self: &Rc<Self>) {
        self.flush();
        let idxs = self.state.borrow_mut().doc.undo();
        if !idxs.is_empty() {
            let sel = idxs.iter().copied().find(|i| self.state.borrow().doc.entry(*i).is_some());
            self.after_doc_change(sel);
        }
    }

    fn redo(self: &Rc<Self>) {
        self.flush();
        let idxs = self.state.borrow_mut().doc.redo();
        if !idxs.is_empty() {
            let sel = idxs.iter().copied().find(|i| self.state.borrow().doc.entry(*i).is_some());
            self.after_doc_change(sel);
        }
    }

    pub fn new_entry(self: &Rc<Self>, ty: &str) {
        self.flush();
        if !self.state.borrow().has_file {
            self.new_library();
        }
        let idx = {
            let mut st = self.state.borrow_mut();
            let keys = st.doc.keys();
            let key = suci::keygen::unique_key("new-entry", |k| keys.contains(k));
            let e = Entry::new(ty, &key);
            st.doc.add_entry(e)
        };
        self.w.search.set_text("");
        self.after_doc_change(Some(idx));
        editor::focus_first_field(self);
    }

    pub fn add_fetched(self: &Rc<Self>, e: Entry) -> usize {
        self.flush();
        let idx = self.state.borrow_mut().doc.add_entry(e);
        self.w.search.set_text("");
        self.after_doc_change(Some(idx));
        let key = self.state.borrow().doc.entry(idx).map(|e| e.key.clone()).unwrap_or_default();
        self.toast(&format!("Added {key}"), None);
        idx
    }

    fn delete_selected(self: &Rc<Self>) {
        self.flush();
        let sel = self.selected_indices();
        if sel.is_empty() {
            return;
        }
        let n = self.state.borrow_mut().doc.delete(&sel);
        self.after_doc_change(None);
        let me = self.clone();
        self.toast(
            &format!("Deleted {n} {}", if n == 1 { "entry" } else { "entries" }),
            Some(Box::new(move || me.undo())),
        );
    }

    fn duplicate_selected(self: &Rc<Self>) {
        self.flush();
        let Some(idx) = self.state.borrow().current else { return };
        let Some(mut e) = self.state.borrow().doc.entry(idx).cloned() else { return };
        e.key = String::new();
        e.remove("file");
        let new = self.state.borrow_mut().doc.add_entry(e);
        self.after_doc_change(Some(new));
    }

    fn convert_selected(self: &Rc<Self>, to_unicode: bool) {
        self.flush();
        let sel = self.selected_indices();
        let updates: Vec<(usize, Entry)> = {
            let st = self.state.borrow();
            sel.iter()
                .filter_map(|&i| st.doc.entry(i).cloned().map(|e| (i, convert_entry(e, to_unicode))))
                .collect()
        };
        let n = self.state.borrow_mut().doc.update_entries(updates);
        self.after_doc_change(None);
        self.toast(&format!("Converted {n} {}", if n == 1 { "entry" } else { "entries" }), None);
    }

    fn convert_all_to_unicode(self: &Rc<Self>) {
        self.flush();
        let updates: Vec<(usize, Entry)> = {
            let st = self.state.borrow();
            st.doc
                .entry_indices()
                .into_iter()
                .filter_map(|i| {
                    let e = st.doc.entry(i)?.clone();
                    let c = convert_entry(e.clone(), true);
                    (c.fields != e.fields).then_some((i, c))
                })
                .collect()
        };
        let n = self.state.borrow_mut().doc.update_entries(updates);
        self.after_doc_change(None);
        let me = self.clone();
        self.toast(&format!("Converted LaTeX accents to Unicode in {n} entries"), Some(Box::new(move || me.undo())));
    }

    fn open_first_pdf(self: &Rc<Self>) {
        let path = {
            let st = self.state.borrow();
            st.current
                .and_then(|i| st.doc.entry(i))
                .and_then(|e| e.get_nonempty("file"))
                .and_then(|f| suci::files::parse_file_field(&f).into_iter().find_map(|lf| st.doc.file_dirs.resolve(&lf.path)))
        };
        match path {
            Some(p) => self.launch_file(&p),
            None => self.toast("No linked file found for this entry", None),
        }
    }

    pub fn launch_file(&self, p: &std::path::Path) {
        let f = gio::File::for_path(p);
        gtk::FileLauncher::new(Some(&f)).launch(Some(&self.w.window), None::<&gio::Cancellable>, |_| {});
    }

    pub fn launch_uri(&self, uri: &str) {
        gtk::UriLauncher::new(uri).launch(Some(&self.w.window), None::<&gio::Cancellable>, |_| {});
    }

    // ---------- citations & clipboard ----------

    pub fn refresh_citation(self: &Rc<Self>) {
        editor::refresh_citation(self);
    }

    fn render_selected(&self) -> Option<suci::cite::Rendered> {
        let st = self.state.borrow();
        let entries: Vec<Entry> = st.selected.iter().filter_map(|&i| st.doc.entry(i).cloned()).collect();
        if entries.is_empty() {
            return None;
        }
        let refs: Vec<&Entry> = entries.iter().collect();
        let style = st.settings.citation_style.clone();
        let lookup = |k: &str| st.doc.lookup_entry(k);
        st.styles.render(&style, &refs, &lookup).ok()
    }

    pub fn copy_references(self: &Rc<Self>) {
        self.flush();
        let Some(r) = self.render_selected() else { return };
        let html = r.bib_html.iter().map(|h| format!("<p>{h}</p>")).collect::<Vec<_>>().join("\n");
        let text = r.bib_text.join("\n\n");
        self.set_clipboard_rich(&html, &text);
        let n = r.bib_text.len();
        self.toast(&format!("Copied {n} formatted {}", if n == 1 { "reference" } else { "references" }), None);
    }

    pub fn copy_citations(self: &Rc<Self>) {
        self.flush();
        let Some(r) = self.render_selected() else { return };
        let text = r.citations.join(if r.note_style { "\n\n" } else { "; " });
        self.w.window.clipboard().set_text(&text);
        self.toast(if r.note_style { "Copied note citation" } else { "Copied in-text citation" }, None);
    }

    pub fn copy_keys(self: &Rc<Self>, as_latex: bool) {
        self.flush();
        let keys: Vec<String> = {
            let st = self.state.borrow();
            st.selected.iter().filter_map(|&i| st.doc.entry(i).map(|e| e.key.clone())).collect()
        };
        if keys.is_empty() {
            return;
        }
        let text = if as_latex { format!("\\cite{{{}}}", keys.join(",")) } else { keys.join(",") };
        self.w.window.clipboard().set_text(&text);
        self.toast(&format!("Copied {text}"), None);
    }

    fn copy_bibtex(self: &Rc<Self>) {
        self.flush();
        let text: String = {
            let st = self.state.borrow();
            st.selected.iter().filter_map(|&i| st.doc.entry(i).map(|e| e.to_bibtex())).collect::<Vec<_>>().join("\n\n")
        };
        if !text.is_empty() {
            self.w.window.clipboard().set_text(&text);
            self.toast("Copied BibTeX", None);
        }
    }

    fn paste_bibtex(self: &Rc<Self>) {
        let me = self.clone();
        self.w.window.clipboard().read_text_async(None::<&gio::Cancellable>, move |res| {
            let text = match res {
                Ok(Some(t)) => t.to_string(),
                _ => {
                    me.toast("The clipboard does not contain text", None);
                    return;
                }
            };
            // a DOI or ISBN on the clipboard goes to the fetcher
            if suci::fetch::detect(&text).is_some() && !text.contains('@') {
                dialogs::start_fetch(&me, &text, None);
                return;
            }
            let lib = suci::bib::Library::parse(&text);
            let entries: Vec<Entry> = lib.entries().cloned().collect();
            if entries.is_empty() {
                me.toast("No BibTeX entries found on the clipboard", None);
                return;
            }
            me.flush();
            let mut last = None;
            let n = entries.len();
            for mut e in entries {
                e.raw = None;
                let exists = me.state.borrow().doc.find_key(&e.key).is_some();
                if exists || e.key.is_empty() {
                    e.key = String::new();
                }
                last = Some(me.state.borrow_mut().doc.add_entry(e));
            }
            me.after_doc_change(last);
            me.toast(&format!("Pasted {n} {}", if n == 1 { "entry" } else { "entries" }), None);
        });
    }

    pub fn set_clipboard_rich(&self, html: &str, text: &str) {
        let html_doc = format!("<meta http-equiv=\"content-type\" content=\"text/html; charset=utf-8\">{html}");
        let providers = [
            gdk::ContentProvider::for_bytes("text/html", &glib::Bytes::from_owned(html_doc.into_bytes())),
            gdk::ContentProvider::for_value(&text.to_value()),
        ];
        let union = gdk::ContentProvider::new_union(&providers);
        let _ = self.w.window.clipboard().set_content(Some(&union));
    }

    // ---------- feedback ----------

    pub fn toast(&self, msg: &str, action: Option<Box<dyn Fn()>>) {
        self.w.toast_label.set_text(msg);
        self.w.toast_button.set_visible(action.is_some());
        *self.toast_action.borrow_mut() = action;
        self.w.toast_revealer.set_reveal_child(true);
        let serial = self.toast_serial.get() + 1;
        self.toast_serial.set(serial);
        let rev = self.w.toast_revealer.clone();
        let cell = self.toast_serial.clone();
        glib::timeout_add_seconds_local_once(6, move || {
            // only hide if no newer toast replaced this one
            if cell.get() == serial {
                rev.set_reveal_child(false);
            }
        });
    }

    pub fn error(&self, title: &str, detail: &str) {
        gtk::AlertDialog::builder().modal(true).message(title).detail(detail).build().show(Some(&self.w.window));
    }
}

pub fn convert_entry(mut e: Entry, to_unicode: bool) -> Entry {
    for f in e.fields.iter_mut() {
        let name = f.name.to_lowercase();
        if matches!(name.as_str(), "file" | "url" | "doi" | "eprint" | "isbn" | "issn") {
            continue;
        }
        for p in f.parts.iter_mut() {
            if let suci::bib::Part::Lit(t) = p {
                *t = if to_unicode { suci::latex::accents_to_unicode(t) } else { suci::latex::unicode_to_latex(t) };
            }
        }
    }
    e.raw = None;
    e
}

fn bib_filters() -> gio::ListStore {
    let f = gtk::FileFilter::new();
    f.set_name(Some("BibTeX / biblatex (*.bib)"));
    f.add_pattern("*.bib");
    f.add_pattern("*.BIB");
    let all = gtk::FileFilter::new();
    all.set_name(Some("All files"));
    all.add_pattern("*");
    let store = gio::ListStore::new::<gtk::FileFilter>();
    store.append(&f);
    store.append(&all);
    store
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let file = gio::Menu::new();
    file.append(Some("Save As…"), Some("win.save-as"));
    file.append(Some("New Library"), Some("win.new-library"));
    menu.append_section(None, &file);

    let edit = gio::Menu::new();
    edit.append(Some("Paste BibTeX from Clipboard"), Some("win.paste-bibtex"));
    edit.append(Some("Duplicate Entry"), Some("win.duplicate"));
    edit.append(Some("Copy BibTeX of Selection"), Some("win.copy-bibtex"));
    edit.append(Some("Copy \\cite{…} of Selection"), Some("win.copy-latex"));
    menu.append_section(None, &edit);

    let conv = gio::Menu::new();
    conv.append(Some("Selected Entries: LaTeX Accents → Unicode"), Some("win.to-unicode-sel"));
    conv.append(Some("Selected Entries: Unicode → LaTeX Accents"), Some("win.to-latex-sel"));
    conv.append(Some("Whole Library: LaTeX Accents → Unicode"), Some("win.to-unicode-all"));
    menu.append_submenu(Some("Diacritics"), &conv);

    let sort = gio::Menu::new();
    sort.append(Some("Alphabetical (ā after a, ś after s)"), Some("win.sort-order::latin"));
    sort.append(Some("Sanskrit varṇamālā (a ā i ī … ś ṣ s h)"), Some("win.sort-order::sanskrit"));
    menu.append_submenu(Some("Sort Order"), &sort);

    let misc = gio::Menu::new();
    misc.append(Some("Check Library…"), Some("win.check"));
    misc.append(Some("Preferences"), Some("win.prefs"));
    misc.append(Some("Keyboard Shortcuts"), Some("win.shortcuts"));
    misc.append(Some("About Sūcī"), Some("win.about"));
    menu.append_section(None, &misc);
    menu
}

pub fn fmt_num(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
