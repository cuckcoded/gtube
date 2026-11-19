//! Video / channel / playlist cards used by every feed.

use super::widgets::AspectBin;
use crate::api::models::*;
use crate::images;
use gtk::{gdk, gio, glib, prelude::*};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Grid,
    List,
}

fn label(text: &str, classes: &[&str], lines: i32) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    l.set_max_width_chars(28);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if lines > 1 {
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_lines(lines);
    }
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// Makes `w` open the channel on click without triggering the card itself.
fn link_channel(w: &impl IsA<gtk::Widget>, channel_id: Option<String>) {
    let Some(id) = channel_id else { return };
    w.as_ref().set_cursor_from_name(Some("pointer"));
    let g = gtk::GestureClick::new();
    g.set_button(1);
    g.connect_pressed(|g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
    });
    g.connect_released(move |_, _, _, _| super::open_channel(&id));
    w.as_ref().add_controller(g);
}

fn thumbnail(url: &str, badge: Option<&str>, live: bool, progress: Option<f64>, stacked: bool) -> gtk::Overlay {
    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    images::picture(&pic, url);
    let aspect = AspectBin::new(16.0 / 9.0);
    aspect.set_child(Some(&pic));
    aspect.add_css_class("thumb");
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&aspect));
    if stacked {
        overlay.add_css_class("stacked");
    }
    if let Some(b) = badge {
        let l = gtk::Label::new(Some(b));
        l.add_css_class("badge");
        if live {
            l.add_css_class("live");
        }
        l.set_halign(gtk::Align::End);
        l.set_valign(gtk::Align::End);
        l.set_margin_end(6);
        l.set_margin_bottom(6);
        overlay.add_overlay(&l);
    }
    if let Some(p) = progress.filter(|p| *p > 0.0) {
        let bar = gtk::ProgressBar::new();
        bar.set_fraction(p.min(1.0));
        bar.set_valign(gtk::Align::End);
        bar.add_css_class("watch-progress");
        overlay.add_overlay(&bar);
    }
    overlay
}

pub fn video_card(v: &VideoItem, layout: Layout) -> gtk::Widget {
    // YouTube's own progress (when signed in), else how far it got here.
    // What was just watched here is newer than what the feed says.
    let progress = crate::api::stats::reported_progress(&v.id).or(v.progress);
    let thumb = thumbnail(&v.thumbnail_url(), v.duration.as_deref(), v.is_live, progress, false);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(&label(&v.title, &["card-title"], 2));
    if let Some(ch) = &v.channel_name {
        let l = label(ch, &["dim-label", "card-sub"], 1);
        l.set_hexpand(false);
        l.set_halign(gtk::Align::Start);
        link_channel(&l, v.channel_id.clone());
        text.append(&l);
    }
    let meta = v.meta_line();
    if !meta.is_empty() {
        text.append(&label(&meta, &["dim-label", "card-sub"], 1));
    }

    let card = match layout {
        Layout::Grid => {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            if v.channel_avatar.is_some() {
                let av = adw::Avatar::new(36, v.channel_name.as_deref(), true);
                av.set_valign(gtk::Align::Start);
                images::avatar(&av, v.channel_avatar.as_deref());
                link_channel(&av, v.channel_id.clone());
                row.append(&av);
            }
            row.append(&text);
            let card = gtk::Box::new(gtk::Orientation::Vertical, 10);
            card.append(&thumb);
            card.append(&row);
            card.set_width_request(250);
            card
        }
        Layout::List => {
            thumb.set_size_request(168, -1);
            thumb.add_css_class("list-thumb");
            thumb.set_valign(gtk::Align::Start);
            let card = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            card.append(&thumb);
            card.append(&text);
            card
        }
    };
    card.add_css_class("video-card");
    attach_menu(&card, Item::Video(v.clone()));
    card.upcast()
}

pub fn channel_card(c: &ChannelItem, layout: Layout) -> gtk::Widget {
    let size = if layout == Layout::Grid { 112 } else { 64 };
    let av = adw::Avatar::new(size, Some(&c.title), true);
    images::avatar(&av, c.avatar.as_deref());
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let title = label(&c.title, &["card-title"], 1);
    text.append(&title);
    let sub = [c.handle.as_deref(), c.subscribers.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" • ");
    if !sub.is_empty() {
        text.append(&label(&sub, &["dim-label", "card-sub"], 1));
    }
    if let Some(d) = &c.description {
        text.append(&label(d, &["dim-label", "card-sub"], 2));
    }
    let card = if layout == Layout::Grid {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 12);
        b.set_margin_top(12);
        title.set_xalign(0.5);
        for w in [&text] {
            w.set_halign(gtk::Align::Center);
        }
        let mut child = text.first_child();
        while let Some(c) = child {
            if let Ok(l) = c.clone().downcast::<gtk::Label>() {
                l.set_xalign(0.5);
                l.set_justify(gtk::Justification::Center);
            }
            child = c.next_sibling();
        }
        b.append(&av);
        b.append(&text);
        b.set_width_request(250);
        b
    } else {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        b.append(&av);
        text.set_valign(gtk::Align::Center);
        b.append(&text);
        b
    };
    card.add_css_class("channel-card");
    attach_menu(&card, Item::Channel(c.clone()));
    card.upcast()
}

pub fn playlist_card(p: &PlaylistItem, layout: Layout) -> gtk::Widget {
    let url = p.thumbnail.clone().unwrap_or_default();
    let thumb = thumbnail(&url, p.video_count.as_deref(), false, None, true);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
    text.append(&label(&p.title, &["card-title"], 2));
    if let Some(ch) = &p.channel_name {
        let l = label(ch, &["dim-label", "card-sub"], 1);
        link_channel(&l, p.channel_id.clone());
        text.append(&l);
    }
    text.append(&label(if p.is_mix() { "Mix • endless" } else { "Playlist" }, &["dim-label", "card-sub"], 1));
    let card = if layout == Layout::Grid {
        let b = gtk::Box::new(gtk::Orientation::Vertical, 10);
        b.append(&thumb);
        b.append(&text);
        b.set_width_request(250);
        b
    } else {
        thumb.set_size_request(168, -1);
        thumb.add_css_class("list-thumb");
        thumb.set_valign(gtk::Align::Start);
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        b.append(&thumb);
        b.append(&text);
        b
    };
    card.add_css_class("video-card");
    attach_menu(&card, Item::Playlist(p.clone()));
    card.upcast()
}

pub fn item_widget(item: &Item, layout: Layout) -> gtk::Widget {
    match item {
        Item::Video(v) => video_card(v, layout),
        Item::Channel(c) => channel_card(c, layout),
        Item::Playlist(p) => playlist_card(p, layout),
    }
}

pub fn activate(item: &Item) {
    match item {
        Item::Video(v) => super::open_video(v.clone()),
        Item::Channel(c) => super::open_channel(&c.id),
        Item::Playlist(p) if p.is_mix() => super::queue::open_mix(p.video_id.clone(), p.id.clone(), 0.0),
        Item::Playlist(p) => super::open_playlist(&p.id),
    }
}

fn item_url(item: &Item) -> String {
    match item {
        Item::Video(v) => v.url(),
        Item::Channel(c) => format!("https://www.youtube.com/channel/{}", c.id),
        Item::Playlist(p) => match &p.video_id {
            Some(v) if p.is_mix() => format!("https://www.youtube.com/watch?v={v}&list={}", p.id),
            _ => format!("https://www.youtube.com/playlist?list={}", p.id),
        },
    }
}

/// Right-click (or long-press) menu with the usual per-item actions.
fn attach_menu(card: &gtk::Box, item: Item) {
    let group = gio::SimpleActionGroup::new();
    let add = |name: &str, f: Box<dyn Fn()>| {
        let a = gio::SimpleAction::new(name, None);
        a.connect_activate(move |_, _| f());
        group.add_action(&a);
    };
    let menu = gio::Menu::new();
    let it = item.clone();
    add("open", Box::new(move || activate(&it)));
    menu.append(Some("Open"), Some("card.open"));
    match &item {
        Item::Video(v) if !v.is_short => {
            let queue = gio::Menu::new();
            let vv = v.clone();
            add("play-next", Box::new(move || super::ctx().watch.enqueue(vv.clone(), true)));
            queue.append(Some("Play next"), Some("card.play-next"));
            let vv = v.clone();
            add("add-queue", Box::new(move || super::ctx().watch.enqueue(vv.clone(), false)));
            queue.append(Some("Add to queue"), Some("card.add-queue"));
            let id = v.id.clone();
            add("mix", Box::new(move || super::queue::open_mix(Some(id.clone()), format!("RD{id}"), 0.0)));
            queue.append(Some("Start mix"), Some("card.mix"));
            menu.append_section(None, &queue);
            let more = gio::Menu::new();
            let vv = v.clone();
            add("audio", Box::new(move || {
                crate::config::update(|s| s.audio_only = true);
                super::open_video(vv.clone());
            }));
            more.append(Some("Play as audio only"), Some("card.audio"));
            let vv = v.clone();
            add("download", Box::new(move || super::downloads::show_dialog(&vv)));
            more.append(Some("Download…"), Some("card.download"));
            if crate::api::client().is_logged_in() {
                let id = v.id.clone();
                add("save", Box::new(move || super::playlist::save_dialog(&id)));
                more.append(Some("Save to playlist…"), Some("card.save"));
            }
            if let Some(cid) = v.channel_id.clone() {
                add("channel", Box::new(move || super::open_channel(&cid)));
                more.append(Some("Go to channel"), Some("card.channel"));
            }
            menu.append_section(None, &more);
        }
        Item::Video(v) => {
            let vv = v.clone();
            add("download", Box::new(move || super::downloads::show_dialog(&vv)));
            menu.append(Some("Download…"), Some("card.download"));
        }
        Item::Playlist(p) if p.is_mix() => {}
        Item::Playlist(p) => {
            let id = p.id.clone();
            add("shuffle", Box::new(move || super::queue::open_playlist(None, id.clone(), 0.0, true)));
            menu.append(Some("Shuffle play"), Some("card.shuffle"));
            let id = p.id.clone();
            add("play", Box::new(move || super::queue::open_playlist(None, id.clone(), 0.0, false)));
            menu.append(Some("Play all"), Some("card.play"));
            let pp = p.clone();
            add("download", Box::new(move || super::downloads::download_playlist(&pp)));
            menu.append(Some("Download playlist"), Some("card.download"));
        }
        Item::Channel(_) => {}
    }
    let section = gio::Menu::new();
    let url = item_url(&item);
    let u = url.clone();
    let weak = card.downgrade();
    add("copy", Box::new(move || {
        if let Some(c) = weak.upgrade() {
            c.clipboard().set_text(&u);
            super::toast("Link copied");
        }
    }));
    section.append(Some("Copy link"), Some("card.copy"));
    add("browser", Box::new(move || super::open_in_browser(&url)));
    section.append(Some("Open in browser"), Some("card.browser"));
    menu.append_section(None, &section);
    card.insert_action_group("card", Some(&group));

    let popup = {
        let card = card.downgrade();
        move |x: f64, y: f64| {
            let Some(card) = card.upgrade() else { return };
            let pop = gtk::PopoverMenu::from_model(Some(&menu));
            pop.set_parent(&card);
            pop.set_has_arrow(false);
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.connect_closed(|p| {
                let p = p.clone();
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
        }
    };
    let click = gtk::GestureClick::new();
    click.set_button(3);
    let p = popup.clone();
    click.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        p(x, y);
    });
    card.add_controller(click);
    let long = gtk::GestureLongPress::new();
    long.set_touch_only(true);
    long.connect_pressed(move |_, x, y| popup(x, y));
    card.add_controller(long);
}
