//! Playlist pages (also used for Watch later and Liked videos).

use super::cards::Layout;
use super::feed::{FeedView, Source};
use super::widgets::AspectBin;
use crate::api::{self, models::*, parse};
use crate::{images, runtime};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

pub fn videos_of(items: &[Item]) -> Vec<VideoItem> {
    items.iter().filter_map(|i| if let Item::Video(v) = i { Some(v.clone()) } else { None }).collect()
}

/// Opening a video from a list plays the list (all of it) from there.
pub fn play_from(feed: &FeedView, title: &str, list_id: Option<&str>, index: usize, shuffle: bool) {
    super::queue::play_list(title, list_id, &feed.items(), index, feed.continuation(), shuffle);
}

pub fn page(playlist_id: &str) -> adw::NavigationPage {
    let browse_id = if playlist_id.starts_with("VL") { playlist_id.to_string() } else { format!("VL{playlist_id}") };
    let feed = FeedView::new(Source::Empty, Layout::List);
    feed.set_empty_state("view-list-symbolic", "This playlist is empty", "");
    let page = adw::NavigationPage::builder().title("Playlist").child(&feed.widget).build();

    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    let thumb = AspectBin::new(16.0 / 9.0);
    thumb.set_child(Some(&pic));
    thumb.add_css_class("thumb");
    thumb.set_size_request(320, -1);
    thumb.set_valign(gtk::Align::Start);
    let title = gtk::Label::new(None);
    title.add_css_class("title-1");
    title.set_xalign(0.0);
    title.set_wrap(true);
    let owner = gtk::Label::new(None);
    owner.add_css_class("heading");
    owner.set_xalign(0.0);
    let stats = gtk::Label::new(None);
    stats.add_css_class("dim-label");
    stats.set_xalign(0.0);
    stats.set_wrap(true);
    let desc = super::text::label("");
    desc.add_css_class("dim-label");
    desc.set_lines(3);
    desc.set_ellipsize(gtk::pango::EllipsizeMode::End);
    let play = gtk::Button::with_label("Play all");
    play.add_css_class("pill");
    play.add_css_class("suggested-action");
    let shuffle = gtk::Button::new();
    shuffle.set_child(Some(&adw::ButtonContent::builder().icon_name("media-playlist-shuffle-symbolic").label("Shuffle").build()));
    shuffle.add_css_class("pill");
    let download = gtk::Button::with_label("Download");
    download.add_css_class("pill");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&play);
    buttons.append(&shuffle);
    buttons.append(&download);
    let text = gtk::Box::new(gtk::Orientation::Vertical, 8);
    text.set_hexpand(true);
    for w in [title.upcast_ref::<gtk::Widget>(), owner.upcast_ref(), stats.upcast_ref(), desc.upcast_ref(), buttons.upcast_ref()] {
        text.append(w);
    }
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    head.add_css_class("playlist-header");
    head.append(&thumb);
    head.append(&text);
    feed.header.append(&head);

    let info: Rc<RefCell<PlaylistPage>> = Rc::new(RefCell::new(PlaylistPage { id: playlist_id.into(), ..Default::default() }));
    let list_id = playlist_id.trim_start_matches("VL").to_string();
    {
        let (f, info, id) = (Rc::downgrade(&feed), info.clone(), list_id.clone());
        feed.connect_activate(move |i, _| {
            if let Some(f) = f.upgrade() {
                let t = info.borrow().title.clone();
                play_from(&f, &t, Some(&id), i, false);
            }
        });
    }
    for (button, shuffled) in [(&play, false), (&shuffle, true)] {
        let (f, info, id) = (Rc::downgrade(&feed), info.clone(), list_id.clone());
        button.connect_clicked(move |_| {
            if let Some(f) = f.upgrade()
                && !f.items().is_empty()
            {
                let t = info.borrow().title.clone();
                play_from(&f, &t, Some(&id), 0, shuffled);
            }
        });
    }
    {
        let info = info.clone();
        download.connect_clicked(move |_| {
            let i = info.borrow();
            super::downloads::download_playlist(&PlaylistItem { id: i.id.trim_start_matches("VL").to_string(), title: i.title.clone(), ..Default::default() });
        });
    }

    let opts = super::feed::opts();
    let (feed_c, page_c, id) = (feed.clone(), page.clone(), playlist_id.to_string());
    runtime::spawn_local(async move {
        let bid = browse_id.clone();
        let res = runtime::run(async move {
            let v = api::client().browse(&bid, None).await?;
            Ok(parse::parse_playlist(&v, &id, opts))
        })
        .await;
        let p = match res {
            Ok(p) => p,
            Err(e) => {
                super::toast(&format!("Could not load playlist: {e}"));
                return;
            }
        };
        page_c.set_title(&p.title);
        title.set_text(&p.title);
        owner.set_text(p.owner.as_deref().unwrap_or(""));
        owner.set_visible(p.owner.is_some());
        if let Some(oid) = p.owner_id.clone() {
            owner.set_cursor_from_name(Some("pointer"));
            let g = gtk::GestureClick::new();
            g.connect_released(move |_, _, _, _| super::open_channel(&oid));
            owner.add_controller(g);
        }
        stats.set_text(&p.stats.join(" • "));
        desc.set_markup(&super::text::linkify(p.description.as_deref().unwrap_or("")));
        desc.set_visible(p.description.is_some());
        let thumb_url = p.thumbnail.clone().or_else(|| {
            p.feed.items.iter().find_map(|i| if let Item::Video(v) = i { Some(v.thumbnail_url()) } else { None })
        });
        if let Some(u) = thumb_url {
            images::picture(&pic, &u);
        }
        let first = p.feed.clone();
        *info.borrow_mut() = p;
        feed_c.set_feed(Source::Browse { id: browse_id, params: None }, first);
    });
    super::keep_alive(&page, feed);
    page
}
