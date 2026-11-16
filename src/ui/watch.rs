//! The watch page: player area, video info, likes/dislikes, description,
//! comments and related videos (and the queue, when playing a list).

use super::cards::{self, Layout};
use super::comments::Comments;
pub use super::queue::Queue;
use super::queue::QueuePanel;
use super::widgets::AspectBin;
use crate::api::{self, LikeAction, extras, models::*, parse};
use crate::{config, images, runtime};
use adw::prelude::*;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct WatchPage {
    pub page: adw::NavigationPage,
    scroller: gtk::ScrolledWindow,
    pub placeholder: AspectBin,
    title: gtk::Label,
    avatar: adw::Avatar,
    channel_name: gtk::Label,
    channel_subs: gtk::Label,
    subscribe_btn: gtk::Button,
    like_btn: gtk::ToggleButton,
    like_label: gtk::Label,
    dislike_btn: gtk::ToggleButton,
    dislike_label: gtk::Label,
    ratio: gtk::LevelBar,
    meta: gtk::Label,
    sb_info: gtk::Label,
    desc: gtk::Label,
    desc_more: gtk::Button,
    comments: Rc<Comments>,
    related: gtk::ListBox,
    related_items: RefCell<Vec<VideoItem>>,
    related_more: gtk::Button,
    related_token: RefCell<Option<String>>,
    autoplay: gtk::Switch,
    queue: Rc<QueuePanel>,
    video: RefCell<Option<VideoItem>>,
    details: RefCell<Option<VideoDetails>>,
    subscribed: Cell<bool>,
    generation: Cell<u64>,
    updating: Cell<bool>,
    opening: Cell<bool>,
}

fn dim(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class("dim-label");
    l.set_xalign(0.0);
    l
}

impl WatchPage {
    pub fn new() -> Rc<Self> {
        let placeholder = AspectBin::new(16.0 / 9.0);
        placeholder.add_css_class("video-placeholder");
        placeholder.set_hexpand(true);

        let title = gtk::Label::new(None);
        title.add_css_class("title-2");
        title.set_wrap(true);
        title.set_xalign(0.0);
        title.set_selectable(true);

        // Channel row
        let avatar = adw::Avatar::new(44, None, true);
        let channel_name = gtk::Label::new(None);
        channel_name.add_css_class("heading");
        channel_name.set_xalign(0.0);
        channel_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let channel_subs = dim("");
        channel_subs.add_css_class("caption");
        let names = gtk::Box::new(gtk::Orientation::Vertical, 2);
        names.set_valign(gtk::Align::Center);
        names.append(&channel_name);
        names.append(&channel_subs);
        let channel_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        channel_box.append(&avatar);
        channel_box.append(&names);
        let channel_btn = gtk::Button::builder().child(&channel_box).build();
        channel_btn.add_css_class("flat");
        channel_btn.add_css_class("channel-button");
        let subscribe_btn = gtk::Button::with_label("Subscribe");
        subscribe_btn.add_css_class("pill");
        subscribe_btn.add_css_class("subscribe");
        subscribe_btn.set_valign(gtk::Align::Center);

        // Likes / dislikes
        let like_label = gtk::Label::new(Some("–"));
        let like_inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        like_inner.append(&gtk::Image::from_icon_name("gtube-like-symbolic"));
        like_inner.append(&like_label);
        let like_btn = gtk::ToggleButton::builder().child(&like_inner).tooltip_text("I like this").build();
        let dislike_label = gtk::Label::new(Some("–"));
        let dislike_inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dislike_inner.append(&gtk::Image::from_icon_name("gtube-dislike-symbolic"));
        dislike_inner.append(&dislike_label);
        let dislike_btn = gtk::ToggleButton::builder()
            .child(&dislike_inner)
            .tooltip_text("Dislikes are estimated by Return YouTube Dislike")
            .build();
        let rating = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rating.add_css_class("linked");
        rating.append(&like_btn);
        rating.append(&dislike_btn);
        let ratio = gtk::LevelBar::new();
        ratio.set_min_value(0.0);
        ratio.set_max_value(1.0);
        ratio.add_css_class("ratio-bar");
        ratio.set_visible(false);
        for o in ["low", "high", "full"] {
            ratio.remove_offset_value(Some(o));
        }
        let rating_col = gtk::Box::new(gtk::Orientation::Vertical, 4);
        rating_col.set_valign(gtk::Align::Center);
        rating_col.append(&rating);
        rating_col.append(&ratio);

        let share_menu = gio::Menu::new();
        share_menu.append(Some("Copy link"), Some("watch.copy"));
        share_menu.append(Some("Copy link at current time"), Some("watch.copy-time"));
        share_menu.append(Some("Open in browser"), Some("watch.browser"));
        let share_btn = gtk::MenuButton::builder()
            .icon_name("send-to-symbolic")
            .menu_model(&share_menu)
            .tooltip_text("Share")
            .valign(gtk::Align::Center)
            .build();
        share_btn.add_css_class("circular");
        let download_btn = gtk::Button::from_icon_name("folder-download-symbolic");
        download_btn.set_tooltip_text(Some("Download"));
        download_btn.add_css_class("circular");
        download_btn.set_valign(gtk::Align::Center);
        let audio_btn = gtk::Button::from_icon_name("audio-headphones-symbolic");
        audio_btn.set_tooltip_text(Some("Toggle audio only"));
        audio_btn.add_css_class("circular");
        audio_btn.set_valign(gtk::Align::Center);
        let mini_btn = gtk::Button::from_icon_name("gtube-miniplayer-symbolic");
        mini_btn.set_tooltip_text(Some("Miniplayer"));
        mini_btn.add_css_class("circular");
        mini_btn.set_valign(gtk::Align::Center);

        let actions_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        actions_row.set_halign(gtk::Align::End);
        actions_row.set_hexpand(true);
        actions_row.append(&rating_col);
        actions_row.append(&audio_btn);
        actions_row.append(&mini_btn);
        actions_row.append(&download_btn);
        actions_row.append(&share_btn);

        let owner_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        owner_row.append(&channel_btn);
        owner_row.append(&subscribe_btn);
        // Channel + buttons on one line, wrapping when the column is narrow.
        let info_row = adw::WrapBox::builder().child_spacing(12).line_spacing(8).build();
        info_row.append(&owner_row);
        info_row.append(&actions_row);

        // Description
        let meta = gtk::Label::new(None);
        meta.add_css_class("heading");
        meta.set_xalign(0.0);
        meta.set_wrap(true);
        let sb_info = dim("");
        sb_info.add_css_class("caption");
        sb_info.set_wrap(true);
        sb_info.set_visible(false);
        let desc = super::text::label("");
        let desc_more = gtk::Button::with_label("Show more");
        desc_more.add_css_class("flat");
        desc_more.set_halign(gtk::Align::Start);
        let desc_card = gtk::Box::new(gtk::Orientation::Vertical, 8);
        desc_card.add_css_class("description-card");
        desc_card.append(&meta);
        desc_card.append(&sb_info);
        desc_card.append(&desc);
        desc_card.append(&desc_more);

        let comments = Comments::new();

        let left = gtk::Box::new(gtk::Orientation::Vertical, 12);
        left.set_hexpand(true);
        left.append(&placeholder);
        left.append(&title);
        left.append(&info_row);
        left.append(&desc_card);
        left.append(&comments.widget);

        // Right column: queue + related
        let queue = QueuePanel::new();

        let up_next = gtk::Label::new(Some("Up next"));
        up_next.add_css_class("heading");
        up_next.set_hexpand(true);
        up_next.set_xalign(0.0);
        let autoplay = gtk::Switch::new();
        autoplay.set_active(config::settings().autoplay);
        autoplay.set_valign(gtk::Align::Center);
        let ap_label = dim("Autoplay");
        let related_head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        related_head.append(&up_next);
        related_head.append(&ap_label);
        related_head.append(&autoplay);
        let related = gtk::ListBox::new();
        related.add_css_class("feed-list");
        related.set_selection_mode(gtk::SelectionMode::None);
        let related_more = gtk::Button::with_label("Show more");
        related_more.add_css_class("pill");
        related_more.set_halign(gtk::Align::Center);
        related_more.set_visible(false);

        let right = gtk::Box::new(gtk::Orientation::Vertical, 12);
        right.append(&queue.widget);
        right.append(&related_head);
        right.append(&related);
        right.append(&related_more);

        let columns = super::widgets::SideLayout::new(&left, &right);
        columns.set_margin_top(16);
        columns.set_margin_bottom(32);
        columns.set_margin_start(24);
        columns.set_margin_end(24);
        // As wide as the window allows while the player still fits on
        // screen with the title below it (big screens).
        columns.set_fit_height(16.0 / 9.0, 16 + 150);
        let scroller = super::widgets::scrolled(&columns);

        let bp_bin = adw::BreakpointBin::builder().width_request(360).height_request(300).child(&scroller).build();
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 1000sp").unwrap());
        let c = columns.clone();
        bp.connect_apply(move |_| c.set_stacked(true));
        let c = columns.clone();
        bp.connect_unapply(move |_| c.set_stacked(false));
        bp.add_setter(&columns, "margin-start", Some(&12.to_value()));
        bp.add_setter(&columns, "margin-end", Some(&12.to_value()));
        bp_bin.add_breakpoint(bp);
        let wide = adw::Breakpoint::new(adw::BreakpointCondition::parse("min-width: 1900sp").unwrap());
        let c = columns.clone();
        wide.connect_apply(move |_| c.set_side_width(440));
        let c = columns.clone();
        wide.connect_unapply(move |_| c.set_side_width(super::widgets::SideLayout::SIDE_WIDTH));
        bp_bin.add_breakpoint(wide);

        let page = adw::NavigationPage::builder().title("Watch").tag("watch").child(&bp_bin).build();

        let this = Rc::new(Self {
            page,
            scroller,
            placeholder,
            title,
            avatar,
            channel_name,
            channel_subs,
            subscribe_btn,
            like_btn,
            like_label,
            dislike_btn,
            dislike_label,
            ratio,
            meta,
            sb_info,
            desc,
            desc_more,
            comments,
            related,
            related_items: RefCell::new(Vec::new()),
            related_more,
            related_token: RefCell::new(None),
            autoplay,
            queue,
            video: RefCell::new(None),
            details: RefCell::new(None),
            subscribed: Cell::new(false),
            generation: Cell::new(0),
            updating: Cell::new(false),
            opening: Cell::new(false),
        });
        this.connect(channel_btn, download_btn, audio_btn, mini_btn);
        this
    }

    fn connect(self: &Rc<Self>, channel_btn: gtk::Button, download_btn: gtk::Button, audio_btn: gtk::Button, mini_btn: gtk::Button) {
        let w = Rc::downgrade(self);
        channel_btn.connect_clicked(move |_| {
            if let Some(t) = w.upgrade()
                && let Some(id) = t.details.borrow().as_ref().map(|d| d.channel.id.clone()).filter(|s| !s.is_empty())
            {
                super::open_channel(&id);
            }
        });
        let w = Rc::downgrade(self);
        download_btn.connect_clicked(move |_| {
            if let Some(v) = w.upgrade().and_then(|t| t.video.borrow().clone()) {
                super::downloads::show_dialog(&v);
            }
        });
        audio_btn.connect_clicked(|_| {
            let p = super::ctx().player.clone();
            p.set_audio_only(!config::settings().audio_only);
        });
        mini_btn.connect_clicked(|_| super::ctx().player.minimize());
        let w = Rc::downgrade(self);
        self.desc_more.connect_clicked(move |b| {
            let Some(t) = w.upgrade() else { return };
            let expand = b.label().as_deref() == Some("Show more");
            t.show_description(expand);
            if !expand {
                // Bring the top of the description back into view.
                if let Some(p) = t.desc.compute_point(&t.scroller, &gtk::graphene::Point::new(0.0, 0.0))
                    && p.y() < 0.0
                {
                    let adj = t.scroller.vadjustment();
                    adj.set_value(adj.value() + p.y() as f64 - 120.0);
                }
            }
        });
        let w = Rc::downgrade(self);
        self.subscribe_btn.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                t.toggle_subscribe();
            }
        });
        let w = Rc::downgrade(self);
        self.like_btn.connect_toggled(move |b| {
            if let Some(t) = w.upgrade() {
                t.rate(b.is_active(), true);
            }
        });
        let w = Rc::downgrade(self);
        self.dislike_btn.connect_toggled(move |b| {
            if let Some(t) = w.upgrade() {
                t.rate(b.is_active(), false);
            }
        });
        self.autoplay.connect_active_notify(|s| config::update(|c| c.autoplay = s.is_active()));
        let w = Rc::downgrade(self);
        self.related.connect_row_activated(move |_, row| {
            if let Some(t) = w.upgrade() {
                let v = t.related_items.borrow().get(row.index() as usize).cloned();
                if let Some(v) = v {
                    t.open(v, None, 0.0);
                }
            }
        });
        self.queue.on_changed.replace(Some(Box::new(|| super::ctx().watch.update_skip())));
        let w = Rc::downgrade(self);
        self.related_more.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                t.load_more_related();
            }
        });
        let w = Rc::downgrade(self);
        self.scroller.vadjustment().connect_value_changed(move |adj| {
            if let Some(t) = w.upgrade()
                && adj.value() + adj.page_size() > adj.upper() - 600.0
            {
                t.comments.load_more();
            }
        });
        // Coming back to this page after the miniplayer was closed restarts the video.
        let w = Rc::downgrade(self);
        self.placeholder.connect_map(move |_| {
            let Some(t) = w.upgrade() else { return };
            if t.opening.get() {
                return;
            }
            let v = t.video.borrow().clone();
            if let Some(v) = v {
                let p = super::ctx().player.clone();
                if p.current_id().as_deref() != Some(v.id.as_str()) {
                    p.load(v, 0.0);
                }
            }
        });

        // Share actions
        let group = gio::SimpleActionGroup::new();
        let w = Rc::downgrade(self);
        let copy = gio::SimpleAction::new("copy", None);
        copy.connect_activate(move |_, _| {
            if let Some(t) = w.upgrade()
                && let Some(v) = t.video.borrow().as_ref()
            {
                t.page.clipboard().set_text(&v.url());
                super::toast("Link copied");
            }
        });
        group.add_action(&copy);
        let w = Rc::downgrade(self);
        let copy_t = gio::SimpleAction::new("copy-time", None);
        copy_t.connect_activate(move |_, _| {
            if let Some(t) = w.upgrade()
                && let Some(v) = t.video.borrow().as_ref()
            {
                let pos = super::ctx().player.position();
                t.page.clipboard().set_text(&format!("https://youtu.be/{}?t={}", v.id, pos as u64));
                super::toast("Link copied");
            }
        });
        group.add_action(&copy_t);
        let w = Rc::downgrade(self);
        let browser = gio::SimpleAction::new("browser", None);
        browser.connect_activate(move |_, _| {
            if let Some(t) = w.upgrade()
                && let Some(v) = t.video.borrow().as_ref()
            {
                super::open_in_browser(&v.url());
            }
        });
        group.add_action(&browser);
        self.page.insert_action_group("watch", Some(&group));
    }

    /// Shows the full description, or (like youtube.com) only its first
    /// few lines with a "Show more" button.
    fn show_description(&self, expanded: bool) {
        let details = self.details.borrow();
        let Some(d) = details.as_ref() else { return };
        let full = d.description.trim_end();
        let cut = super::text::excerpt_end(full, 4, 280);
        let truncated = cut < full.len();
        self.desc_more.set_visible(truncated);
        self.desc_more.set_label(if expanded { "Show less" } else { "Show more" });
        if expanded || !truncated {
            self.desc.set_markup(&super::text::linkify_runs(&d.description, &d.description_links));
        } else {
            let links: Vec<_> = d.description_links.iter().filter(|(s, l, _)| s + l <= cut).cloned().collect();
            let mut markup = super::text::linkify_runs(full[..cut].trim_end(), &links);
            markup.push('…');
            self.desc.set_markup(&markup);
        }
    }

    /// Opens `v` (optionally as part of a playlist queue) and shows the page.
    pub fn open(self: &Rc<Self>, v: VideoItem, queue: Option<Queue>, start: f64) {
        self.generation.set(self.generation.get() + 1);
        self.video.replace(Some(v.clone()));
        self.details.replace(None);
        self.page.set_title(if v.title.is_empty() { "Watch" } else { &v.title });
        self.title.set_text(&v.title);
        self.channel_name.set_text(v.channel_name.as_deref().unwrap_or(""));
        self.channel_subs.set_text("");
        self.avatar.set_text(v.channel_name.as_deref());
        self.avatar.set_custom_image(None::<&gtk::gdk::Paintable>);
        images::avatar(&self.avatar, v.channel_avatar.as_deref());
        self.meta.set_text(&v.meta_line());
        self.desc.set_markup("");
        self.desc_more.set_visible(false);
        self.sb_info.set_visible(false);
        self.set_votes(None, None);
        self.updating.set(true);
        self.like_btn.set_active(false);
        self.dislike_btn.set_active(false);
        self.updating.set(false);
        self.subscribe_btn.set_visible(false);
        self.comments.clear();
        self.related.remove_all();
        self.related_items.borrow_mut().clear();
        self.related_more.set_visible(false);
        self.set_queue(queue);
        self.scroller.vadjustment().set_value(0.0);

        self.opening.set(true);
        self.show();
        self.opening.set(false);
        let player = super::ctx().player.clone();
        if player.current_id().as_deref() != Some(v.id.as_str()) {
            player.load(v.clone(), start);
        } else if start > 0.0 {
            player.seek_to(start);
        }
        self.load_details(v.id.clone());
        if config::settings().dislikes {
            self.load_votes(v.id.clone());
        }
    }

    /// Brings the watch page to the front of the navigation stack.
    pub fn show(&self) {
        let nav = super::ctx().nav.clone();
        if nav.visible_page().as_ref() == Some(&self.page) {
            return;
        }
        let stack = nav.navigation_stack();
        let in_stack = (0..stack.n_items()).any(|i| stack.item(i).and_downcast::<adw::NavigationPage>().as_ref() == Some(&self.page));
        if in_stack {
            nav.pop_to_page(&self.page);
        } else {
            nav.push(&self.page);
        }
    }

    fn set_queue(&self, queue: Option<Queue>) {
        self.queue.set(queue);
        self.update_skip();
    }

    /// Shows the player's previous / next buttons when they'd do something.
    pub fn update_skip(&self) {
        let prev = self.queue.has_previous();
        let next = self.queue.is_active() || !self.related_items.borrow().is_empty();
        super::ctx().player.set_skip(prev, next);
    }

    /// "Play next" / "Add to queue".
    pub fn enqueue(self: &Rc<Self>, v: VideoItem, next: bool) {
        let p = super::ctx().player.clone();
        let playing = self
            .video
            .borrow()
            .clone()
            .filter(|c| !p.is_music_owned() && p.current_id().as_deref() == Some(c.id.as_str()));
        if playing.is_none() && !self.queue.is_active() {
            // Nothing to queue after: just play it.
            return self.open(v, None, 0.0);
        }
        let title = v.title.clone();
        self.queue.add(v, next, playing);
        self.update_skip();
        super::toast(&if next { format!("Playing next: {title}") } else { format!("Added to queue: {title}") });
    }

    /// The next video of the queue, or (like the player's Next button on
    /// youtube.com) the first related one.
    pub fn next(self: &Rc<Self>) {
        self.advance(true);
    }

    pub fn previous(self: &Rc<Self>) {
        let p = super::ctx().player.clone();
        if p.position() > 3.0 {
            return p.seek_to(0.0);
        }
        match self.queue.previous() {
            Some(q) => {
                let v = q.items[q.index].clone();
                self.open_keep_view(v, Some(q));
            }
            None => p.seek_to(0.0),
        }
    }

    fn load_details(self: &Rc<Self>, id: String) {
        let generation = self.generation.get();
        let opts = super::feed::opts();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move {
                let v = api::client().next(&id).await?;
                Ok(parse::parse_next(&v, opts))
            })
            .await;
            let Some(t) = w.upgrade() else { return };
            if t.generation.get() != generation {
                return;
            }
            match res {
                Ok(d) => t.apply_details(d),
                Err(e) => super::toast(&format!("Could not load video details: {e}")),
            }
        });
    }

    fn apply_details(self: &Rc<Self>, d: VideoDetails) {
        if !d.title.is_empty() {
            self.title.set_text(&d.title);
            self.page.set_title(&d.title);
            if let Some(v) = self.video.borrow_mut().as_mut() {
                v.title = d.title.clone();
            }
            super::ctx().player.set_title(&d.id, &d.title);
        }
        self.channel_name.set_text(&d.channel.title);
        self.channel_subs.set_text(d.channel.subscribers.as_deref().unwrap_or(""));
        self.avatar.set_text(Some(&d.channel.title));
        images::avatar(&self.avatar, d.channel.avatar.as_deref());
        let meta = [d.views.as_deref(), d.date.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" • ");
        self.meta.set_text(&meta);
        self.details.replace(Some(d.clone()));
        self.show_description(false);
        self.subscribed.set(d.subscribed.unwrap_or(false));
        self.update_subscribe();
        self.subscribe_btn.set_visible(!d.channel.id.is_empty());
        if self.like_label.text() == "–"
            && let Some(l) = &d.likes
        {
            self.like_label.set_text(l);
        }
        self.updating.set(true);
        self.like_btn.set_active(d.like_status.as_deref() == Some("LIKE"));
        self.dislike_btn.set_active(d.like_status.as_deref() == Some("DISLIKE"));
        self.updating.set(false);

        // Fill in channel info for the player's video item (used by downloads etc.)
        if let Some(v) = self.video.borrow_mut().as_mut() {
            if v.channel_id.is_none() && !d.channel.id.is_empty() {
                v.channel_id = Some(d.channel.id.clone());
            }
            if v.channel_name.is_none() {
                v.channel_name = Some(d.channel.title.clone());
            }
            v.is_live = v.is_live || d.is_live;
        }

        self.append_related(&d.related.items);
        self.related_token.replace(d.related.continuation.clone());
        self.related_more.set_visible(d.related.continuation.is_some());
        self.comments.reset(d.comments_token.clone(), d.comment_sorts.clone(), d.comments_count.clone());
    }

    fn append_related(&self, items: &[Item]) {
        for it in items {
            if let Item::Video(v) = it {
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&cards::video_card(v, Layout::List)));
                self.related.append(&row);
                self.related_items.borrow_mut().push(v.clone());
            }
        }
        self.update_skip();
    }

    fn load_more_related(self: &Rc<Self>) {
        let Some(token) = self.related_token.take() else { return };
        self.related_more.set_sensitive(false);
        let generation = self.generation.get();
        let opts = super::feed::opts();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move {
                let v = api::client().next_continuation(&token).await?;
                Ok(parse::parse_feed(&v, opts))
            })
            .await;
            let Some(t) = w.upgrade() else { return };
            if t.generation.get() != generation {
                return;
            }
            t.related_more.set_sensitive(true);
            match res {
                Ok(feed) => {
                    t.append_related(&feed.items);
                    t.related_more.set_visible(feed.continuation.is_some());
                    t.related_token.replace(feed.continuation);
                }
                Err(e) => super::toast(&format!("Could not load more: {e}")),
            }
        });
    }

    fn load_votes(self: &Rc<Self>, id: String) {
        let generation = self.generation.get();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { extras::dislikes(api::client().http(), &id).await }).await;
            let Some(t) = w.upgrade() else { return };
            if t.generation.get() != generation {
                return;
            }
            if let Ok(v) = res {
                t.set_votes(v.likes, v.dislikes);
            }
        });
    }

    fn set_votes(&self, likes: Option<i64>, dislikes: Option<i64>) {
        match likes {
            Some(l) => self.like_label.set_text(&extras::format_count(l)),
            None => self.like_label.set_text("–"),
        }
        match dislikes {
            Some(d) => self.dislike_label.set_text(&extras::format_count(d)),
            None => self.dislike_label.set_text(if config::settings().dislikes { "–" } else { "" }),
        }
        if let (Some(l), Some(d)) = (likes, dislikes)
            && l + d > 0
        {
            self.ratio.set_value(l as f64 / (l + d) as f64);
            self.ratio.set_tooltip_text(Some(&format!("{:.1}% liked (Return YouTube Dislike)", 100.0 * l as f64 / (l + d) as f64)));
            self.ratio.set_visible(true);
        } else {
            self.ratio.set_visible(false);
        }
    }

    /// Shows what SponsorBlock found for the current video.
    pub fn set_segments(&self, segs: &[extras::Segment]) {
        if segs.is_empty() {
            self.sb_info.set_visible(false);
            return;
        }
        if let Some(full) = segs.iter().find(|s| s.action == "full") {
            self.sb_info.set_text(&format!("SponsorBlock: the whole video is labeled “{}”", extras::sb_label(&full.category)));
        } else {
            let mut counts: Vec<(String, usize)> = Vec::new();
            for s in segs.iter().filter(|s| s.action != "poi") {
                let l = extras::sb_label(&s.category).to_string();
                match counts.iter_mut().find(|(n, _)| *n == l) {
                    Some(c) => c.1 += 1,
                    None => counts.push((l, 1)),
                }
            }
            if counts.is_empty() {
                self.sb_info.set_text("SponsorBlock: highlight available");
            } else {
                let parts: Vec<String> = counts.iter().map(|(l, n)| if *n > 1 { format!("{l} ×{n}") } else { l.clone() }).collect();
                self.sb_info.set_text(&format!("SponsorBlock: {}", parts.join(", ")));
            }
        }
        self.sb_info.set_visible(true);
    }

    fn update_subscribe(&self) {
        let b = &self.subscribe_btn;
        if self.subscribed.get() {
            b.set_label("Subscribed");
            b.remove_css_class("suggested-action");
        } else {
            b.set_label("Subscribe");
            b.add_css_class("suggested-action");
        }
    }

    fn toggle_subscribe(self: &Rc<Self>) {
        if !api::client().is_logged_in() {
            super::login::show();
            return;
        }
        let Some(id) = self.details.borrow().as_ref().map(|d| d.channel.id.clone()) else { return };
        let sub = !self.subscribed.get();
        self.subscribed.set(sub);
        self.update_subscribe();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { api::client().subscribe(&id, sub).await }).await;
            if let Err(e) = res {
                super::toast(&format!("Could not update subscription: {e}"));
                if let Some(t) = w.upgrade() {
                    t.subscribed.set(!sub);
                    t.update_subscribe();
                }
            } else {
                super::toast(if sub { "Subscribed" } else { "Unsubscribed" });
            }
        });
    }

    fn rate(self: &Rc<Self>, active: bool, like: bool) {
        if self.updating.get() {
            return;
        }
        if !api::client().is_logged_in() {
            self.updating.set(true);
            if like {
                self.like_btn.set_active(false);
            } else {
                self.dislike_btn.set_active(false);
            }
            self.updating.set(false);
            super::toast("Sign in to rate videos");
            return;
        }
        self.updating.set(true);
        if active {
            if like {
                self.dislike_btn.set_active(false);
            } else {
                self.like_btn.set_active(false);
            }
        }
        self.updating.set(false);
        let Some(id) = self.video.borrow().as_ref().map(|v| v.id.clone()) else { return };
        let action = match (active, like) {
            (true, true) => LikeAction::Like,
            (true, false) => LikeAction::Dislike,
            _ => LikeAction::Remove,
        };
        runtime::spawn_local(async move {
            if let Err(e) = runtime::run(async move { api::client().like(&id, action).await }).await {
                super::toast(&format!("Could not save rating: {e}"));
            }
        });
    }

    /// Called when playback reaches the end.
    pub fn play_next(self: &Rc<Self>) {
        self.advance(false);
    }

    fn advance(self: &Rc<Self>, user: bool) {
        if self.queue.is_active() {
            let w = Rc::downgrade(self);
            self.queue.next(move |q| {
                let Some(t) = w.upgrade() else { return };
                match q {
                    Some(q) => {
                        let v = q.items[q.index].clone();
                        if !user {
                            super::toast(&format!("Up next: {}", v.title));
                        }
                        t.open_keep_view(v, Some(q));
                    }
                    // The queue is over: carry on like a single video.
                    None => t.play_related(user),
                }
            });
            return;
        }
        self.play_related(user);
    }

    fn play_related(self: &Rc<Self>, user: bool) {
        if !user && !config::settings().autoplay {
            return;
        }
        let next = self.related_items.borrow().iter().find(|v| !v.is_live).cloned();
        if let Some(v) = next {
            if !user {
                super::toast(&format!("Up next: {}", v.title));
            }
            self.open_keep_view(v, None);
        }
    }

    /// Like `open`, but doesn't pull the page to the front when the user is
    /// browsing elsewhere with the miniplayer.
    fn open_keep_view(self: &Rc<Self>, v: VideoItem, q: Option<Queue>) {
        let player = super::ctx().player.clone();
        if player.mode() == super::player::Mode::Mini {
            player.load(v.clone(), 0.0);
            self.generation.set(self.generation.get() + 1);
            self.video.replace(Some(v.clone()));
            self.page.set_title(if v.title.is_empty() { "Watch" } else { &v.title });
            self.title.set_text(&v.title);
            self.comments.clear();
            self.related.remove_all();
            self.related_items.borrow_mut().clear();
            self.set_queue(q);
            self.load_details(v.id.clone());
            if config::settings().dislikes {
                self.load_votes(v.id);
            }
        } else {
            self.open(v, q, 0.0);
        }
    }
}
