//! Building blocks of Music pages: track rows, tiles, shelves, headers and
//! context menus.

use super::{open, play, queue};
use crate::api::music::{self as m, Card, Entry, Header, HeaderKind, Link, Mood, Shape, Shelf, ShelfStyle, Target, TopResult, Track, VideoKind};
use crate::api::{self, models::PlaylistItem};
use crate::ui::widgets::AspectBin;
use crate::{images, runtime};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

/// Links inside labels: `gtube-music:<browse id>`.
const LINK: &str = "gtube-music:";

fn esc(s: &str) -> String {
    glib::markup_escape_text(s).to_string()
}

pub fn links_markup(links: &[Link]) -> String {
    links
        .iter()
        .map(|l| match &l.id {
            Some(id) if !l.text.trim().is_empty() => format!("<a href=\"{LINK}{}\">{}</a>", esc(id), esc(&l.text)),
            _ => esc(&l.text),
        })
        .collect()
}

fn artists_markup(t: &Track) -> String {
    t.artists.iter().map(|a| links_markup(std::slice::from_ref(a))).collect::<Vec<_>>().join(", ")
}

/// "Artist, Artist • Album • 1.2B plays"
pub fn track_byline(t: &Track, album: bool, info: bool) -> String {
    let mut parts = Vec::new();
    if t.kind == VideoKind::Episode {
        parts.push("Episode".to_string());
    }
    let artists = artists_markup(t);
    if !artists.is_empty() {
        parts.push(artists);
    }
    if album && let Some(a) = &t.album {
        parts.push(links_markup(std::slice::from_ref(a)));
    }
    if info && let Some(i) = &t.info {
        parts.push(esc(i));
    }
    parts.join(" • ")
}

pub fn label(text: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    for c in classes {
        l.add_css_class(c);
    }
    l
}

/// A label whose `<a>` links open Music pages.
pub fn link_label(markup: &str, classes: &[&str]) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(markup);
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.add_css_class("music-links");
    for c in classes {
        l.add_css_class(c);
    }
    l.connect_activate_link(|_, uri| match uri.strip_prefix(LINK) {
        Some(id) => {
            open(&Target::browse(id));
            glib::Propagation::Stop
        }
        None => glib::Propagation::Proceed,
    });
    l
}

/// Limits a label's natural width so the artwork decides a tile's width.
fn narrow(l: &gtk::Label, lines: i32) {
    l.set_max_width_chars(1);
    l.set_width_chars(1);
    if lines > 1 {
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_lines(lines);
    }
}

/// Artwork of `size` pixels (width), cropped to the shape.
pub fn art(url: Option<&str>, size: i32, shape: Shape) -> AspectBin {
    let ratio = if shape == Shape::Wide { 16.0 / 9.0 } else { 1.0 };
    let bin = AspectBin::new(ratio);
    bin.set_size_request(size, -1);
    bin.add_css_class("music-art");
    if shape == Shape::Round {
        bin.add_css_class("round");
    }
    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    if let Some(u) = url {
        images::picture(&pic, &m::image_url(u, (size * 2).clamp(120, 1200) as u32));
    }
    bin.set_child(Some(&pic));
    bin.set_valign(gtk::Align::Start);
    bin
}

fn explicit_badge() -> gtk::Label {
    let e = gtk::Label::new(Some("E"));
    e.add_css_class("explicit");
    e.set_tooltip_text(Some("Explicit"));
    e.set_valign(gtk::Align::Center);
    e
}

pub fn pill(icon: &str, text: &str, suggested: bool) -> gtk::Button {
    let content = adw::ButtonContent::builder().icon_name(icon).label(text).build();
    let b = gtk::Button::builder().child(&content).build();
    b.add_css_class("pill");
    b.add_css_class("music-pill");
    if suggested {
        b.add_css_class("suggested-action");
    }
    b
}

pub fn round_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.add_css_class("circular");
    b.set_valign(gtk::Align::Center);
    b
}

// ---------------------------------------------------------------------------
// Activation

pub fn activate_target(t: &Target) {
    match t {
        Target::Watch { .. } => play(t),
        _ => open(t),
    }
}

pub fn activate_entry(e: &Entry) {
    match e {
        Entry::Track(t) => play(&t.play_target()),
        Entry::Card(c) => {
            if let Some(t) = c.target.as_ref().or(c.play.as_ref()) {
                activate_target(t);
            }
        }
        Entry::Mood(md) => open(&md.target),
    }
}

/// A Music link for an item, for copying.
pub fn share_url(t: &Target) -> Option<String> {
    match t {
        Target::Browse { id, .. } if id.starts_with("VL") => Some(format!("https://music.youtube.com/playlist?list={}", &id[2..])),
        Target::Browse { id, .. } if id.starts_with("UC") => Some(format!("https://music.youtube.com/channel/{id}")),
        Target::Browse { id, .. } => Some(format!("https://music.youtube.com/browse/{id}")),
        Target::Watch { video_id: Some(v), .. } => Some(format!("https://music.youtube.com/watch?v={v}")),
        Target::Watch { playlist_id: Some(p), .. } => Some(format!("https://music.youtube.com/playlist?list={p}")),
        _ => None,
    }
}

pub fn copy_text(text: &str) {
    if let Some(d) = gdk::Display::default() {
        d.clipboard().set_text(text);
        crate::ui::toast("Link copied");
    }
}

// ---------------------------------------------------------------------------
// Menus

pub struct MenuBuilder {
    menu: gio::Menu,
    section: gio::Menu,
    group: gio::SimpleActionGroup,
    n: u32,
}

impl MenuBuilder {
    pub fn new() -> Self {
        Self { menu: gio::Menu::new(), section: gio::Menu::new(), group: gio::SimpleActionGroup::new(), n: 0 }
    }

    pub fn item(&mut self, label: &str, f: impl Fn() + 'static) {
        let name = format!("a{}", self.n);
        self.n += 1;
        let a = gio::SimpleAction::new(&name, None);
        a.connect_activate(move |_, _| f());
        self.group.add_action(&a);
        self.section.append(Some(label), Some(&format!("item.{name}")));
    }

    pub fn separator(&mut self) {
        if self.section.n_items() > 0 {
            self.menu.append_section(None, &self.section);
            self.section = gio::Menu::new();
        }
    }

    pub fn install(mut self, b: &gtk::MenuButton) {
        self.separator();
        b.insert_action_group("item", Some(&self.group));
        b.set_menu_model(Some(&self.menu));
    }
}

/// A "⋮" button whose menu is built when first opened.
pub fn menu_button(build: impl Fn(&mut MenuBuilder) + 'static) -> gtk::MenuButton {
    let b = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("More").valign(gtk::Align::Center).build();
    b.add_css_class("flat");
    b.add_css_class("circular");
    b.set_create_popup_func(move |b| {
        if b.menu_model().is_none() {
            let mut mb = MenuBuilder::new();
            build(&mut mb);
            mb.install(b);
        }
    });
    b
}

/// Opens `menu`'s popup on right click / long press on `w`.
pub fn context_menu(w: &impl IsA<gtk::Widget>, menu: &gtk::MenuButton) {
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let m = menu.downgrade();
    click.connect_pressed(move |g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(m) = m.upgrade() {
            m.popup();
        }
    });
    w.as_ref().add_controller(click);
    let long = gtk::GestureLongPress::new();
    let m = menu.downgrade();
    long.connect_pressed(move |g, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(m) = m.upgrade() {
            m.popup();
        }
    });
    w.as_ref().add_controller(long);
}

/// Opens a menu built by `build` at the pointer on right click / long press
/// (for widgets without a "⋮" button, like tiles).
pub fn popup_menu(w: &impl IsA<gtk::Widget>, build: impl Fn(&mut MenuBuilder) + 'static) {
    let build = Rc::new(build);
    let show = {
        let build = build.clone();
        move |w: &gtk::Widget, x: f64, y: f64| {
            let mut mb = MenuBuilder::new();
            build(&mut mb);
            mb.separator();
            if mb.menu.n_items() == 0 {
                return;
            }
            w.insert_action_group("item", Some(&mb.group));
            let pop = gtk::PopoverMenu::from_model(Some(&mb.menu));
            pop.set_parent(w);
            pop.set_has_arrow(false);
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            pop.connect_closed(|p| {
                let p = p.clone();
                // Let the activated item run before the popover goes away.
                glib::idle_add_local_once(move || p.unparent());
            });
            pop.popup();
        }
    };
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_SECONDARY);
    let s = show.clone();
    click.connect_pressed(move |g, _, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(w) = g.widget() {
            s(&w, x, y);
        }
    });
    w.as_ref().add_controller(click);
    let long = gtk::GestureLongPress::new();
    long.connect_pressed(move |g, x, y| {
        g.set_state(gtk::EventSequenceState::Claimed);
        if let Some(w) = g.widget() {
            show(&w, x, y);
        }
    });
    w.as_ref().add_controller(long);
}

pub fn track_menu(mb: &mut MenuBuilder, t: &Track) {
    let tr = t.clone();
    mb.item("Start radio", move || play(&tr.radio_target()));
    mb.separator();
    let tr = t.clone();
    mb.item("Play next", move || queue().add_next(vec![tr.clone()]));
    let tr = t.clone();
    mb.item("Add to queue", move || queue().add_last(vec![tr.clone()]));
    mb.separator();
    if let Some(id) = t.album.as_ref().and_then(|a| a.id.clone()) {
        mb.item("Go to album", move || open(&Target::browse(&id)));
    }
    if let Some(id) = t.first_artist_id().map(str::to_string) {
        mb.item("Go to artist", move || open(&Target::browse(&id)));
    }
    mb.separator();
    let tr = t.clone();
    mb.item("Download…", move || crate::ui::downloads::show_dialog(&tr.video_item()));
    let url = t.url();
    mb.item("Copy link", move || copy_text(&url));
}

/// The list an album/playlist card plays (its play button's target).
fn card_list(c: &Card) -> Option<String> {
    match c.play.as_ref()? {
        Target::Watch { playlist_id: Some(p), .. } => Some(p.clone()),
        _ => None,
    }
}

pub fn card_menu(mb: &mut MenuBuilder, c: &Card) {
    if let Some(p) = c.play.clone() {
        let t = p.clone();
        mb.item("Play next", move || queue().add_target(t.clone(), true));
        mb.item("Add to queue", move || queue().add_target(p.clone(), false));
    }
    if let Some(list) = card_list(c).filter(|l| !l.starts_with("RD")) {
        mb.item("Start radio", move || {
            play(&Target::Watch { video_id: None, playlist_id: Some(format!("RDAMPL{list}")), params: Some("wAEB".into()), index: None })
        });
    }
    mb.separator();
    let own = match &c.target {
        Some(Target::Browse { id, .. }) => Some(id.clone()),
        _ => None,
    };
    if let Some(id) = c.subtitle.iter().filter_map(|l| l.id.clone()).find(|i| i.starts_with("UC") && Some(i) != own.as_ref()) {
        mb.item("Go to artist", move || open(&Target::browse(&id)));
    }
    if let Some(url) = c.target.as_ref().and_then(share_url) {
        mb.item("Copy link", move || copy_text(&url));
    }
}

// ---------------------------------------------------------------------------
// Rows

fn row_box(lead: &impl IsA<gtk::Widget>, title: &gtk::Widget, sub: Option<&gtk::Label>) -> gtk::Box {
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(title);
    if let Some(s) = sub {
        texts.append(s);
    }
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.append(lead);
    b.append(&texts);
    b
}

pub fn track_title(t: &Track) -> gtk::Widget {
    let title = label(&t.title, &["track-title"]);
    if !t.explicit {
        return title.upcast();
    }
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    b.append(&title);
    b.append(&explicit_badge());
    b.upcast()
}

/// A song row: artwork (or track number), title, byline, length, menu.
pub fn track_row(t: &Track) -> gtk::ListBoxRow {
    let lead: gtk::Widget = match (&t.thumbnail, &t.index) {
        (None, Some(i)) => {
            let l = gtk::Label::new(Some(i));
            l.set_width_chars(3);
            l.add_css_class("dim-label");
            l.add_css_class("numeric");
            l.upcast()
        }
        _ => art(t.thumbnail.as_deref(), 48, Shape::Square).upcast(),
    };
    let byline = link_label(&track_byline(t, true, true), &["dim-label", "caption"]);
    let b = row_box(&lead, &track_title(t), (!byline.text().is_empty()).then_some(&byline));
    if let Some(d) = &t.duration {
        let l = label(d, &["dim-label", "numeric"]);
        l.set_ellipsize(gtk::pango::EllipsizeMode::None);
        b.append(&l);
    }
    let tr = t.clone();
    let menu = menu_button(move |mb| track_menu(mb, &tr));
    b.append(&menu);
    let row = gtk::ListBoxRow::builder().child(&b).build();
    row.add_css_class("music-row");
    context_menu(&row, &menu);
    row
}

pub fn card_row(c: &Card) -> gtk::ListBoxRow {
    let lead = art(c.thumbnail.as_deref(), if c.shape == Shape::Round { 56 } else { 48 }, if c.shape == Shape::Round { Shape::Round } else { Shape::Square });
    let title = label(&c.title, &["track-title"]);
    let sub = link_label(&links_markup(&c.subtitle), &["dim-label", "caption"]);
    let b = row_box(&lead, title.upcast_ref(), (!c.subtitle.is_empty()).then_some(&sub));
    let cc = c.clone();
    let menu = menu_button(move |mb| card_menu(mb, &cc));
    b.append(&menu);
    let row = gtk::ListBoxRow::builder().child(&b).build();
    row.add_css_class("music-row");
    context_menu(&row, &menu);
    row
}

fn mood_row(md: &Mood) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.append(&label(&md.title, &["track-title"]));
    let row = gtk::ListBoxRow::builder().child(&b).build();
    row.add_css_class("music-row");
    row
}

pub fn entry_row(e: &Entry) -> gtk::ListBoxRow {
    match e {
        Entry::Track(t) => track_row(t),
        Entry::Card(c) => card_row(c),
        Entry::Mood(md) => mood_row(md),
    }
}

/// A list of rows that remembers what each row is.
#[derive(Clone)]
pub struct EntryList {
    pub list: gtk::ListBox,
    entries: Rc<RefCell<Vec<Entry>>>,
}

impl EntryList {
    pub fn new() -> Self {
        let list = gtk::ListBox::new();
        list.add_css_class("music-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let entries: Rc<RefCell<Vec<Entry>>> = Rc::default();
        let e = entries.clone();
        list.connect_row_activated(move |_, row| {
            let entry = usize::try_from(row.index()).ok().and_then(|i| e.borrow().get(i).cloned());
            if let Some(entry) = entry {
                activate_entry(&entry);
            }
        });
        Self { list, entries }
    }

    pub fn append(&self, e: &Entry) {
        self.list.append(&entry_row(e));
        self.entries.borrow_mut().push(e.clone());
    }
}

// ---------------------------------------------------------------------------
// Tiles

fn play_overlay(target: Target) -> gtk::Widget {
    let img = gtk::Image::from_icon_name("media-playback-start-symbolic");
    img.set_pixel_size(20);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.append(&img);
    b.add_css_class("tile-play");
    b.set_halign(gtk::Align::End);
    b.set_valign(gtk::Align::End);
    b.set_margin_end(10);
    b.set_margin_bottom(10);
    b.set_cursor_from_name(Some("pointer"));
    b.set_tooltip_text(Some("Play"));
    let g = gtk::GestureClick::new();
    g.set_button(gdk::BUTTON_PRIMARY);
    g.connect_pressed(|g, _, _, _| {
        g.set_state(gtk::EventSequenceState::Claimed);
    });
    g.connect_released(move |_, _, _, _| play(&target));
    b.add_controller(g);
    b.upcast()
}

const TILE: i32 = 180;
const WIDE_TILE: i32 = 300;

/// An album / playlist / artist / video tile.
pub fn card_tile(c: &Card) -> gtk::Widget {
    let size = if c.shape == Shape::Wide { WIDE_TILE } else { TILE };
    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&art(c.thumbnail.as_deref(), size, c.shape)));
    if let Some(p) = c.play.clone() {
        overlay.add_overlay(&play_overlay(p));
    }
    let title = label(&c.title, &["tile-title"]);
    narrow(&title, 2);
    let sub = link_label(&links_markup(&c.subtitle), &["dim-label", "caption"]);
    narrow(&sub, 2);
    if c.shape == Shape::Round {
        title.set_xalign(0.5);
        title.set_justify(gtk::Justification::Center);
        sub.set_xalign(0.5);
        sub.set_justify(gtk::Justification::Center);
    }
    let v = gtk::Box::new(gtk::Orientation::Vertical, 4);
    v.append(&overlay);
    if c.explicit {
        let h = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        h.append(&title);
        h.append(&explicit_badge());
        title.set_hexpand(true);
        v.append(&h);
    } else {
        v.append(&title);
    }
    if !c.subtitle.is_empty() {
        v.append(&sub);
    }
    let tile = gtk::Button::builder().child(&v).valign(gtk::Align::Start).build();
    tile.add_css_class("flat");
    tile.add_css_class("music-tile");
    let cc = c.clone();
    tile.connect_clicked(move |_| activate_entry(&Entry::Card(cc.clone())));
    let cc = c.clone();
    popup_menu(&tile, move |mb| card_menu(mb, &cc));
    tile.upcast()
}

/// A song as a tile (carousels of singles).
fn track_tile(t: &Track) -> gtk::Widget {
    let card = Card {
        title: t.title.clone(),
        subtitle: t.artists.clone(),
        thumbnail: t.thumbnail.clone(),
        target: Some(t.play_target()),
        play: Some(t.play_target()),
        shape: if t.kind == VideoKind::Video { Shape::Wide } else { Shape::Square },
        explicit: t.explicit,
    };
    card_tile(&card)
}

thread_local! {
    static MOOD_CSS: RefCell<(gtk::CssProvider, std::collections::HashSet<u32>, String)> =
        RefCell::new((gtk::CssProvider::new(), Default::default(), String::new()));
}

/// The color stripe of a mood button, as a CSS class.
fn mood_class(color: u32) -> String {
    let class = format!("mood-{:06x}", color & 0xffffff);
    MOOD_CSS.with_borrow_mut(|(provider, seen, css)| {
        if seen.insert(color) {
            if seen.len() == 1
                && let Some(d) = gdk::Display::default()
            {
                gtk::style_context_add_provider_for_display(&d, provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
            }
            css.push_str(&format!(".{class} {{ border-left-color: #{:06x}; }}\n", color & 0xffffff));
            provider.load_from_string(css);
        }
    });
    class
}

pub fn mood_tile(md: &Mood) -> gtk::Widget {
    let l = label(&md.title, &["mood-title"]);
    let b = gtk::Button::builder().child(&l).build();
    b.add_css_class("mood-tile");
    if let Some(c) = md.color {
        b.add_css_class(&mood_class(c));
    }
    let t = md.target.clone();
    b.connect_clicked(move |_| open(&t));
    b.upcast()
}

pub fn entry_tile(e: &Entry) -> gtk::Widget {
    match e {
        Entry::Track(t) => track_tile(t),
        Entry::Card(c) => card_tile(c),
        Entry::Mood(md) => mood_tile(md),
    }
}

// ---------------------------------------------------------------------------
// Shelves

/// Where more entries of a shelf go (continuations).
#[derive(Clone)]
pub enum Sink {
    List(EntryList),
    Grid(gtk::FlowBox),
}

impl Sink {
    pub fn append(&self, entries: &[Entry]) {
        match self {
            Sink::List(l) => entries.iter().for_each(|e| l.append(e)),
            Sink::Grid(g) => entries.iter().for_each(|e| append_flow(g, &entry_tile(e))),
        }
    }
}

fn append_flow(g: &gtk::FlowBox, w: &gtk::Widget) {
    g.append(w);
    if let Some(c) = w.parent() {
        c.set_focusable(false);
        c.add_css_class("plain-child");
    }
}

fn shelf_header(s: &Shelf, scroller: Option<&gtk::ScrolledWindow>) -> Option<gtk::Widget> {
    let title = s.title.as_ref()?;
    let v = gtk::Box::new(gtk::Orientation::Vertical, 0);
    if let Some(st) = &s.strapline {
        v.append(&label(st, &["dim-label", "caption", "strapline"]));
    }
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let t = match &s.title_target {
        Some(Target::Browse { id, .. }) => link_label(&format!("<a href=\"{LINK}{}\">{}</a>", esc(id), esc(title)), &["shelf-title"]),
        _ => label(title, &["shelf-title"]),
    };
    t.set_hexpand(true);
    row.append(&t);
    if let Some((text, target)) = &s.more {
        let b = gtk::Button::with_label(text);
        b.add_css_class("pill");
        b.add_css_class("small-pill");
        b.set_valign(gtk::Align::Center);
        let t = target.clone();
        b.connect_clicked(move |_| activate_target(&t));
        row.append(&b);
    }
    if let Some(sw) = scroller {
        let prev = round_button("go-previous-symbolic", "Back");
        let next = round_button("go-next-symbolic", "Forward");
        for (b, dir) in [(&prev, -1.0), (&next, 1.0)] {
            let sw = sw.downgrade();
            b.connect_clicked(move |_| {
                let Some(sw) = sw.upgrade() else { return };
                let adj = sw.hadjustment();
                let to = (adj.value() + dir * adj.page_size() * 0.85).clamp(adj.lower(), (adj.upper() - adj.page_size()).max(0.0));
                let a = adj.clone();
                let target = adw::CallbackAnimationTarget::new(move |v| a.set_value(v));
                adw::TimedAnimation::new(&sw, adj.value(), to, 350, target).play();
            });
        }
        let adj = sw.hadjustment();
        let (p, n) = (prev.clone(), next.clone());
        let update = move |a: &gtk::Adjustment| {
            p.set_sensitive(a.value() > a.lower() + 1.0);
            n.set_sensitive(a.value() + a.page_size() < a.upper() - 1.0);
            let scrolls = a.upper() - a.lower() > a.page_size() + 1.0;
            p.set_visible(scrolls);
            n.set_visible(scrolls);
        };
        let u = update.clone();
        adj.connect_value_changed(move |a| u(a));
        adj.connect_changed(move |a| update(a));
        row.append(&prev);
        row.append(&next);
    }
    v.append(&row);
    Some(v.upcast())
}

/// Columns of up to four rows (quick picks, mood carousels).
fn columns(entries: &[Entry], width: i32, rows: usize, tiles: bool) -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    for chunk in entries.chunks(rows) {
        if tiles {
            let col = gtk::Box::new(gtk::Orientation::Vertical, 8);
            col.set_size_request(width, -1);
            col.set_valign(gtk::Align::Start);
            for e in chunk {
                col.append(&entry_tile(e));
            }
            b.append(&col);
        } else {
            let list = EntryList::new();
            list.list.set_size_request(width, -1);
            list.list.set_valign(gtk::Align::Start);
            for e in chunk {
                list.append(e);
            }
            b.append(&list.list);
        }
    }
    b
}

pub fn shelf_widget(s: &Shelf) -> (gtk::Widget, Option<Sink>) {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
    v.add_css_class("music-shelf");
    let mut sink = None;
    match s.style {
        ShelfStyle::Carousel => {
            let all_tracks = s.entries.iter().all(|e| matches!(e, Entry::Track(_)));
            let all_moods = s.entries.iter().all(|e| matches!(e, Entry::Mood(_)));
            let content = if all_tracks && s.entries.len() > 4 {
                columns(&s.entries, 380, 4, false)
            } else if all_moods {
                columns(&s.entries, 200, 4, true)
            } else {
                let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                for e in &s.entries {
                    b.append(&entry_tile(e));
                }
                b
            };
            let sw = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::External)
                .vscrollbar_policy(gtk::PolicyType::Never)
                .propagate_natural_height(true)
                .child(&content)
                .build();
            sw.add_css_class("carousel");
            if let Some(h) = shelf_header(s, Some(&sw)) {
                v.append(&h);
            }
            v.append(&sw);
        }
        ShelfStyle::Grid => {
            if let Some(h) = shelf_header(s, None) {
                v.append(&h);
            }
            let moods = s.entries.iter().all(|e| matches!(e, Entry::Mood(_)));
            let g = gtk::FlowBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .homogeneous(true)
                .min_children_per_line(2)
                .max_children_per_line(if moods { 6 } else { 10 })
                .column_spacing(8)
                .row_spacing(if moods { 8 } else { 12 })
                .build();
            if !moods {
                g.set_homogeneous(false);
            }
            for e in &s.entries {
                append_flow(&g, &entry_tile(e));
            }
            v.append(&g);
            sink = Some(Sink::Grid(g));
        }
        ShelfStyle::List => {
            if let Some(h) = shelf_header(s, None) {
                v.append(&h);
            }
            let l = EntryList::new();
            for e in &s.entries {
                l.append(e);
            }
            v.append(&l.list);
            sink = Some(Sink::List(l));
        }
        ShelfStyle::Text => {
            if let Some(t) = &s.title {
                v.append(&label(t, &["shelf-title"]));
            }
            if let Some(t) = &s.text {
                let l = crate::ui::text::label(t);
                l.add_css_class("about-text");
                v.append(&l);
            }
            if let Some(f) = &s.footer {
                v.append(&label(f, &["dim-label", "caption"]));
            }
        }
    }
    (v.upcast(), sink)
}

// ---------------------------------------------------------------------------
// Chips, top result, headers

pub fn chips_row(chips: &[m::Chip], on_select: impl Fn(Option<Target>) + 'static) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let on_select = Rc::new(on_select);
    for c in chips {
        let t = gtk::ToggleButton::with_label(&c.title);
        t.add_css_class("chip");
        t.set_active(c.selected);
        let (target, selected, f) = (c.target.clone(), c.selected, on_select.clone());
        t.connect_clicked(move |_| f(if selected { None } else { Some(target.clone()) }));
        b.append(&t);
    }
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::External)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .child(&b)
        .build()
        .upcast()
}

pub fn top_result(top: &TopResult) -> gtk::Widget {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 10);
    v.add_css_class("music-shelf");
    v.append(&label("Top result", &["shelf-title"]));
    let card = gtk::Box::new(gtk::Orientation::Horizontal, 18);
    card.add_css_class("top-result");
    let shape = if top.card.shape == Shape::Round { Shape::Round } else { Shape::Square };
    card.append(&art(top.card.thumbnail.as_deref(), 112, shape));
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 4);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    let title = label(&top.card.title, &["title-2"]);
    title.set_wrap(true);
    texts.append(&title);
    texts.append(&link_label(&links_markup(&top.card.subtitle), &["dim-label"]));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_margin_top(8);
    if let Some(t) = top.shuffle.clone() {
        let b = pill("media-playlist-shuffle-symbolic", "Shuffle", true);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    } else if let Some(t) = top.card.play.clone() {
        let b = pill("media-playback-start-symbolic", "Play", true);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if let Some(t) = top.radio.clone() {
        let b = pill("gtube-mix-symbolic", "Mix", false);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if buttons.first_child().is_some() {
        texts.append(&buttons);
    }
    card.append(&texts);
    let btn = gtk::Button::builder().child(&card).build();
    btn.add_css_class("flat");
    btn.add_css_class("top-result-button");
    let target = top.card.target.clone();
    btn.connect_clicked(move |_| {
        if let Some(t) = &target {
            activate_target(t);
        }
    });
    v.append(&btn);
    if !top.tracks.is_empty() {
        let l = EntryList::new();
        for t in &top.tracks {
            l.append(&Entry::Track(t.clone()));
        }
        v.append(&l.list);
    }
    v.upcast()
}

fn require_login() -> bool {
    if api::client().is_logged_in() {
        return true;
    }
    crate::ui::toast("Sign in to use this");
    false
}

fn subscribe_button(h: &Header) -> Option<gtk::Button> {
    let channel = h.channel_id.clone()?;
    let b = gtk::Button::new();
    b.add_css_class("pill");
    b.add_css_class("music-pill");
    let subscribed = Rc::new(std::cell::Cell::new(h.subscribed.unwrap_or(false)));
    let count = h.subscribers.clone().unwrap_or_default();
    let update = {
        let (b, s) = (b.downgrade(), subscribed.clone());
        move || {
            let Some(b) = b.upgrade() else { return };
            let word = if s.get() { "Subscribed" } else { "Subscribe" };
            let short = count.split_whitespace().next().unwrap_or("");
            b.set_label(&if short.is_empty() { word.to_string() } else { format!("{word} {short}") });
            if s.get() {
                b.remove_css_class("subscribe");
            } else {
                b.add_css_class("subscribe");
            }
        }
    };
    update();
    b.connect_clicked(move |_| {
        if !require_login() {
            return;
        }
        let want = !subscribed.get();
        subscribed.set(want);
        update();
        let (c, s, u) = (channel.clone(), subscribed.clone(), update.clone());
        runtime::spawn_local(async move {
            let id = c.clone();
            if let Err(e) = runtime::run(async move { api::client().subscribe(&id, want).await }).await {
                s.set(!want);
                u();
                crate::ui::toast(&format!("Could not update the subscription: {e}"));
            }
        });
    });
    Some(b)
}

fn save_button(playlist_id: String, saved: bool) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::builder().active(saved).valign(gtk::Align::Center).build();
    b.add_css_class("circular");
    let set_look = |b: &gtk::ToggleButton| {
        let on = b.is_active();
        b.set_icon_name(if on { "object-select-symbolic" } else { "list-add-symbolic" });
        b.set_tooltip_text(Some(if on { "Remove from library" } else { "Save to library" }));
    };
    set_look(&b);
    b.connect_toggled(move |b| {
        set_look(b);
        let want = b.is_active();
        if !api::client().is_logged_in() {
            crate::ui::toast("Sign in to save to your library");
            return;
        }
        let id = playlist_id.clone();
        runtime::spawn_local(async move {
            match runtime::run(async move { api::client().music_save_playlist(&id, want).await }).await {
                Ok(()) => crate::ui::toast(if want { "Saved to library" } else { "Removed from library" }),
                Err(e) => crate::ui::toast(&format!("Could not update your library: {e}")),
            }
        });
    });
    b
}

pub fn header_widget(h: &Header, browse_id: Option<&str>) -> gtk::Widget {
    match h.kind {
        HeaderKind::Artist => artist_header(h),
        HeaderKind::Collection => collection_header(h, browse_id),
        HeaderKind::Simple => {
            let l = label(&h.title, &["title-1"]);
            l.set_wrap(true);
            l.upcast()
        }
    }
}

/// Long descriptions (Wikipedia extracts) are cut to their first paragraph;
/// the whole text is in the tooltip.
fn first_paragraph(d: &str) -> &str {
    d.split('\n').map(str::trim).find(|l| !l.is_empty()).unwrap_or(d)
}

fn artist_header(h: &Header) -> gtk::Widget {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 12);
    v.add_css_class("artist-header");
    let banner = gtk::Overlay::new();
    let bin = AspectBin::new(2.8);
    bin.add_css_class("artist-banner");
    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    if let Some(u) = &h.thumbnail {
        images::picture(&pic, u);
    }
    bin.set_child(Some(&pic));
    banner.set_child(Some(&bin));
    let shade = gtk::Box::new(gtk::Orientation::Vertical, 2);
    shade.add_css_class("artist-shade");
    shade.set_valign(gtk::Align::End);
    let title = label(&h.title, &["artist-title"]);
    title.set_wrap(true);
    shade.append(&title);
    if let Some(m) = &h.monthly {
        shade.append(&label(m, &["artist-sub"]));
    }
    banner.add_overlay(&shade);
    v.append(&banner);

    if let Some(d) = &h.description {
        let l = label(first_paragraph(d), &["dim-label"]);
        l.set_wrap(true);
        l.set_lines(3);
        l.set_tooltip_text(Some(d));
        v.append(&l);
    }
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some((text, t)) = h.play.clone() {
        let b = pill("media-playlist-shuffle-symbolic", if text.is_empty() { "Shuffle" } else { &text }, true);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if let Some((text, t)) = h.radio.clone() {
        let b = pill("gtube-mix-symbolic", if text.is_empty() { "Radio" } else { &text }, false);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if let Some(b) = subscribe_button(h) {
        buttons.append(&b);
    }
    v.append(&buttons);
    v.upcast()
}

fn collection_header(h: &Header, browse_id: Option<&str>) -> gtk::Widget {
    let v = gtk::Box::new(gtk::Orientation::Vertical, 16);
    v.add_css_class("collection-header");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 24);
    top.append(&art(h.thumbnail.as_deref(), 220, Shape::Square));
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 6);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    let title = label(&h.title, &["title-1"]);
    title.set_wrap(true);
    title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    texts.append(&title);
    if let Some(s) = &h.subtitle {
        let l = label(s, &["dim-label"]);
        l.set_wrap(true);
        texts.append(&l);
    }
    if !h.owner.is_empty() {
        let l = link_label(&links_markup(&h.owner), &["heading"]);
        l.set_wrap(true);
        texts.append(&l);
    }
    if let Some(s) = &h.second_subtitle {
        let l = label(s, &["dim-label", "caption"]);
        l.set_wrap(true);
        texts.append(&l);
    }
    if let Some(d) = &h.description {
        let l = label(first_paragraph(d), &["dim-label"]);
        l.set_wrap(true);
        l.set_lines(3);
        l.set_tooltip_text(Some(d));
        l.set_margin_top(4);
        texts.append(&l);
    }
    top.append(&texts);
    v.append(&top);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if let Some((_, t)) = h.play.clone() {
        let b = pill("media-playback-start-symbolic", "Play", true);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if let Some(t) = h.shuffle.clone() {
        let b = round_button("media-playlist-shuffle-symbolic", "Shuffle");
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    if let Some((text, t)) = h.radio.clone() {
        let b = round_button("gtube-mix-symbolic", &text);
        b.connect_clicked(move |_| play(&t));
        buttons.append(&b);
    }
    // The list to save / download: the album's or playlist's own id.
    let list = h.playlist_id.clone().or_else(|| browse_id.and_then(|b| b.strip_prefix("VL")).map(str::to_string));
    if let Some(l) = list.clone().filter(|l| !l.starts_with("RD") || l.starts_with("RDCLAK")) {
        buttons.append(&save_button(l, h.saved.unwrap_or(false)));
    }
    if let Some(l) = list.clone() {
        let b = round_button("folder-download-symbolic", "Download…");
        let title = h.title.clone();
        let owner = h.owner.first().map(|o| o.text.clone());
        b.connect_clicked(move |_| {
            crate::ui::downloads::download_playlist(&PlaylistItem { id: l.clone(), title: title.clone(), channel_name: owner.clone(), ..Default::default() })
        });
        buttons.append(&b);
    }
    let hc = h.clone();
    let bid = browse_id.map(str::to_string);
    let menu = menu_button(move |mb| {
        if let Some((_, t)) = hc.play.clone() {
            let t2 = t.clone();
            mb.item("Play next", move || queue().add_target(t2.clone(), true));
            mb.item("Add to queue", move || queue().add_target(t.clone(), false));
        }
        mb.separator();
        if let Some(id) = hc.owner.iter().find_map(|o| o.id.clone()) {
            mb.item(if id.starts_with("UC") { "Go to artist" } else { "Go to profile" }, move || open(&Target::browse(&id)));
        }
        if let Some(url) = bid.as_deref().and_then(|b| share_url(&Target::browse(b))) {
            mb.item("Copy link", move || copy_text(&url));
        }
    });
    menu.set_valign(gtk::Align::Center);
    buttons.append(&menu);
    v.append(&buttons);
    v.upcast()
}
