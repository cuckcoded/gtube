//! "You": the signed-in user's profile, watch history, playlists, Watch
//! later, liked videos and subscribed channels.

use super::cards::Layout;
use super::feed::{FeedView, Source};
use crate::api::models::Account;
use crate::images;
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub struct Library {
    pub page: adw::NavigationPage,
    stack: adw::ViewStack,
    feeds: Vec<(String, Rc<FeedView>)>,
    avatar: adw::Avatar,
    name: gtk::Label,
    handle: gtk::Label,
    channel_btn: gtk::Button,
    channel_id: RefCell<Option<String>>,
    header: gtk::Box,
}

impl Library {
    pub fn new() -> Rc<Self> {
        let stack = adw::ViewStack::new();
        stack.set_vexpand(true);
        let tabs: [(&str, &str, &str, &str, Layout); 5] = [
            ("history", "History", "document-open-recent-symbolic", "FEhistory", Layout::Grid),
            ("playlists", "Playlists", "view-list-symbolic", "FEplaylist_aggregation", Layout::Grid),
            ("later", "Watch later", "alarm-symbolic", "VLWL", Layout::List),
            ("liked", "Liked videos", "gtube-like-symbolic", "VLLL", Layout::List),
            ("channels", "Subscriptions", "gtube-subscriptions-symbolic", "FEchannels", Layout::Grid),
        ];
        let mut feeds = Vec::new();
        for (name, title, icon, browse, layout) in tabs {
            let f = FeedView::new(Source::Browse { id: browse.into(), params: None }, layout);
            f.needs_login.set(true);
            f.set_empty_state(icon, &format!("No {}", title.to_lowercase()), "");
            if layout == Layout::List {
                let (t, list) = (title.to_string(), browse.trim_start_matches("VL").to_string());
                let fw = Rc::downgrade(&f);
                f.connect_activate(move |i, _| {
                    if let Some(f) = fw.upgrade() {
                        super::playlist::play_from(&f, &t, Some(&list), i, false);
                    }
                });
            }
            let p = stack.add_titled(&f.widget, Some(name), title);
            p.set_icon_name(Some(icon));
            feeds.push((name.to_string(), f));
        }

        let switcher = adw::ViewSwitcher::builder().stack(&stack).policy(adw::ViewSwitcherPolicy::Wide).build();
        switcher.set_halign(gtk::Align::Center);
        let avatar = adw::Avatar::new(72, None, true);
        let name = gtk::Label::new(None);
        name.add_css_class("title-1");
        name.set_xalign(0.0);
        let handle = gtk::Label::new(None);
        handle.add_css_class("dim-label");
        handle.set_xalign(0.0);
        let channel_btn = gtk::Button::with_label("View channel");
        channel_btn.add_css_class("pill");
        channel_btn.set_halign(gtk::Align::Start);
        let names = gtk::Box::new(gtk::Orientation::Vertical, 4);
        names.set_valign(gtk::Align::Center);
        names.append(&name);
        names.append(&handle);
        names.append(&channel_btn);
        let header = gtk::Box::new(gtk::Orientation::Horizontal, 20);
        header.set_margin_top(18);
        header.set_margin_start(24);
        header.set_margin_end(24);
        header.append(&avatar);
        header.append(&names);
        header.set_visible(false);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.append(&header);
        content.append(&switcher);
        content.append(&stack);
        let page = adw::NavigationPage::builder().title("You").tag("you").child(&content).build();

        let this = Rc::new(Self { page, stack, feeds, avatar, name, handle, channel_btn, channel_id: RefCell::new(None), header });
        let w = Rc::downgrade(&this);
        this.channel_btn.connect_clicked(move |_| {
            if let Some(id) = w.upgrade().and_then(|t| t.channel_id.borrow().clone()) {
                super::open_channel(&id);
            }
        });
        let w = Rc::downgrade(&this);
        this.stack.connect_visible_child_name_notify(move |_| {
            if let Some(t) = w.upgrade() {
                t.ensure_visible_loaded();
            }
        });
        let w = Rc::downgrade(&this);
        this.page.connect_shown(move |_| {
            if let Some(t) = w.upgrade() {
                t.ensure_visible_loaded();
            }
        });
        this
    }

    fn ensure_visible_loaded(&self) {
        let name = self.stack.visible_child_name().map(|s| s.to_string()).unwrap_or_default();
        if let Some((_, f)) = self.feeds.iter().find(|(n, _)| *n == name) {
            f.ensure_loaded();
        }
    }

    pub fn show_tab(&self, name: &str) {
        self.stack.set_visible_child_name(name);
        self.ensure_visible_loaded();
    }

    pub fn set_account(&self, acc: Option<&Account>) {
        self.header.set_visible(acc.is_some());
        if let Some(a) = acc {
            self.name.set_text(&a.name);
            self.handle.set_text(a.handle.as_deref().unwrap_or(""));
            self.avatar.set_text(Some(&a.name));
            images::avatar(&self.avatar, a.avatar.as_deref());
            self.channel_btn.set_visible(a.channel_id.is_some());
            self.channel_id.replace(a.channel_id.clone());
        }
    }

    /// Drops loaded data, e.g. after signing in or out.
    pub fn reset(&self) {
        for (_, f) in &self.feeds {
            f.invalidate();
        }
        if self.page.is_mapped() {
            self.ensure_visible_loaded();
        }
    }
}
