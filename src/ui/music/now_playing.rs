//! "Now playing": the artwork or the music video (Song / Video switch) next
//! to the Up next, Lyrics and Related tabs.

use super::page::{PageView, Source};
use super::queue::MusicQueue;
use super::widgets::{self as w, art, context_menu, link_label, menu_button, track_byline, track_title};
use crate::api::music::{self as m, Shape, Target, Track};
use crate::ui::widgets::{FitBin, SideLayout};
use crate::{api, images, runtime};
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
    queue_title: gtk::Label,
    queue_list: gtk::ListBox,
    lyrics_stack: gtk::Stack,
    lyrics_label: gtk::Label,
    lyrics_source: gtk::Label,
    related: Rc<PageView>,
    lyrics_for: RefCell<String>,
    related_for: RefCell<String>,
    lyrics_gen: Cell<u64>,
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
        let queue_title = w::label("", &["dim-label", "caption"]);
        queue_title.set_margin_start(8);
        let queue_list = gtk::ListBox::new();
        queue_list.add_css_class("music-list");
        queue_list.set_selection_mode(gtk::SelectionMode::None);
        let up_next = gtk::Box::new(gtk::Orientation::Vertical, 6);
        up_next.append(&queue_title);
        up_next.append(&queue_list);
        let queue_scroll = crate::ui::widgets::scrolled(&up_next);
        queue_scroll.set_vexpand(true);
        tabs.add_titled(&queue_scroll, Some("queue"), "Up next");

        let lyrics_label = gtk::Label::new(None);
        lyrics_label.set_wrap(true);
        lyrics_label.set_xalign(0.0);
        lyrics_label.set_selectable(true);
        lyrics_label.add_css_class("lyrics");
        let lyrics_source = w::label("", &["dim-label", "caption"]);
        let lyrics_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
        lyrics_box.set_margin_start(8);
        lyrics_box.set_margin_end(8);
        lyrics_box.set_margin_top(8);
        lyrics_box.set_margin_bottom(24);
        lyrics_box.append(&lyrics_label);
        lyrics_box.append(&lyrics_source);
        let lyrics_scroll = crate::ui::widgets::scrolled(&lyrics_box);
        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);
        let none = gtk::Label::new(Some("Lyrics aren't available for this song"));
        none.add_css_class("dim-label");
        none.set_wrap(true);
        let lyrics_stack = gtk::Stack::new();
        lyrics_stack.add_named(&lyrics_scroll, Some("text"));
        lyrics_stack.add_named(&spinner, Some("loading"));
        lyrics_stack.add_named(&none, Some("none"));
        tabs.add_titled(&lyrics_stack, Some("lyrics"), "Lyrics");

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

        let widget = adw::BreakpointBin::builder().width_request(360).height_request(300).child(&layout).build();
        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 900sp").unwrap());
        let (l, a) = (layout.clone(), art_bin.clone());
        bp.connect_apply(move |_| {
            l.set_stacked(true);
            a.set_max_width(280);
        });
        let (l, a) = (layout.clone(), art_bin.clone());
        bp.connect_unapply(move |_| {
            l.set_stacked(false);
            a.set_max_width(720);
        });
        bp.add_setter(&layout, "margin-start", Some(&12.to_value()));
        bp.add_setter(&layout, "margin-end", Some(&12.to_value()));
        widget.add_breakpoint(bp);

        let this = Rc::new(Self {
            widget,
            dock,
            art,
            main_stack,
            toggle,
            tabs,
            queue_title,
            queue_list,
            lyrics_stack,
            lyrics_label,
            lyrics_source,
            related,
            lyrics_for: RefCell::new(String::new()),
            related_for: RefCell::new(String::new()),
            lyrics_gen: Cell::new(0),
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
        let q = queue.clone();
        self.queue_list.connect_row_activated(move |_, row| {
            if let Ok(i) = usize::try_from(row.index()) {
                q.play_index(i);
            }
        });
        let (wk, q) = (Rc::downgrade(self), queue.clone());
        self.tabs.connect_visible_child_name_notify(move |_| {
            if let Some(n) = wk.upgrade() {
                n.load_tab(&q);
            }
        });
    }

    /// "Now playing" was opened or closed.
    pub fn set_visible(self: &Rc<Self>, on: bool, queue: &MusicQueue) {
        self.visible.set(on);
        if on {
            self.load_tab(queue);
            self.scroll_to_current();
        }
    }

    pub fn update_queue(&self, queue: &Rc<MusicQueue>) {
        while let Some(c) = self.queue_list.first_child() {
            self.queue_list.remove(&c);
        }
        let current = queue.index();
        for (i, t) in queue.tracks().iter().enumerate() {
            self.queue_list.append(&queue_row(t, i, Some(i) == current, queue));
        }
        let title = queue.title().map(|t| format!("Playing from {t}")).unwrap_or_default();
        self.queue_title.set_text(&title);
        self.queue_title.set_visible(!title.is_empty());
    }

    pub fn update_current(&self, queue: &MusicQueue) {
        let current = queue.index();
        let mut i = 0;
        while let Some(row) = self.queue_list.row_at_index(i) {
            if Some(i as usize) == current {
                row.add_css_class("now-playing");
            } else {
                row.remove_css_class("now-playing");
            }
            i += 1;
        }
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
        self.scroll_to_current();
    }

    fn scroll_to_current(&self) {
        if !self.visible.get() {
            return;
        }
        let list = self.queue_list.clone();
        gtk::glib::idle_add_local_once(move || {
            let mut i = 0;
            while let Some(row) = list.row_at_index(i) {
                if row.has_css_class("now-playing") {
                    if let Some(sw) = list.ancestor(gtk::ScrolledWindow::static_type()).and_downcast::<gtk::ScrolledWindow>()
                        && let Some(b) = row.compute_bounds(&list)
                    {
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

    /// The Song / Video switch follows the player (also changed with "A").
    pub fn sync_video_switch(&self, video: bool) {
        self.updating.set(true);
        self.toggle.set_active_name(Some(if video { "video" } else { "song" }));
        self.main_stack.set_visible_child_name(if video { "video" } else { "song" });
        self.updating.set(false);
    }

    /// Loads the lyrics / related tab for the current track when shown.
    pub fn load_tab(self: &Rc<Self>, queue: &MusicQueue) {
        if !self.visible.get() {
            return;
        }
        let info = queue.info();
        match self.tabs.visible_child_name().as_deref() {
            Some("lyrics") => {
                let key = format!("{}|{:?}|{}", info.video_id, info.lyrics_id, info.ready);
                if *self.lyrics_for.borrow() == key {
                    return;
                }
                self.lyrics_for.replace(key);
                let generation = self.lyrics_gen.get() + 1;
                self.lyrics_gen.set(generation);
                let Some(id) = info.lyrics_id else {
                    self.lyrics_stack.set_visible_child_name(if info.ready { "none" } else { "loading" });
                    return;
                };
                self.lyrics_stack.set_visible_child_name("loading");
                let wk = Rc::downgrade(self);
                runtime::spawn_local(async move {
                    let res = runtime::run(async move { api::client().music_browse(&id, None).await.map(|v| m::parse_lyrics(&v)) }).await;
                    let Some(n) = wk.upgrade() else { return };
                    if n.lyrics_gen.get() != generation {
                        return;
                    }
                    match res {
                        Ok(Some(l)) => {
                            n.lyrics_label.set_text(&l.text);
                            n.lyrics_source.set_text(l.source.as_deref().unwrap_or(""));
                            n.lyrics_source.set_visible(l.source.is_some());
                            n.lyrics_stack.set_visible_child_name("text");
                        }
                        _ => n.lyrics_stack.set_visible_child_name("none"),
                    }
                });
            }
            Some("related") => {
                let Some(id) = info.related_id else { return };
                if *self.related_for.borrow() == id {
                    return;
                }
                self.related_for.replace(id.clone());
                self.related.set_source(Source::browse(&id));
                self.related.load();
            }
            _ => {}
        }
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
