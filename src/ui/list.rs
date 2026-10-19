//! The entry list (a sortable ColumnView over item indices).

use super::App;
use gtk::prelude::*;
use gtk::{glib, pango};
use std::cmp::Ordering;
use std::rc::Rc;
use suci::bib::Entry;
use suci::collate::{latin_key, sanskrit_key, SortMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Col {
    Key,
    Author,
    Year,
    Title,
    Type,
    File,
}

#[derive(Debug, Clone, Default)]
pub struct Row {
    pub key: String,
    pub author: String,
    pub year: Option<i32>,
    pub year_text: String,
    pub title: String,
    pub ty: String,
    pub has_file: bool,
    author_latin: String,
    author_skt: Vec<u32>,
    title_latin: String,
    title_skt: Vec<u32>,
    key_lower: String,
}

pub fn make_row(e: &Entry, sanskrit: bool) -> Row {
    let persons = e.get_nonempty("author").or_else(|| e.get_nonempty("editor")).unwrap_or_default();
    let author = suci::names::short_display(&persons);
    // sort by full surnames then given names
    let sort_names: String = suci::names::parse_names(&persons)
        .iter()
        .map(|n| suci::latex::to_plain(&format!("{} {} {}", n.last, n.von, n.first)))
        .collect::<Vec<_>>()
        .join(" ");
    let mut title = suci::latex::to_plain(&e.get("title").unwrap_or_default());
    if let Some(sub) = e.get_nonempty("subtitle") {
        title.push_str(": ");
        title.push_str(&suci::latex::to_plain(&sub));
    }
    let year = suci::search::entry_year(e);
    let year_text = e
        .get_nonempty("date")
        .or_else(|| e.get_nonempty("year"))
        .map(|d| {
            let d = suci::latex::to_plain(&d);
            // show 2001 for 2001-05-03, keep ranges like 1990/1995
            if d.len() >= 7 && d.as_bytes()[4] == b'-' && !d.contains('/') {
                d[..4].to_string()
            } else {
                d
            }
        })
        .unwrap_or_default();
    let title_sort = title.trim_start_matches(|c: char| !c.is_alphanumeric()).to_string();
    Row {
        key: e.key.clone(),
        author_latin: latin_key(&sort_names),
        author_skt: if sanskrit { sanskrit_key(&sort_names) } else { Vec::new() },
        title_latin: latin_key(&title_sort),
        title_skt: if sanskrit { sanskrit_key(&title_sort) } else { Vec::new() },
        key_lower: e.key.to_lowercase(),
        author,
        year,
        year_text,
        title,
        ty: super::editor::type_display(&e.ty_lower()),
        has_file: e.get_nonempty("file").is_some(),
    }
}

pub fn rebuild_rows(app: &Rc<App>) {
    let mut st = app.state.borrow_mut();
    let sanskrit = st.settings.sort_mode == SortMode::Sanskrit;
    let items = &st.doc.lib.items;
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).clamp(1, 8);
    let chunk = items.len().div_ceil(threads).max(1);
    let rows: Vec<Option<Row>> = std::thread::scope(|s| {
        let handles: Vec<_> = items
            .chunks(chunk)
            .map(|part| {
                s.spawn(move || {
                    part.iter()
                        .map(|it| if let suci::bib::Item::Entry(e) = it { Some(make_row(e, sanskrit)) } else { None })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        handles.into_iter().flat_map(|h| h.join().unwrap()).collect()
    });
    st.rows = rows;
}

fn idx_of(o: &glib::Object) -> usize {
    o.downcast_ref::<gtk::StringObject>().and_then(|s| s.string().parse().ok()).unwrap_or(usize::MAX)
}

fn compare(app: &App, col: Col, a: usize, b: usize) -> Ordering {
    let Ok(st) = app.state.try_borrow() else { return Ordering::Equal };
    let (Some(Some(ra)), Some(Some(rb))) = (st.rows.get(a), st.rows.get(b)) else { return Ordering::Equal };
    let skt = st.settings.sort_mode == SortMode::Sanskrit;
    let by_author = |ra: &Row, rb: &Row| {
        // entries without author or editor go last
        let (ea, eb) = (ra.author.is_empty(), rb.author.is_empty());
        if ea != eb {
            return ea.cmp(&eb);
        }
        if skt {
            ra.author_skt.cmp(&rb.author_skt)
        } else {
            ra.author_latin.cmp(&rb.author_latin)
        }
    };
    let by_title = |ra: &Row, rb: &Row| if skt { ra.title_skt.cmp(&rb.title_skt) } else { ra.title_latin.cmp(&rb.title_latin) };
    let by_year = |ra: &Row, rb: &Row| ra.year.unwrap_or(i32::MAX).cmp(&rb.year.unwrap_or(i32::MAX));
    match col {
        Col::Key => ra.key_lower.cmp(&rb.key_lower),
        Col::Author => by_author(ra, rb).then_with(|| by_year(ra, rb)).then_with(|| by_title(ra, rb)),
        Col::Year => by_year(ra, rb).then_with(|| by_author(ra, rb)),
        Col::Title => by_title(ra, rb),
        Col::Type => ra.ty.cmp(&rb.ty).then_with(|| by_author(ra, rb)),
        Col::File => rb.has_file.cmp(&ra.has_file),
    }
}

fn set_label(app: &App, label: &gtk::Label, col: Col, idx: usize) {
    let st = app.state.borrow();
    let Some(Some(r)) = st.rows.get(idx) else {
        label.set_text("");
        return;
    };
    match col {
        Col::Key => label.set_text(&r.key),
        Col::Author => label.set_text(&r.author),
        Col::Year => label.set_text(&r.year_text),
        Col::Title => {
            label.set_text(&r.title);
            label.set_tooltip_text(Some(&r.title));
        }
        Col::Type => label.set_text(&r.ty),
        Col::File => label.set_text(if r.has_file { "PDF" } else { "" }),
    }
}

fn factory(app: &Rc<App>, col: Col) -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(move |_, item| {
        let li = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.set_ellipsize(pango::EllipsizeMode::End);
        if col == Col::Key {
            label.add_css_class("monospace");
        }
        if col == Col::File {
            label.add_css_class("dim-label");
            label.add_css_class("caption");
        }
        li.set_child(Some(&label));
    });
    let me = app.clone();
    f.connect_bind(move |_, item| {
        let li = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = li.child().and_downcast::<gtk::Label>().unwrap();
        let idx = li.item().map(|o| idx_of(&o)).unwrap_or(usize::MAX);
        set_label(&me, &label, col, idx);
        me.bound.borrow_mut().entry(idx).or_default().push((col, label.downgrade()));
    });
    let me = app.clone();
    f.connect_unbind(move |_, item| {
        let li = item.downcast_ref::<gtk::ListItem>().unwrap();
        let idx = li.item().map(|o| idx_of(&o)).unwrap_or(usize::MAX);
        if let Some(label) = li.child().and_downcast::<gtk::Label>() {
            if let Some(v) = me.bound.borrow_mut().get_mut(&idx) {
                v.retain(|(c, w)| !(*c == col && w.upgrade().is_some_and(|l| l == label)));
            }
        }
    });
    f
}

pub fn setup_columns(app: &Rc<App>) {
    let cols = [
        (Col::Author, "Author / Editor", 165, false),
        (Col::Year, "Year", 58, false),
        (Col::Title, "Title", 300, true),
        (Col::Key, "Key", 105, false),
        (Col::Type, "Type", 88, false),
        (Col::File, "File", 40, false),
    ];
    let mut author_col = None;
    for (col, title, width, expand) in cols {
        let c = gtk::ColumnViewColumn::new(Some(title), Some(factory(app, col)));
        c.set_resizable(true);
        c.set_expand(expand);
        c.set_fixed_width(width);
        let me = app.clone();
        let sorter = gtk::CustomSorter::new(move |a, b| compare(&me, col, idx_of(a), idx_of(b)).into());
        c.set_sorter(Some(&sorter));
        app.w.view.append_column(&c);
        if col == Col::Author {
            author_col = Some(c);
        }
    }
    app.w.sort_model.set_sorter(app.w.view.sorter().as_ref());
    if let Some(c) = author_col {
        app.w.view.sort_by_column(Some(&c), gtk::SortType::Ascending);
    }
}

/// Re-apply the current sort (after changing the sort mode).
pub fn resort(app: &Rc<App>) {
    rebuild_rows(app);
    if let Some(s) = app.w.view.sorter() {
        s.changed(gtk::SorterChange::Different);
    }
}

/// Update the visible labels of one row after an edit, without touching the
/// model (which would reset the selection).
pub fn refresh_row(app: &Rc<App>, idx: usize) {
    {
        let mut st = app.state.borrow_mut();
        let sanskrit = st.settings.sort_mode == SortMode::Sanskrit;
        let row = st.doc.entry(idx).map(|e| make_row(e, sanskrit));
        if idx < st.rows.len() {
            st.rows[idx] = row;
        }
    }
    let labels: Vec<(Col, gtk::Label)> = app
        .bound
        .borrow()
        .get(&idx)
        .map(|v| v.iter().filter_map(|(c, w)| w.upgrade().map(|l| (*c, l))).collect())
        .unwrap_or_default();
    for (c, l) in labels {
        set_label(app, &l, c, idx);
    }
}
