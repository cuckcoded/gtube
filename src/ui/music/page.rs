//! A Music page: any browse or search result rendered as a header, chips,
//! a top result and shelves, loading more as it is scrolled.

use super::widgets::{self as w, Sink};
use crate::api::{self, music as m, music::Target};
use crate::runtime;
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Browse { id: String, params: Option<String> },
    Search { query: String, params: Option<String> },
}

impl Source {
    pub fn browse(id: &str) -> Self {
        Source::Browse { id: id.to_string(), params: None }
    }

    pub fn from_target(t: &Target) -> Option<Self> {
        match t {
            Target::Browse { id, params } => Some(Source::Browse { id: id.clone(), params: params.clone() }),
            Target::Search { query, params } => Some(Source::Search { query: query.clone(), params: params.clone() }),
            Target::Watch { .. } => None,
        }
    }

    fn endpoint(&self) -> &'static str {
        match self {
            Source::Browse { .. } => "browse",
            Source::Search { .. } => "search",
        }
    }

    fn browse_id(&self) -> Option<&str> {
        match self {
            Source::Browse { id, .. } => Some(id),
            Source::Search { .. } => None,
        }
    }
}

pub struct PageView {
    pub widget: gtk::Stack,
    scroller: gtk::ScrolledWindow,
    body: gtk::Box,
    status_icon: gtk::Image,
    status_title: gtk::Label,
    status_desc: gtk::Label,
    retry: gtk::Button,
    signin: gtk::Button,
    source: RefCell<Option<Source>>,
    /// What clicking the selected chip goes back to.
    base: RefCell<Option<Source>>,
    generation: Cell<u64>,
    loaded: Cell<bool>,
    busy: Cell<bool>,
    page_cont: RefCell<Option<String>>,
    list_cont: RefCell<Option<(Sink, String)>>,
    pub needs_login: Cell<bool>,
    /// Shows the page's own title (pages without a header).
    pub show_title: Cell<bool>,
    pub on_loaded: RefCell<Option<Box<dyn Fn(&m::Page)>>>,
}

impl PageView {
    pub fn new() -> Rc<Self> {
        let body = gtk::Box::new(gtk::Orientation::Vertical, 36);
        body.set_margin_top(18);
        body.set_margin_bottom(36);
        body.set_margin_start(24);
        body.set_margin_end(24);
        let clamp = adw::Clamp::builder().maximum_size(1500).tightening_threshold(1200).child(&body).build();
        let scroller = crate::ui::widgets::scrolled(&clamp);
        scroller.set_vexpand(true);

        let spinner = adw::Spinner::new();
        spinner.set_size_request(40, 40);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);

        let status = gtk::Box::new(gtk::Orientation::Vertical, 12);
        status.set_valign(gtk::Align::Center);
        status.set_halign(gtk::Align::Center);
        status.set_margin_start(24);
        status.set_margin_end(24);
        let status_icon = gtk::Image::from_icon_name("gtube-music-symbolic");
        status_icon.set_pixel_size(64);
        status_icon.add_css_class("dim-label");
        let status_title = gtk::Label::new(None);
        status_title.add_css_class("title-2");
        status_title.set_wrap(true);
        status_title.set_justify(gtk::Justification::Center);
        let status_desc = gtk::Label::new(None);
        status_desc.set_wrap(true);
        status_desc.set_justify(gtk::Justification::Center);
        status_desc.add_css_class("dim-label");
        status_desc.set_max_width_chars(60);
        let retry = gtk::Button::with_label("Try again");
        retry.add_css_class("pill");
        retry.set_halign(gtk::Align::Center);
        let signin = gtk::Button::with_label("Sign in");
        signin.add_css_class("pill");
        signin.add_css_class("suggested-action");
        signin.set_halign(gtk::Align::Center);
        signin.set_action_name(Some("win.signin"));
        for x in [status_icon.upcast_ref::<gtk::Widget>(), status_title.upcast_ref(), status_desc.upcast_ref(), retry.upcast_ref(), signin.upcast_ref()] {
            status.append(x);
        }

        let widget = gtk::Stack::new();
        widget.set_transition_type(gtk::StackTransitionType::Crossfade);
        widget.add_named(&spinner, Some("loading"));
        widget.add_named(&status, Some("status"));
        widget.add_named(&scroller, Some("content"));

        let this = Rc::new(Self {
            widget,
            scroller,
            body,
            status_icon,
            status_title,
            status_desc,
            retry,
            signin,
            source: RefCell::new(None),
            base: RefCell::new(None),
            generation: Cell::new(0),
            loaded: Cell::new(false),
            busy: Cell::new(false),
            page_cont: RefCell::new(None),
            list_cont: RefCell::new(None),
            needs_login: Cell::new(false),
            show_title: Cell::new(true),
            on_loaded: RefCell::new(None),
        });
        let wk = Rc::downgrade(&this);
        this.retry.connect_clicked(move |_| {
            if let Some(p) = wk.upgrade() {
                p.load();
            }
        });
        let adj = this.scroller.vadjustment();
        let wk = Rc::downgrade(&this);
        adj.connect_value_changed(move |_| {
            if let Some(p) = wk.upgrade() {
                p.check_more();
            }
        });
        let wk = Rc::downgrade(&this);
        adj.connect_changed(move |_| {
            if let Some(p) = wk.upgrade() {
                p.check_more();
            }
        });
        this
    }

    pub fn source(&self) -> Option<Source> {
        self.source.borrow().clone()
    }

    /// Sets what the page shows (loaded on the next `ensure_loaded`/`load`).
    pub fn set_source(&self, s: Source) {
        self.base.replace(Some(s.clone()));
        self.source.replace(Some(s));
        self.loaded.set(false);
    }

    /// Shows `s` (a filter of the base source) on the next load.
    pub fn set_current(&self, s: Source) {
        self.source.replace(Some(s));
        self.loaded.set(false);
    }

    pub fn ensure_loaded(self: &Rc<Self>) {
        if !self.loaded.get() {
            self.load();
        }
    }

    /// Forgets the content (session changed); reloaded when next shown.
    pub fn invalidate(&self) {
        self.loaded.set(false);
        self.generation.set(self.generation.get() + 1);
        if let Some(b) = self.base.borrow().clone() {
            self.source.replace(Some(b));
        }
    }

    fn status(&self, icon: &str, title: &str, desc: &str, retry: bool, signin: bool) {
        self.status_icon.set_icon_name(Some(icon));
        self.status_title.set_text(title);
        self.status_desc.set_text(desc);
        self.status_desc.set_visible(!desc.is_empty());
        self.retry.set_visible(retry);
        self.signin.set_visible(signin);
        self.widget.set_visible_child_name("status");
    }

    pub fn load(self: &Rc<Self>) {
        let Some(src) = self.source() else { return };
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.loaded.set(true);
        self.busy.set(false);
        self.page_cont.replace(None);
        self.list_cont.replace(None);
        if self.needs_login.get() && !api::client().is_logged_in() {
            self.status("gtube-library-symbolic", "Enjoy your favorite music", "Sign in to see the songs, albums, artists and playlists you saved, and your listening history.", false, true);
            return;
        }
        self.widget.set_visible_child_name("loading");
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let s = src.clone();
            let res = runtime::run(async move {
                let c = api::client();
                let v = match &s {
                    Source::Browse { id, params } => c.music_browse(id, params.as_deref()).await?,
                    Source::Search { query, params } => c.music_search(query, params.as_deref()).await?,
                };
                Ok(m::parse_page(&v))
            })
            .await;
            let Some(p) = wk.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            match res {
                Ok(page) => p.render(&src, page),
                Err(e) => {
                    crate::log::warn(&format!("music page {src:?} failed: {e:#}"));
                    p.status("network-error-symbolic", "Couldn't load this page", &e.to_string(), true, false);
                }
            }
        });
    }

    fn clear(&self) {
        while let Some(c) = self.body.first_child() {
            self.body.remove(&c);
        }
    }

    fn render(self: &Rc<Self>, src: &Source, page: m::Page) {
        self.clear();
        if let Some(h) = &page.header {
            self.body.append(&w::header_widget(h, src.browse_id()));
        } else if self.show_title.get()
            && let Some(t) = &page.title
        {
            let l = w::label(t, &["title-1"]);
            l.set_wrap(true);
            self.body.append(&l);
        }
        if !page.chips.is_empty() {
            let wk = Rc::downgrade(self);
            let row = w::chips_row(&page.chips, move |t| {
                let Some(p) = wk.upgrade() else { return };
                let next = match t {
                    Some(t) => Source::from_target(&t),
                    None => p.base.borrow().clone(),
                };
                if let Some(n) = next {
                    p.source.replace(Some(n));
                    p.load();
                }
            });
            // Chips sit right under the header, closer than shelves.
            row.set_margin_bottom(-18);
            self.body.append(&row);
        }
        if let Some(t) = &page.top {
            self.body.append(&w::top_result(t));
        }
        self.add_shelves(&page.shelves);
        self.page_cont.replace(page.continuation.clone());
        let empty = page.header.is_none() && page.top.is_none() && page.shelves.is_empty();
        if empty {
            let (title, desc) = match src {
                Source::Search { .. } => ("No results", "Try different keywords, or another filter."),
                _ => ("Nothing here yet", ""),
            };
            self.status("gtube-music-symbolic", title, desc, false, false);
        } else {
            self.widget.set_visible_child_name("content");
            self.scroller.vadjustment().set_value(0.0);
        }
        if let Some(f) = self.on_loaded.borrow().as_ref() {
            f(&page);
        }
        let wk = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            if let Some(p) = wk.upgrade() {
                p.check_more();
            }
        });
    }

    fn add_shelves(&self, shelves: &[m::Shelf]) {
        for s in shelves {
            let (widget, sink) = w::shelf_widget(s);
            self.body.append(&widget);
            if let (Some(sink), Some(c)) = (sink, s.continuation.clone())
                && self.list_cont.borrow().is_none()
            {
                self.list_cont.replace(Some((sink, c)));
            }
        }
    }

    /// Loads more once the end of the page is near.
    fn check_more(self: &Rc<Self>) {
        if self.busy.get() || !self.loaded.get() || self.widget.visible_child_name().as_deref() != Some("content") {
            return;
        }
        let adj = self.scroller.vadjustment();
        if adj.upper() <= 0.0 || adj.value() + adj.page_size() < adj.upper() - 800.0 {
            return;
        }
        let Some(src) = self.source() else { return };
        let list = self.list_cont.borrow().clone();
        let token = match &list {
            Some((_, t)) => t.clone(),
            None => match self.page_cont.borrow().clone() {
                Some(t) => t,
                None => return,
            },
        };
        self.busy.set(true);
        let generation = self.generation.get();
        let wk = Rc::downgrade(self);
        let endpoint = src.endpoint();
        runtime::spawn_local(async move {
            let res = runtime::run(async move { api::client().music_continuation(endpoint, &token).await.map(|v| m::parse_more(&v)) }).await;
            let Some(p) = wk.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            p.busy.set(false);
            let more = match res {
                Ok(m) => m,
                Err(e) => {
                    crate::log::warn(&format!("music continuation failed: {e:#}"));
                    // Don't retry in a loop; scrolling again retries.
                    p.list_cont.replace(None);
                    p.page_cont.replace(None);
                    return;
                }
            };
            match list {
                Some((sink, _)) => {
                    sink.append(&more.entries);
                    p.list_cont.replace(more.continuation.map(|c| (sink, c)));
                }
                None => {
                    p.add_shelves(&more.shelves);
                    p.page_cont.replace(more.continuation);
                }
            }
            let wk = Rc::downgrade(&p);
            glib::idle_add_local_once(move || {
                if let Some(p) = wk.upgrade() {
                    p.check_more();
                }
            });
        });
    }
}
