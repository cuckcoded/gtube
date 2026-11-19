//! Playlist pages (also used for Watch later and Liked videos).

use super::cards::Layout;
use super::feed::{FeedView, Source};
use super::widgets::AspectBin;
use crate::api::{self, PlaylistEdit, models::*, parse};
use crate::{images, runtime};
use adw::prelude::*;
use std::cell::{Cell, RefCell};
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
    let edit = gtk::Button::with_label("Edit");
    edit.add_css_class("pill");
    edit.set_visible(false);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.append(&play);
    buttons.append(&shuffle);
    buttons.append(&download);
    buttons.append(&edit);
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
    {
        let info = info.clone();
        let (pg, t, d) = (page.downgrade(), title.downgrade(), desc.downgrade());
        edit.connect_clicked(move |_| {
            let (pg, t, d) = (pg.clone(), t.clone(), d.clone());
            edit_dialog(&info, move |p| {
                if let (Some(pg), Some(t), Some(d)) = (pg.upgrade(), t.upgrade(), d.upgrade()) {
                    pg.set_title(&p.title);
                    t.set_text(&p.title);
                    d.set_markup(&super::text::linkify(p.description.as_deref().unwrap_or("")));
                    d.set_visible(p.description.is_some());
                }
            });
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
        if p.editable {
            edit.set_visible(true);
            make_editable(&feed_c, &list_id);
        }
        let first = p.feed.clone();
        *info.borrow_mut() = p;
        feed_c.set_feed(Source::Browse { id: browse_id, params: None }, first);
    });
    super::keep_alive(&page, feed);
    page
}

/// Sends changes to a playlist; if YouTube refuses, says so and reloads
/// `feed` to show the playlist as it is.
fn send(list: &str, edits: Vec<PlaylistEdit>, feed: Option<&Rc<FeedView>>, done: impl FnOnce(bool) + 'static) {
    let (list, f) = (list.to_string(), feed.map(Rc::downgrade));
    runtime::spawn_local(async move {
        let res = runtime::run(async move { api::client().edit_playlist(&list, &edits).await }).await;
        if let Err(e) = &res {
            super::toast(&format!("Could not change the playlist: {e}"));
            if let Some(f) = f.and_then(|f| f.upgrade()) {
                f.reload();
            }
        }
        done(res.is_ok());
    });
}

/// Rows of the user's own playlist get a drag handle and a menu to move or
/// remove them.
fn make_editable(feed: &Rc<FeedView>, list: &str) {
    let (f, list) = (Rc::downgrade(feed), list.to_string());
    feed.connect_row(move |row, card| {
        let r = row.downgrade();
        let pos = Rc::new(move || r.upgrade().map_or(0, |r| r.index().max(0) as usize));
        let act = {
            let (f, list) = (f.clone(), list.clone());
            Rc::new(move |from: usize, to: Option<usize>| {
                if let Some(f) = f.upgrade() {
                    match to {
                        Some(to) => move_entry(&f, &list, from, to),
                        None => remove_entry(&f, &list, from),
                    }
                }
            })
        };
        let (p, a) = (pos.clone(), act.clone());
        let handle = super::widgets::reorder_handle(row, move || p(), move |from, to| a(from, Some(to)));
        let menu = super::music::menu_button(move |mb| {
            let (p, a) = (pos.clone(), act.clone());
            mb.item("Move up", move || {
                let i = p();
                if i > 0 {
                    a(i, Some(i - 1));
                }
            });
            let (p, a) = (pos.clone(), act.clone());
            mb.item("Move down", move || a(p(), Some(p() + 1)));
            mb.separator();
            let (p, a) = (pos.clone(), act.clone());
            mb.item("Remove from playlist", move || a(p(), None));
        });
        card.set_hexpand(true);
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        b.append(&handle);
        b.append(card);
        b.append(&menu);
        b.upcast()
    });
}

fn set_id(items: &[Item], i: usize) -> Option<String> {
    match items.get(i) {
        Some(Item::Video(v)) => v.set_video_id.clone(),
        _ => None,
    }
}

fn move_entry(feed: &Rc<FeedView>, list: &str, from: usize, to: usize) {
    let items = feed.items();
    // Without its set id YouTube can't move it: don't pretend to.
    if from == to || from.max(to) >= items.len() || set_id(&items, from).is_none() {
        return;
    }
    feed.move_item(from, to);
    let items = feed.items();
    let Some(moved) = set_id(&items, to) else { return };
    let edit = match (set_id(&items, to + 1), to.checked_sub(1).and_then(|i| set_id(&items, i))) {
        (Some(next), _) => PlaylistEdit::MoveBefore(moved, next),
        (None, Some(prev)) => PlaylistEdit::MoveAfter(moved, prev),
        _ => return,
    };
    send(list, vec![edit], Some(feed), |_| {});
}

fn remove_entry(feed: &Rc<FeedView>, list: &str, i: usize) {
    let Some(Item::Video(v)) = feed.items().get(i).cloned() else { return };
    feed.remove(i);
    send(list, vec![PlaylistEdit::Remove { video_id: v.id, set_id: v.set_video_id }], Some(feed), |_| {});
}

const PRIVACIES: [&str; 3] = ["PUBLIC", "UNLISTED", "PRIVATE"];

fn privacy_row() -> adw::ComboRow {
    adw::ComboRow::builder().title("Visibility").model(&gtk::StringList::new(&["Public", "Unlisted", "Private"])).build()
}

/// "New playlist", made with `video_id` in it.
fn create_dialog(video_id: String) {
    let d = adw::AlertDialog::new(Some("New playlist"), None);
    d.add_response("cancel", "Cancel");
    d.add_response("create", "Create");
    d.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("create"));
    d.set_close_response("cancel");
    d.set_response_enabled("create", false);
    let name = adw::EntryRow::builder().title("Name").build();
    let privacy = privacy_row();
    privacy.set_selected(2);
    let dd = d.downgrade();
    name.connect_changed(move |e| {
        if let Some(d) = dd.upgrade() {
            d.set_response_enabled("create", !e.text().trim().is_empty());
        }
    });
    let group = adw::PreferencesGroup::new();
    group.add(&name);
    group.add(&privacy);
    d.set_extra_child(Some(&group));
    d.connect_response(Some("create"), move |_, _| {
        let title = name.text().trim().to_string();
        let (t, p, v) = (title.clone(), PRIVACIES[privacy.selected() as usize], video_id.clone());
        runtime::spawn_local(async move {
            match runtime::run(async move { api::client().create_playlist(&t, p, &[v]).await }).await {
                Ok(_) => super::toast(&format!("Saved to {title}")),
                Err(e) => super::toast(&format!("Could not create the playlist: {e}")),
            }
        });
    });
    d.present(Some(&super::ctx().window));
}

/// Name, description and visibility of the user's playlist.
fn edit_dialog(info: &Rc<RefCell<PlaylistPage>>, done: impl Fn(&PlaylistPage) + 'static) {
    let (old_title, old_desc, old_privacy) = {
        let p = info.borrow();
        (p.title.clone(), p.description.clone().unwrap_or_default(), p.privacy.clone())
    };
    let d = adw::AlertDialog::new(Some("Edit playlist"), None);
    d.add_response("cancel", "Cancel");
    d.add_response("save", "Save");
    d.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("save"));
    d.set_close_response("cancel");

    let name = adw::EntryRow::builder().title("Name").text(&old_title).build();
    let privacies = PRIVACIES;
    let privacy = privacy_row();
    // No "Private"/"Unlisted" badge shown: public.
    let was = old_privacy.as_deref().and_then(|v| privacies.iter().position(|p| *p == v)).unwrap_or(0) as u32;
    privacy.set_selected(was);
    let group = adw::PreferencesGroup::new();
    group.add(&name);
    group.add(&privacy);
    let text = gtk::TextView::builder()
        .wrap_mode(gtk::WrapMode::WordChar)
        .accepts_tab(false)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(8)
        .right_margin(8)
        .build();
    text.update_property(&[gtk::accessible::Property::Label("Description")]);
    text.buffer().set_text(&old_desc);
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(96)
        .max_content_height(240)
        .propagate_natural_height(true)
        .child(&text)
        .build();
    scroll.add_css_class("card");
    let desc_group = adw::PreferencesGroup::builder().title("Description").build();
    desc_group.add(&scroll);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.append(&group);
    body.append(&desc_group);
    d.set_extra_child(Some(&body));

    let (info, done) = (info.clone(), Rc::new(done));
    d.connect_response(Some("save"), move |_, _| {
        let title = name.text().trim().to_string();
        let buf = text.buffer();
        let desc = buf.text(&buf.start_iter(), &buf.end_iter(), false).trim().to_string();
        let mut edits = Vec::new();
        if !title.is_empty() && title != old_title {
            edits.push(PlaylistEdit::Name(title.clone()));
        }
        if desc != old_desc {
            edits.push(PlaylistEdit::Description(desc.clone()));
        }
        let new_privacy = (privacy.selected() != was).then(|| privacies[privacy.selected() as usize].to_string());
        if let Some(p) = &new_privacy {
            edits.push(PlaylistEdit::Privacy(p.clone()));
        }
        if edits.is_empty() {
            return;
        }
        let list = info.borrow().id.trim_start_matches("VL").to_string();
        let (info, done) = (info.clone(), done.clone());
        send(&list, edits, None, move |ok| {
            if !ok {
                return;
            }
            let mut p = info.borrow_mut();
            if !title.is_empty() {
                p.title = title;
            }
            p.description = Some(desc).filter(|d| !d.is_empty());
            if new_privacy.is_some() {
                p.privacy = new_privacy;
            }
            done(&p);
        });
    });
    d.present(Some(&super::ctx().window));
}

/// "Save to playlist": the user's playlists, ticked where the video is.
pub fn save_dialog(video_id: &str) {
    if !api::client().is_logged_in() {
        return super::login::show();
    }
    let id = video_id.to_string();
    runtime::spawn_local(async move {
        let vid = id.clone();
        let targets = match runtime::run(async move { api::client().save_targets(&vid).await }).await {
            Ok(t) => t,
            Err(e) => return super::toast(&format!("Could not load your playlists: {e}")),
        };
        let d = adw::AlertDialog::new(Some("Save to playlist"), None);
        d.add_response("new", "New playlist…");
        d.add_response("close", "Done");
        d.set_close_response("close");
        let vid = id.clone();
        d.connect_response(Some("new"), move |_, _| create_dialog(vid.clone()));
        let group = adw::PreferencesGroup::new();
        if targets.is_empty() {
            group.set_description(Some("You have no playlists yet"));
        }
        for t in targets {
            let check = gtk::CheckButton::builder().active(t.saved).valign(gtk::Align::Center).build();
            let row = adw::ActionRow::builder().title(&t.title).activatable_widget(&check).build();
            row.add_prefix(&check);
            group.add(&row);
            // Set while undoing a failed change, which isn't sent again.
            let quiet = Rc::new(Cell::new(false));
            let (vid, list) = (id.clone(), t.id);
            check.connect_toggled(move |c| {
                if quiet.get() {
                    return;
                }
                let on = c.is_active();
                let edit = if on { PlaylistEdit::Add(vid.clone()) } else { PlaylistEdit::Remove { video_id: vid.clone(), set_id: None } };
                let (c, quiet) = (c.downgrade(), quiet.clone());
                send(&list, vec![edit], None, move |ok| {
                    if let Some(c) = c.upgrade().filter(|_| !ok) {
                        quiet.set(true);
                        c.set_active(!on);
                        quiet.set(false);
                    }
                });
            });
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(360)
            .propagate_natural_height(true)
            .child(&group)
            .build();
        d.set_extra_child(Some(&scroll));
        d.present(Some(&super::ctx().window));
    });
}
