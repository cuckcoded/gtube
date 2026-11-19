//! A scrollable, infinitely-loading grid or list of feed items.

use super::cards::{self, Layout};
use crate::api::{self, models::*, parse};
use crate::runtime;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum Source {
    Browse { id: String, params: Option<String> },
    Search { query: String, params: Option<String> },
    Empty,
}

pub fn opts() -> parse::Opts {
    parse::Opts { hide_shorts: crate::config::settings().hide_shorts }
}

pub async fn fetch(src: Source, token: Option<String>) -> anyhow::Result<Feed> {
    let o = opts();
    runtime::run(async move {
        let c = api::client();
        let v = match (&src, token) {
            (Source::Search { .. }, Some(t)) => c.search_continuation(&t).await?,
            (_, Some(t)) => c.browse_continuation(&t).await?,
            (Source::Browse { id, params }, None) => c.browse(id, params.as_deref()).await?,
            (Source::Search { query, params }, None) => c.search(query, params.as_deref()).await?,
            (Source::Empty, None) => return Ok(Feed::default()),
        };
        Ok(parse::parse_feed(&v, o))
    })
    .await
}

/// A centered icon + title + description block for empty and error states.
pub fn status_box(icon: &str, title: &str, desc: &str) -> (gtk::Box, gtk::Label, gtk::Label, gtk::Button) {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
    b.set_valign(gtk::Align::Center);
    b.set_halign(gtk::Align::Center);
    b.set_margin_top(64);
    b.set_margin_bottom(64);
    b.set_margin_start(24);
    b.set_margin_end(24);
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(96);
    img.add_css_class("dim-label");
    let t = gtk::Label::new(Some(title));
    t.add_css_class("title-1");
    t.set_wrap(true);
    t.set_justify(gtk::Justification::Center);
    let d = gtk::Label::new(Some(desc));
    d.set_wrap(true);
    d.set_justify(gtk::Justification::Center);
    d.set_max_width_chars(60);
    d.add_css_class("dim-label");
    let btn = gtk::Button::new();
    btn.add_css_class("pill");
    btn.add_css_class("suggested-action");
    btn.set_halign(gtk::Align::Center);
    btn.set_visible(false);
    b.append(&img);
    b.append(&t);
    b.append(&d);
    b.append(&btn);
    (b, t, d, btn)
}

type ActivateFn = Box<dyn Fn(usize, &[Item])>;
type RowFn = Box<dyn Fn(&gtk::ListBoxRow, &gtk::Widget) -> gtk::Widget>;

pub struct FeedView {
    pub widget: adw::BreakpointBin,
    scroller: gtk::ScrolledWindow,
    pub header: gtk::Box,
    chips: gtk::Box,
    chips_scroller: gtk::ScrolledWindow,
    grid: gtk::FlowBox,
    list: gtk::ListBox,
    spinner: adw::Spinner,
    status: gtk::Box,
    status_title: gtk::Label,
    status_desc: gtk::Label,
    status_btn: gtk::Button,
    layout: Layout,
    source: RefCell<Source>,
    items: RefCell<Vec<Item>>,
    continuation: RefCell<Option<String>>,
    /// The selected chip's token (None: the unfiltered feed).
    chip_token: RefCell<Option<String>>,
    loading: Cell<bool>,
    generation: Cell<u64>,
    loaded: Cell<bool>,
    empty_loads: Cell<u32>,
    pub needs_login: Cell<bool>,
    empty: RefCell<(String, String, String)>,
    on_activate: RefCell<Option<ActivateFn>>,
    /// Builds a list row's content around its card.
    row_fn: RefCell<Option<RowFn>>,
}

impl FeedView {
    pub fn new(source: Source, layout: Layout) -> Rc<Self> {
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let margin = if layout == Layout::Grid { 24 } else { 16 };
        content.set_margin_top(12);
        content.set_margin_bottom(24);
        content.set_margin_start(margin);
        content.set_margin_end(margin);

        let header = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let chips = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let chips_scroller = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .hscrollbar_policy(gtk::PolicyType::External)
            .child(&chips)
            .visible(false)
            .build();

        let grid = gtk::FlowBox::builder()
            .homogeneous(true)
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(1)
            .max_children_per_line(4)
            .column_spacing(16)
            .row_spacing(28)
            .valign(gtk::Align::Start)
            .activate_on_single_click(true)
            .build();
        let list = gtk::ListBox::builder().selection_mode(gtk::SelectionMode::None).build();
        list.add_css_class("feed-list");
        grid.set_visible(layout == Layout::Grid);
        list.set_visible(layout == Layout::List);

        let (status, status_title, status_desc, status_btn) = status_box("view-grid-symbolic", "", "");
        status.set_visible(false);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_margin_top(24);
        spinner.set_margin_bottom(24);
        spinner.set_visible(false);

        content.append(&header);
        content.append(&chips_scroller);
        content.append(&status);
        content.append(&grid);
        content.append(&list);
        content.append(&spinner);

        // Lists (playlists, watch later…) stay readable on wide screens.
        let body: gtk::Widget = if layout == Layout::List {
            adw::Clamp::builder().maximum_size(1400).tightening_threshold(1100).child(&content).build().upcast()
        } else {
            content.upcast()
        };
        let scroller = super::widgets::scrolled(&body);
        scroller.set_vexpand(true);
        scroller.set_hexpand(true);
        let widget = adw::BreakpointBin::builder().width_request(200).height_request(200).child(&scroller).build();
        // Bigger cards rather than ever more of them as the window widens:
        // at most this many per row from these widths on.
        // (Four below the first width; the ranges don't overlap, as only
        // one breakpoint applies at a time.)
        for (min, max, columns) in [(1500, Some(1900), 5u32), (1900, Some(2500), 6), (2500, Some(3100), 7), (3100, None, 8)] {
            let cond = match max {
                Some(max) => format!("min-width: {min}sp and max-width: {max}sp"),
                None => format!("min-width: {min}sp"),
            };
            let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse(&cond).unwrap());
            bp.add_setter(&grid, "max-children-per-line", Some(&columns.to_value()));
            widget.add_breakpoint(bp);
        }
        // Any of them: lists get bigger thumbnails.
        let sc = scroller.clone();
        widget.connect_current_breakpoint_notify(move |b| {
            if b.current_breakpoint().is_some() {
                sc.add_css_class("wide");
            } else {
                sc.remove_css_class("wide");
            }
        });

        let this = Rc::new(Self {
            widget,
            scroller,
            header,
            chips,
            chips_scroller,
            grid,
            list,
            spinner,
            status,
            status_title,
            status_desc,
            status_btn,
            layout,
            source: RefCell::new(source),
            items: RefCell::new(Vec::new()),
            continuation: RefCell::new(None),
            chip_token: RefCell::new(None),
            loading: Cell::new(false),
            generation: Cell::new(0),
            loaded: Cell::new(false),
            empty_loads: Cell::new(0),
            needs_login: Cell::new(false),
            empty: RefCell::new(("view-grid-symbolic".into(), "Nothing here".into(), String::new())),
            on_activate: RefCell::new(None),
            row_fn: RefCell::new(None),
        });

        let weak = Rc::downgrade(&this);
        this.grid.connect_child_activated(move |_, child| {
            if let Some(t) = weak.upgrade() {
                t.activate_index(child.index() as usize);
            }
        });
        let weak = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            if let Some(t) = weak.upgrade() {
                t.activate_index(row.index() as usize);
            }
        });
        let weak = Rc::downgrade(&this);
        this.scroller.vadjustment().connect_value_changed(move |adj| {
            if let Some(t) = weak.upgrade()
                && adj.value() + adj.page_size() > adj.upper() - 1200.0
            {
                t.load_more();
            }
        });
        this.status_btn.connect_clicked(|_| super::login::show());
        this
    }

    pub fn set_empty_state(&self, icon: &str, title: &str, desc: &str) {
        self.empty.replace((icon.into(), title.into(), desc.into()));
    }

    pub fn connect_activate(&self, f: impl Fn(usize, &[Item]) + 'static) {
        self.on_activate.replace(Some(Box::new(f)));
    }

    pub fn connect_row(&self, f: impl Fn(&gtk::ListBoxRow, &gtk::Widget) -> gtk::Widget + 'static) {
        self.row_fn.replace(Some(Box::new(f)));
    }

    /// Removes an item (list layout).
    pub fn remove(&self, i: usize) {
        if let Some(row) = self.list.row_at_index(i as i32) {
            self.list.remove(&row);
            self.items.borrow_mut().remove(i);
        }
    }

    /// Moves an item (list layout).
    pub fn move_item(&self, from: usize, to: usize) {
        let n = self.items.borrow().len();
        let Some(row) = self.list.row_at_index(from as i32).filter(|_| from < n && to < n) else { return };
        self.list.remove(&row);
        self.list.insert(&row, to as i32);
        let mut items = self.items.borrow_mut();
        let it = items.remove(from);
        items.insert(to, it);
    }

    pub fn items(&self) -> Vec<Item> {
        self.items.borrow().clone()
    }

    /// The token for what isn't loaded yet.
    pub fn continuation(&self) -> Option<String> {
        self.continuation.borrow().clone()
    }

    fn activate_index(&self, i: usize) {
        let items = self.items.borrow().clone();
        if i >= items.len() {
            return;
        }
        if let Some(f) = self.on_activate.borrow().as_ref() {
            f(i, &items);
        } else {
            cards::activate(&items[i]);
        }
    }

    pub fn set_source(self: &Rc<Self>, source: Source) {
        self.source.replace(source);
        self.reload();
    }

    /// Loads the first page if this view has never been loaded.
    pub fn ensure_loaded(self: &Rc<Self>) {
        if !self.loaded.get() {
            self.reload();
        }
    }

    /// Forgets the loaded content; the next `ensure_loaded` fetches again.
    pub fn invalidate(&self) {
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
        self.spinner.set_visible(false);
        self.loaded.set(false);
        self.clear();
        self.status.set_visible(false);
    }

    fn clear(&self) {
        self.items.borrow_mut().clear();
        self.continuation.replace(None);
        while let Some(c) = self.grid.first_child() {
            self.grid.remove(&c);
        }
        self.list.remove_all();
    }

    fn show_status(&self, icon: &str, title: &str, desc: &str, login_button: bool) {
        if let Some(img) = self.status.first_child().and_downcast::<gtk::Image>() {
            img.set_icon_name(Some(icon));
        }
        self.status_title.set_text(title);
        self.status_desc.set_text(desc);
        self.status_desc.set_visible(!desc.is_empty());
        self.status_btn.set_label("Sign in");
        self.status_btn.set_visible(login_button);
        self.status.set_visible(true);
    }

    pub fn reload(self: &Rc<Self>) {
        self.loaded.set(true);
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
        self.clear();
        self.status.set_visible(false);
        self.set_chips(&[]);
        if self.needs_login.get() && !api::client().is_logged_in() {
            self.show_status(
                "avatar-default-symbolic",
                "Sign in to see this",
                "This page shows your own YouTube data. Sign in with your Google account to load it.",
                true,
            );
            return;
        }
        self.load(None, true);
    }

    /// Loads the feed again from the top, keeping the selected chip.
    pub fn refresh(self: &Rc<Self>) {
        let token = self.chip_token.borrow().clone();
        let Some(token) = token else {
            self.reload();
            return;
        };
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
        self.clear();
        self.status.set_visible(false);
        self.load(Some(token), false);
    }

    /// Shows an already fetched first page (e.g. a channel tab).
    pub fn set_feed(self: &Rc<Self>, source: Source, feed: Feed) {
        self.loaded.set(true);
        self.generation.set(self.generation.get() + 1);
        self.loading.set(false);
        self.source.replace(source);
        self.clear();
        self.status.set_visible(false);
        self.apply(feed, true);
    }

    fn load_more(self: &Rc<Self>) {
        if self.loading.get() || self.continuation.borrow().is_none() {
            return;
        }
        let token = self.continuation.borrow().clone();
        self.load(token, false);
    }

    fn load(self: &Rc<Self>, token: Option<String>, first: bool) {
        self.loading.set(true);
        self.spinner.set_visible(true);
        let generation = self.generation.get();
        let source = self.source.borrow().clone();
        let weak = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = fetch(source, token).await;
            let Some(this) = weak.upgrade() else { return };
            if this.generation.get() != generation {
                return;
            }
            this.loading.set(false);
            this.spinner.set_visible(false);
            match res {
                Ok(feed) => this.apply(feed, first),
                Err(e) => {
                    if first {
                        this.show_status("dialog-error-symbolic", "Could not load this page", &e.to_string(), false);
                    } else {
                        super::toast(&format!("Could not load more: {e}"));
                    }
                }
            }
        });
    }

    fn apply(self: &Rc<Self>, feed: Feed, first: bool) {
        if first && !feed.chips.is_empty() {
            self.set_chips(&feed.chips);
        }
        let added = feed.items.len();
        for item in &feed.items {
            let w = cards::item_widget(item, self.layout);
            match self.layout {
                Layout::Grid => {
                    let child = gtk::FlowBoxChild::new();
                    child.set_child(Some(&w));
                    child.add_css_class("card-child");
                    self.grid.append(&child);
                }
                Layout::List => {
                    let row = gtk::ListBoxRow::new();
                    let w = match self.row_fn.borrow().as_ref() {
                        Some(f) => f(&row, &w),
                        None => w,
                    };
                    row.set_child(Some(&w));
                    self.list.append(&row);
                }
            }
        }
        self.items.borrow_mut().extend(feed.items);
        self.continuation.replace(feed.continuation);
        if self.items.borrow().is_empty() && self.continuation.borrow().is_none() {
            let (icon, title, desc) = self.empty.borrow().clone();
            self.show_status(&icon, &title, &desc, false);
        }
        // Keep fetching while the page isn't full yet (or a page came back
        // empty because everything on it was filtered out).
        if added == 0 {
            self.empty_loads.set(self.empty_loads.get() + 1);
        } else {
            self.empty_loads.set(0);
        }
        if self.empty_loads.get() < 4 {
            let weak = Rc::downgrade(self);
            glib::idle_add_local_once(move || {
                if let Some(t) = weak.upgrade() {
                    let adj = t.scroller.vadjustment();
                    if adj.upper() <= adj.page_size() + 1200.0 || added == 0 {
                        t.load_more();
                    }
                }
            });
        }
    }

    fn set_chips(self: &Rc<Self>, chips: &[Chip]) {
        while let Some(c) = self.chips.first_child() {
            self.chips.remove(&c);
        }
        self.chips_scroller.set_visible(!chips.is_empty());
        self.chip_token.replace(None);
        // Not a radio group: clicking the selected chip again deselects it
        // and goes back to the unfiltered feed (the chip that came selected,
        // usually "All").
        let default = chips.iter().position(|c| c.selected);
        let buttons: Rc<RefCell<Vec<gtk::ToggleButton>>> = Rc::new(RefCell::new(Vec::new()));
        let updating = Rc::new(Cell::new(false));
        for (i, chip) in chips.iter().enumerate() {
            let b = gtk::ToggleButton::with_label(&chip.title);
            b.add_css_class("chip");
            b.set_active(chip.selected);
            let token = chip.token.clone();
            let weak = Rc::downgrade(self);
            let (all, updating) = (buttons.clone(), updating.clone());
            b.connect_toggled(move |b| {
                if updating.get() {
                    return;
                }
                let Some(t) = weak.upgrade() else { return };
                let select = |k: Option<usize>| {
                    updating.set(true);
                    for (j, o) in all.borrow().iter().enumerate() {
                        o.set_active(Some(j) == k);
                    }
                    updating.set(false);
                };
                let token = if b.is_active() {
                    select(Some(i));
                    (Some(i) != default).then(|| token.clone())
                } else if Some(i) == default {
                    // "All" can't be deselected.
                    select(Some(i));
                    return;
                } else {
                    select(default);
                    None
                };
                t.chip_token.replace(token.clone());
                t.generation.set(t.generation.get() + 1);
                t.clear();
                t.status.set_visible(false);
                t.load(token, false);
            });
            buttons.borrow_mut().push(b.clone());
            self.chips.append(&b);
        }
    }
}
