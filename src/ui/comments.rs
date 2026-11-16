//! Comment threads under a video.

use crate::api::{self, models::*, parse};
use crate::{images, runtime};
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

async fn fetch(token: String) -> anyhow::Result<CommentsPage> {
    runtime::run(async move {
        let v = api::client().next_continuation(&token).await?;
        Ok(parse::parse_comments(&v))
    })
    .await
}

pub struct Comments {
    pub widget: gtk::Box,
    count: gtk::Label,
    sort: gtk::DropDown,
    list: gtk::Box,
    more: gtk::Button,
    spinner: adw::Spinner,
    empty: gtk::Label,
    token: RefCell<Option<String>>,
    sorts: RefCell<Vec<(String, String)>>,
    generation: Cell<u64>,
    loading: Cell<bool>,
    updating: Cell<bool>,
}

impl Comments {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 16);
        widget.set_margin_top(12);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let title = gtk::Label::new(Some("Comments"));
        title.add_css_class("title-3");
        let count = gtk::Label::new(None);
        count.add_css_class("dim-label");
        let sort = gtk::DropDown::from_strings(&[]);
        sort.set_visible(false);
        sort.set_valign(gtk::Align::Center);
        header.append(&title);
        header.append(&count);
        header.append(&sort);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 20);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(28, 28);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_visible(false);
        let more = gtk::Button::with_label("Load more comments");
        more.add_css_class("pill");
        more.set_halign(gtk::Align::Center);
        more.set_visible(false);
        let empty = gtk::Label::new(Some("Comments are turned off or unavailable."));
        empty.add_css_class("dim-label");
        empty.set_xalign(0.0);
        empty.set_visible(false);
        widget.append(&header);
        widget.append(&empty);
        widget.append(&list);
        widget.append(&spinner);
        widget.append(&more);

        let this = Rc::new(Self {
            widget,
            count,
            sort,
            list,
            more,
            spinner,
            empty,
            token: RefCell::new(None),
            sorts: RefCell::new(Vec::new()),
            generation: Cell::new(0),
            loading: Cell::new(false),
            updating: Cell::new(false),
        });
        let w = Rc::downgrade(&this);
        this.more.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                t.load_more();
            }
        });
        let w = Rc::downgrade(&this);
        this.sort.connect_selected_notify(move |dd| {
            let Some(t) = w.upgrade() else { return };
            if t.updating.get() {
                return;
            }
            let tok = t.sorts.borrow().get(dd.selected() as usize).map(|s| s.1.clone());
            if let Some(tok) = tok {
                t.start(Some(tok));
            }
        });
        this
    }

    pub fn reset(self: &Rc<Self>, token: Option<String>, sorts: Vec<(String, String)>, count: Option<String>) {
        self.count.set_text(count.as_deref().unwrap_or(""));
        self.updating.set(true);
        let names: Vec<&str> = sorts.iter().map(|s| s.0.as_str()).collect();
        self.sort.set_model(Some(&gtk::StringList::new(&names)));
        self.sort.set_selected(0);
        self.sort.set_visible(!sorts.is_empty());
        self.updating.set(false);
        self.sorts.replace(sorts);
        self.start(token);
    }

    pub fn clear(&self) {
        self.generation.set(self.generation.get() + 1);
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.token.replace(None);
        self.more.set_visible(false);
        self.empty.set_visible(false);
        self.loading.set(false);
        self.spinner.set_visible(false);
    }

    fn start(self: &Rc<Self>, token: Option<String>) {
        self.clear();
        match token {
            Some(t) => {
                self.token.replace(Some(t));
                self.load_more();
            }
            None => self.empty.set_visible(true),
        }
    }

    pub fn load_more(self: &Rc<Self>) {
        if self.loading.get() {
            return;
        }
        let Some(token) = self.token.borrow().clone() else { return };
        self.loading.set(true);
        self.spinner.set_visible(true);
        self.more.set_visible(false);
        let generation = self.generation.get();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = fetch(token).await;
            let Some(t) = w.upgrade() else { return };
            if t.generation.get() != generation {
                return;
            }
            t.loading.set(false);
            t.spinner.set_visible(false);
            match res {
                Ok(page) => {
                    if page.comments.is_empty() && t.list.first_child().is_none() {
                        t.empty.set_visible(true);
                    }
                    for c in &page.comments {
                        t.list.append(&comment_widget(c, false));
                    }
                    t.more.set_visible(page.continuation.is_some());
                    t.token.replace(page.continuation);
                }
                Err(e) => {
                    t.more.set_visible(true);
                    super::toast(&format!("Could not load comments: {e}"));
                }
            }
        });
    }
}

fn comment_widget(c: &Comment, reply: bool) -> gtk::Widget {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("comment");
    let av = adw::Avatar::new(if reply { 28 } else { 40 }, Some(c.author.trim_start_matches('@')), true);
    av.set_valign(gtk::Align::Start);
    images::avatar(&av, c.avatar.as_deref());
    if let Some(id) = c.author_channel_id.clone() {
        av.set_cursor_from_name(Some("pointer"));
        let g = gtk::GestureClick::new();
        g.connect_released(move |_, _, _, _| super::open_channel(&id));
        av.add_controller(g);
    }
    row.append(&av);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 4);
    body.set_hexpand(true);
    if let Some(p) = &c.pinned {
        let l = gtk::Label::new(Some(&format!("📌 {p}")));
        l.add_css_class("dim-label");
        l.add_css_class("caption");
        l.set_xalign(0.0);
        body.append(&l);
    }
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let author = gtk::Label::new(Some(&c.author));
    author.add_css_class("comment-author");
    if c.is_creator {
        author.add_css_class("creator");
    }
    head.append(&author);
    if c.is_verified {
        let v = gtk::Label::new(Some("✓"));
        v.add_css_class("dim-label");
        head.append(&v);
    }
    let time = gtk::Label::new(Some(&c.published));
    time.add_css_class("dim-label");
    time.add_css_class("caption");
    head.append(&time);
    body.append(&head);
    body.append(&super::text::label(&c.text));

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let likes = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    likes.append(&gtk::Image::from_icon_name("gtube-like-symbolic"));
    likes.append(&gtk::Label::new(Some(c.likes.as_deref().unwrap_or(""))));
    likes.add_css_class("dim-label");
    foot.append(&likes);
    if c.hearted {
        let h = gtk::Label::new(Some("❤"));
        h.add_css_class("heart");
        h.set_tooltip_text(Some("Hearted by the creator"));
        foot.append(&h);
    }
    body.append(&foot);

    if let (Some(token), false) = (c.replies_token.clone(), reply) {
        // Replies are fetched lazily the first time they are expanded.
        let replies = gtk::Box::new(gtk::Orientation::Vertical, 14);
        replies.set_margin_top(8);
        replies.set_visible(false);
        let items = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let more = gtk::Button::with_label("Show more replies");
        more.add_css_class("flat");
        more.add_css_class("replies-btn");
        more.set_halign(gtk::Align::Start);
        more.set_visible(false);
        replies.append(&items);
        replies.append(&more);

        let n = c.reply_count.clone().unwrap_or_default();
        let btn = gtk::Button::with_label(&format!("▾ {n} replies"));
        btn.add_css_class("flat");
        btn.add_css_class("replies-btn");
        btn.set_halign(gtk::Align::Start);

        let state: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(Some(token)));
        let load = {
            let (items, more, state) = (items.clone(), more.clone(), state.clone());
            Rc::new(move || {
                let Some(tok) = state.borrow_mut().take() else { return };
                let (items, more, state) = (items.clone(), more.clone(), state.clone());
                runtime::spawn_local(async move {
                    match fetch(tok).await {
                        Ok(page) => {
                            for r in &page.comments {
                                items.append(&comment_widget(r, true));
                            }
                            more.set_visible(page.continuation.is_some());
                            state.replace(page.continuation);
                        }
                        Err(e) => super::toast(&format!("Could not load replies: {e}")),
                    }
                });
            })
        };
        let loaded = Cell::new(false);
        let (l2, rep) = (load.clone(), replies.clone());
        btn.connect_clicked(move |b| {
            let show = !rep.is_visible();
            rep.set_visible(show);
            b.set_label(&format!("{} {n} replies", if show { "▴" } else { "▾" }));
            if show && !loaded.replace(true) {
                l2();
            }
        });
        more.connect_clicked(move |_| load());
        body.append(&btn);
        body.append(&replies);
    }
    row.append(&body);
    row.upcast()
}
