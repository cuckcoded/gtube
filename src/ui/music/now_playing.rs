//! "Now playing": the artwork or the music video (Song / Video switch) next
//! to the Up next, Lyrics and Related tabs.

use super::lyrics::LyricsView;
use super::page::{PageView, Source};
use super::queue::MusicQueue;
use super::widgets::{self as w, art, context_menu, link_label, menu_button, track_byline, track_title};
use crate::api::music::{Shape, Target, Track};
use crate::ui::widgets::{FitBin, SideLayout};
use crate::images;
use adw::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct NowPlaying {
    pub widget: adw::BreakpointBin,
    /// The player covers this while "Video" is chosen.
    pub dock: gtk::Box,
    art: gtk::Picture,
    main_stack: gtk::Stack,
    toggle: adw::ToggleGroup,
    tabs: adw::ViewStack,
    pub queue_list: Rc<QueueList>,
    pub lyrics: Rc<LyricsView>,
    related: Rc<PageView>,
    related_for: RefCell<String>,
    shown_art: RefCell<String>,
    updating: Cell<bool>,
    pub visible: Cell<bool>,
}

impl NowPlaying {
    pub fn new(queue: &Rc<MusicQueue>) -> Rc<Self> {
        // --- Main column: Song / Video and the artwork or video
        let toggle = adw::ToggleGroup::new();
        toggle.add(adw::Toggle::builder().name("song").label("Song").build());
        toggle.add(adw::Toggle::builder().name("video").label("Video").build());
        toggle.set_halign(gtk::Align::Center);
        toggle.add_css_class("round");

        let art = gtk::Picture::new();
        art.set_content_fit(gtk::ContentFit::Cover);
        art.set_can_shrink(true);
        art.add_css_class("now-art");
        art.set_overflow(gtk::Overflow::Hidden);
        let art_bin = FitBin::new(1.0, 720);
        art_bin.set_child(Some(&art));
        let dock = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dock.add_css_class("video-placeholder");
        let video_bin = FitBin::new(16.0 / 9.0, 1600);
        video_bin.set_child(Some(&dock));
        let main_stack = gtk::Stack::new();
        main_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        main_stack.set_vexpand(true);
        main_stack.set_hexpand(true);
        main_stack.add_named(&art_bin, Some("song"));
        main_stack.add_named(&video_bin, Some("video"));
        let main = gtk::Box::new(gtk::Orientation::Vertical, 16);
        main.append(&toggle);
        main.append(&main_stack);

        // --- Side column: tabs
        let tabs = adw::ViewStack::new();
        tabs.set_vexpand(true);
        let queue_list = QueueList::new(queue);
        tabs.add_titled(&queue_list.widget, Some("queue"), "Up next");
        let lyrics = LyricsView::new(false);
        tabs.add_titled(&lyrics.widget, Some("lyrics"), "Lyrics");

        let related = PageView::new();
        related.show_title.set(false);
        tabs.add_titled(&related.widget, Some("related"), "Related");

        let switcher = adw::InlineViewSwitcher::builder().stack(&tabs).build();
        switcher.add_css_class("flat");
        let side = gtk::Box::new(gtk::Orientation::Vertical, 10);
        side.append(&switcher);
        side.append(&tabs);
        side.add_css_class("now-side");

        let layout = SideLayout::new(&main, &side);
        layout.add_css_class("now-playing");
        layout.set_margin_top(18);
        layout.set_margin_bottom(12);
        layout.set_margin_start(24);
        layout.set_margin_end(24);
        // On big screens the artwork and the tabs grow, side by side in the
        // middle rather than at the far edges.
        let clamp = adw::Clamp::builder().maximum_size(2300).tightening_threshold(2300).child(&layout).build();

        let widget = adw::BreakpointBin::builder().width_request(360).height_request(300).child(&clamp).build();
        // Narrow: the tabs below a small artwork. Wide: both bigger.
        let narrow = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 900sp").unwrap());
        narrow.add_setter(&layout, "margin-start", Some(&12.to_value()));
        narrow.add_setter(&layout, "margin-end", Some(&12.to_value()));
        let wide = adw::Breakpoint::new(adw::BreakpointCondition::parse("min-width: 1600sp and max-width: 2100sp").unwrap());
        let wider = adw::Breakpoint::new(adw::BreakpointCondition::parse("min-width: 2100sp").unwrap());
        for bp in [&narrow, &wide, &wider] {
            widget.add_breakpoint(bp.clone());
        }
        let (l, a) = (layout.clone(), art_bin.clone());
        widget.connect_current_breakpoint_notify(move |b| {
            let bp = b.current_breakpoint();
            let (stacked, side, art) = match bp.as_ref() {
                Some(x) if *x == narrow => (true, SideLayout::SIDE_WIDTH, 280),
                Some(x) if *x == wide => (false, 460, 860),
                Some(x) if *x == wider => (false, 540, 1040),
                _ => (false, SideLayout::SIDE_WIDTH, 720),
            };
            l.set_stacked(stacked);
            l.set_side_width(side);
            a.set_max_width(art);
        });

        let this = Rc::new(Self {
            widget,
            dock,
            art,
            main_stack,
            toggle,
            tabs,
            queue_list,
            lyrics,
            related,
            related_for: RefCell::new(String::new()),
            shown_art: RefCell::new(String::new()),
            updating: Cell::new(false),
            visible: Cell::new(false),
        });
        this.connect(queue);
        this
    }

    fn connect(self: &Rc<Self>, queue: &Rc<MusicQueue>) {
        let video = crate::config::settings().music_video;
        self.toggle.set_active_name(Some(if video { "video" } else { "song" }));
        self.main_stack.set_visible_child_name(if video { "video" } else { "song" });
        let wk = Rc::downgrade(self);
        self.toggle.connect_active_name_notify(move |t| {
            let Some(n) = wk.upgrade() else { return };
            if n.updating.get() {
                return;
            }
            let video = t.active_name().as_deref() == Some("video");
            n.main_stack.set_visible_child_name(if video { "video" } else { "song" });
            // The player tells the queue (see main.rs), also when "A" is used.
            crate::ui::ctx().player.set_music_video(video);
        });
        let (wk, q) = (Rc::downgrade(self), queue.clone());
        self.tabs.connect_visible_child_name_notify(move |_| {
            if let Some(n) = wk.upgrade() {
                n.load_tab(&q);
                crate::ui::ctx().music.update_lyrics();
            }
        });
    }

    /// "Now playing" was opened or closed.
    pub fn set_visible(self: &Rc<Self>, on: bool, queue: &MusicQueue) {
        self.visible.set(on);
        if on {
            self.load_tab(queue);
            self.queue_list.scroll_to_current();
        }
    }

    /// The lyrics tab is on screen.
    pub fn lyrics_shown(&self) -> bool {
        self.visible.get() && self.tabs.visible_child_name().as_deref() == Some("lyrics")
    }

    pub fn update_queue(&self, queue: &Rc<MusicQueue>) {
        self.queue_list.update_queue(queue);
    }

    pub fn update_current(&self, queue: &MusicQueue) {
        self.queue_list.update_current(queue);
        let p = crate::ui::ctx().player.clone();
        let url = match (p.is_music_owned(), queue.current(), p.current()) {
            (true, Some(t), _) => t.thumbnail_url(1080),
            (_, _, Some(v)) => v.thumbnail_url(),
            _ => String::new(),
        };
        if *self.shown_art.borrow() != url {
            self.art.set_paintable(None::<&gtk::gdk::Paintable>);
            images::picture(&self.art, &url);
            self.shown_art.replace(url);
        }
        if self.visible.get() {
            self.queue_list.scroll_to_current();
        }
    }

    /// The Song / Video switch follows the player (also changed with "A").
    pub fn sync_video_switch(&self, video: bool) {
        self.updating.set(true);
        self.toggle.set_active_name(Some(if video { "video" } else { "song" }));
        self.main_stack.set_visible_child_name(if video { "video" } else { "song" });
        self.updating.set(false);
    }

    /// Loads the related tab for the current track when shown.
    pub fn load_tab(self: &Rc<Self>, queue: &MusicQueue) {
        if !self.visible.get() {
            return;
        }
        if self.tabs.visible_child_name().as_deref() != Some("related") {
            return;
        }
        let Some(id) = queue.info().related_id else { return };
        if *self.related_for.borrow() == id {
            return;
        }
        self.related_for.replace(id.clone());
        self.related.set_source(Source::browse(&id));
        self.related.load();
    }
}

/// The play queue as a list ("Up next" on "Now playing" and in the
/// immersive view); the current track is highlighted and kept in view.
pub struct QueueList {
    pub widget: gtk::ScrolledWindow,
    title: gtk::Label,
    list: gtk::ListBox,
}

impl QueueList {
    pub fn new(queue: &Rc<MusicQueue>) -> Rc<Self> {
        let title = w::label("", &["dim-label", "caption", "queue-from"]);
        title.set_margin_start(8);
        let list = gtk::ListBox::new();
        list.add_css_class("music-list");
        list.set_selection_mode(gtk::SelectionMode::None);
        let b = gtk::Box::new(gtk::Orientation::Vertical, 6);
        b.append(&title);
        b.append(&list);
        let widget = crate::ui::widgets::scrolled(&b);
        widget.set_vexpand(true);
        let q = queue.clone();
        list.connect_row_activated(move |_, row| {
            if let Ok(i) = usize::try_from(row.index()) {
                q.play_index(i);
            }
        });
        Rc::new(Self { widget, title, list })
    }

    pub fn update_queue(&self, queue: &Rc<MusicQueue>) {
        self.list.remove_all();
        let current = queue.index();
        for (i, t) in queue.tracks().iter().enumerate() {
            self.list.append(&queue_row(t, i, Some(i) == current, queue));
        }
        let title = queue.title().map(|t| format!("Playing from {t}")).unwrap_or_default();
        self.title.set_text(&title);
        self.title.set_visible(!title.is_empty());
    }

    pub fn update_current(&self, queue: &MusicQueue) {
        let current = queue.index();
        let mut i = 0;
        while let Some(row) = self.list.row_at_index(i) {
            if Some(i as usize) == current {
                row.add_css_class("now-playing");
            } else {
                row.remove_css_class("now-playing");
            }
            i += 1;
        }
    }

    /// Scrolls the current track into view (once the list is laid out).
    pub fn scroll_to_current(&self) {
        let (list, sw) = (self.list.clone(), self.widget.clone());
        gtk::glib::idle_add_local_once(move || {
            let mut i = 0;
            while let Some(row) = list.row_at_index(i) {
                if row.has_css_class("now-playing") {
                    if let Some(b) = row.compute_bounds(&list) {
                        let adj = sw.vadjustment();
                        let (top, bottom) = (b.y() as f64, (b.y() + b.height()) as f64);
                        if top < adj.value() || bottom > adj.value() + adj.page_size() {
                            adj.set_value((top - adj.page_size() / 3.0).max(0.0));
                        }
                    }
                    break;
                }
                i += 1;
            }
        });
    }
}

fn queue_row(t: &Track, i: usize, current: bool, queue: &Rc<MusicQueue>) -> gtk::ListBoxRow {
    let lead = art(t.thumbnail.as_deref(), 40, Shape::Square);
    let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(&track_title(t));
    let by = link_label(&track_byline(t, false, false), &["dim-label", "caption"]);
    texts.append(&by);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    b.append(&lead);
    b.append(&texts);
    if let Some(d) = &t.duration {
        b.append(&w::label(d, &["dim-label", "numeric", "caption"]));
    }
    let (tr, q) = (t.clone(), Rc::downgrade(queue));
    let menu = menu_button(move |mb| {
        let Some(q) = q.upgrade() else { return };
        let q2 = q.clone();
        mb.item("Play next", move || q2.move_next(i));
        let q2 = q.clone();
        mb.item("Remove from queue", move || q2.remove(i));
        mb.separator();
        let t2 = tr.clone();
        mb.item("Start radio", move || super::play(&t2.radio_target()));
        if let Some(id) = tr.album.as_ref().and_then(|a| a.id.clone()) {
            mb.item("Go to album", move || super::open(&Target::browse(&id)));
        }
        if let Some(id) = tr.first_artist_id().map(str::to_string) {
            mb.item("Go to artist", move || super::open(&Target::browse(&id)));
        }
        mb.separator();
        let t2 = tr.clone();
        mb.item("Download…", move || crate::ui::downloads::show_dialog(&t2.video_item()));
        let url = tr.url();
        mb.item("Copy link", move || w::copy_text(&url));
    });
    b.append(&menu);
    let row = gtk::ListBoxRow::builder().child(&b).build();
    row.add_css_class("music-row");
    if current {
        row.add_css_class("now-playing");
    }
    context_menu(&row, &menu);
    row
}
