//! Channel pages: banner, avatar, stats, subscribe button and tabs
//! (Videos, Live, Playlists, …). The Shorts tab is never shown.

use super::cards::Layout;
use super::feed::{FeedView, Source};
use super::widgets::AspectBin;
use crate::api::{self, models::*, parse};
use crate::{images, runtime};
use adw::prelude::*;
use gtk::glib;
use base64::Engine;
use std::cell::Cell;
use std::rc::Rc;

const VIDEOS_PARAMS: &str = "EgZ2aWRlb3PyBgQKAjoA";

/// Which tab a `browseEndpoint.params` blob refers to (it's a protobuf whose
/// field 2 is the tab's internal name).
fn tab_kind(params: &str) -> String {
    let raw = urlencoding::decode(params).map(|s| s.into_owned()).unwrap_or_default();
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(raw.replace('+', "-").replace('/', "_"))
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(&raw))
        .unwrap_or_default();
    if bytes.len() > 2 && bytes[0] == 0x12 {
        let n = bytes[1] as usize;
        if let Some(name) = bytes.get(2..2 + n) {
            return String::from_utf8_lossy(name).into_owned();
        }
    }
    String::new()
}

const SHOWN_TABS: &[&str] = &["videos", "streams", "playlists", "podcasts", "releases"];

pub fn page(channel_id: &str) -> adw::NavigationPage {
    let feed = FeedView::new(Source::Empty, Layout::Grid);
    feed.set_empty_state("camera-video-symbolic", "No videos", "This channel has nothing to show here.");
    let page = adw::NavigationPage::builder().title("Channel").child(&feed.widget).build();

    // Header
    let banner_pic = gtk::Picture::new();
    banner_pic.set_content_fit(gtk::ContentFit::Cover);
    banner_pic.set_can_shrink(true);
    let banner = AspectBin::new(6.2);
    banner.set_child(Some(&banner_pic));
    banner.add_css_class("banner");
    banner.set_visible(false);

    let avatar = adw::Avatar::new(120, None, true);
    let title = gtk::Label::new(None);
    title.add_css_class("title-1");
    title.set_xalign(0.0);
    title.set_wrap(true);
    let meta = gtk::Label::new(None);
    meta.add_css_class("dim-label");
    meta.set_xalign(0.0);
    meta.set_wrap(true);
    let desc = gtk::Label::new(None);
    desc.add_css_class("dim-label");
    desc.set_xalign(0.0);
    desc.set_wrap(true);
    desc.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    desc.set_max_width_chars(90);
    desc.set_selectable(true);
    let full_desc = Rc::new(std::cell::RefCell::new(String::new()));
    let subscribe = gtk::Button::with_label("Subscribe");
    subscribe.add_css_class("pill");
    subscribe.add_css_class("suggested-action");
    subscribe.set_halign(gtk::Align::Start);
    subscribe.set_visible(false);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 6);
    text.set_valign(gtk::Align::Center);
    avatar.set_valign(gtk::Align::Start);
    text.append(&title);
    text.append(&meta);
    text.append(&desc);
    text.append(&subscribe);
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    head.append(&avatar);
    head.append(&text);
    let tabs = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    tabs.add_css_class("channel-tabs");
    feed.header.append(&banner);
    feed.header.append(&head);
    feed.header.append(&tabs);

    // The description shows its first line with a "more" link, like youtube.com.
    let show_desc = {
        let (d, full) = (desc.clone(), full_desc.clone());
        move |expanded: bool| {
            let full = full.borrow();
            let cut = super::text::excerpt_end(&full, 1, 140);
            if expanded || cut >= full.trim_end().len() {
                let less = if cut < full.trim_end().len() { " <a href=\"gtube:less\">less</a>" } else { "" };
                d.set_markup(&format!("{}{less}", super::text::linkify(&full)));
            } else {
                d.set_markup(&format!("{}… <a href=\"gtube:more\">more</a>", super::text::linkify(full[..cut].trim_end())));
            }
        }
    };
    let show_desc = Rc::new(show_desc);
    {
        let s = show_desc.clone();
        desc.connect_activate_link(move |_, uri| match uri {
            "gtube:more" => {
                s(true);
                glib::Propagation::Stop
            }
            "gtube:less" => {
                s(false);
                glib::Propagation::Stop
            }
            _ if super::text::handle_link(uri) => glib::Propagation::Stop,
            _ => glib::Propagation::Proceed,
        });
    }

    let subscribed = Rc::new(Cell::new(false));
    let update_sub = {
        let (b, s) = (subscribe.clone(), subscribed.clone());
        move || {
            if s.get() {
                b.set_label("Subscribed");
                b.remove_css_class("suggested-action");
            } else {
                b.set_label("Subscribe");
                b.add_css_class("suggested-action");
            }
        }
    };
    let update_sub = Rc::new(update_sub);
    {
        let (id, s, u) = (channel_id.to_string(), subscribed.clone(), update_sub.clone());
        subscribe.connect_clicked(move |_| {
            if !api::client().is_logged_in() {
                super::login::show();
                return;
            }
            let sub = !s.get();
            s.set(sub);
            u();
            let (id, s, u) = (id.clone(), s.clone(), u.clone());
            runtime::spawn_local(async move {
                if let Err(e) = runtime::run(async move { api::client().subscribe(&id, sub).await }).await {
                    super::toast(&format!("Could not update subscription: {e}"));
                    s.set(!sub);
                    u();
                }
            });
        });
    }

    let id = channel_id.to_string();
    let opts = super::feed::opts();
    let feed_c = feed.clone();
    let page_c = page.clone();
    runtime::spawn_local(async move {
        let fid = id.clone();
        let res = runtime::run(async move {
            let v = api::client().browse(&fid, Some(VIDEOS_PARAMS)).await?;
            Ok((parse::parse_channel(&v), parse::parse_feed(&v, opts)))
        })
        .await;
        let (info, first) = match res {
            Ok(r) => r,
            Err(e) => {
                super::toast(&format!("Could not load channel: {e}"));
                return;
            }
        };
        let ChannelPage { info: ch, banner: banner_url, video_count, tabs: tab_list, subscribed: sub } = info;
        page_c.set_title(&ch.title);
        title.set_text(&ch.title);
        avatar.set_text(Some(&ch.title));
        images::avatar(&avatar, ch.avatar.as_deref());
        let m = [ch.handle.as_deref(), ch.subscribers.as_deref(), video_count.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" • ");
        meta.set_text(&m);
        full_desc.replace(ch.description.clone().unwrap_or_default());
        show_desc(false);
        desc.set_visible(ch.description.as_deref().is_some_and(|d| !d.is_empty()));
        if let Some(b) = banner_url {
            images::picture(&banner_pic, &b);
            banner.set_visible(true);
        }
        subscribed.set(sub.unwrap_or(false));
        update_sub();
        subscribe.set_visible(true);

        let mut group: Option<gtk::ToggleButton> = None;
        let mut selected_any = false;
        for (name, params) in tab_list {
            let kind = tab_kind(&params);
            if !SHOWN_TABS.contains(&kind.as_str()) {
                continue;
            }
            let b = gtk::ToggleButton::with_label(&name);
            b.add_css_class("chip");
            if let Some(g) = &group {
                b.set_group(Some(g));
            } else {
                group = Some(b.clone());
            }
            if kind == "videos" {
                b.set_active(true);
                selected_any = true;
            }
            let (f, cid) = (Rc::downgrade(&feed_c), id.clone());
            b.connect_toggled(move |b| {
                if b.is_active()
                    && let Some(f) = f.upgrade()
                {
                    f.set_source(Source::Browse { id: cid.clone(), params: Some(params.clone()) });
                }
            });
            tabs.append(&b);
        }
        if !selected_any && let Some(g) = &group {
            g.set_active(true);
        }
        feed_c.set_feed(Source::Browse { id: id.clone(), params: Some(VIDEOS_PARAMS.into()) }, first);
    });
    super::keep_alive(&page, feed);
    page
}
