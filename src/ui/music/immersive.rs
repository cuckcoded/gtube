//! Immersive view: the track that is playing across the whole screen. Big
//! artwork (or the music video) over a blurred wash of the art, a large
//! spectrum visualizer, and controls that fade away while it plays.

use super::queue::MusicQueue;
use super::spectrum::Spectrum;
use crate::ui::widgets::{FitBin, SeekBar, format_time};
use adw::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct Immersive {
    pub widget: gtk::Overlay,
    /// The player covers this while "Video" is chosen.
    pub dock: gtk::Box,
    backdrop: gtk::Picture,
    art: gtk::Picture,
    stack: gtk::Stack,
    title: gtk::Label,
    byline: gtk::Label,
    up_next: gtk::Label,
    toggle: adw::ToggleGroup,
    seek: SeekBar,
    elapsed: gtk::Label,
    total: gtk::Label,
    play_icon: gtk::Image,
    next: gtk::Button,
    pub spectrum: Spectrum,
    chrome: Vec<gtk::Revealer>,
    shown: RefCell<String>,
    art_url: Rc<RefCell<String>>,
    hide_source: RefCell<Option<glib::SourceId>>,
    updating: Cell<bool>,
    pub active: Cell<bool>,
    pub on_exit: RefCell<Option<Box<dyn Fn()>>>,
}

fn button(icon: &str, tip: &str, classes: &[&str]) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    for c in ["flat", "circular"].iter().chain(classes) {
        b.add_css_class(c);
    }
    b
}

fn revealer(child: &impl IsA<gtk::Widget>) -> gtk::Revealer {
    gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .transition_duration(400)
        .reveal_child(true)
        .child(child)
        .build()
}

impl Immersive {
    pub fn new(queue: &Rc<MusicQueue>) -> Rc<Self> {
        let backdrop = gtk::Picture::new();
        backdrop.set_content_fit(gtk::ContentFit::Cover);
        backdrop.set_can_shrink(true);
        backdrop.add_css_class("immersive-backdrop");
        let shade = gtk::Box::new(gtk::Orientation::Vertical, 0);
        shade.add_css_class("immersive-shade");

        let spectrum = Spectrum::new();
        spectrum.add_css_class("immersive-visualizer");
        spectrum.set_valign(gtk::Align::End);
        spectrum.set_can_target(false);
        spectrum.set_size_request(-1, 300);

        let art = gtk::Picture::new();
        art.set_content_fit(gtk::ContentFit::Cover);
        art.set_can_shrink(true);
        art.add_css_class("immersive-art");
        art.set_overflow(gtk::Overflow::Hidden);
        let art_bin = FitBin::new(1.0, 760);
        art_bin.set_child(Some(&art));
        let dock = gtk::Box::new(gtk::Orientation::Vertical, 0);
        dock.add_css_class("video-placeholder");
        let video_bin = FitBin::new(16.0 / 9.0, 2400);
        video_bin.set_child(Some(&dock));
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_vexpand(true);
        stack.add_named(&art_bin, Some("song"));
        stack.add_named(&video_bin, Some("video"));

        let title = gtk::Label::new(None);
        title.add_css_class("immersive-title");
        title.set_wrap(true);
        title.set_lines(2);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.set_justify(gtk::Justification::Center);
        let byline = gtk::Label::new(None);
        byline.add_css_class("immersive-byline");
        byline.set_ellipsize(gtk::pango::EllipsizeMode::End);

        let seek = SeekBar::new();
        seek.add_css_class("music-seek");
        seek.set_hexpand(true);
        let time = |x: f32| {
            let l = gtk::Label::builder().label("0:00").xalign(x).width_chars(7).build();
            l.add_css_class("numeric");
            l.add_css_class("dim-label");
            l
        };
        let (elapsed, total) = (time(1.0), time(0.0));
        let seek_row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        seek_row.append(&elapsed);
        seek_row.append(&seek);
        seek_row.append(&total);
        let prev = button("media-skip-backward-symbolic", "Previous (Shift+P)", &["immersive-skip"]);
        let play_icon = gtk::Image::from_icon_name("media-playback-pause-symbolic");
        play_icon.set_pixel_size(32);
        let play = gtk::Button::builder().child(&play_icon).tooltip_text("Play / pause (Space)").valign(gtk::Align::Center).build();
        play.add_css_class("circular");
        play.add_css_class("immersive-play");
        let next = button("media-skip-forward-symbolic", "Next (Shift+N)", &["immersive-skip"]);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 24);
        buttons.set_halign(gtk::Align::Center);
        buttons.append(&prev);
        buttons.append(&play);
        buttons.append(&next);
        let controls = gtk::Box::new(gtk::Orientation::Vertical, 14);
        controls.append(&seek_row);
        controls.append(&buttons);
        let controls_rev = revealer(&controls);

        let center = gtk::Box::new(gtk::Orientation::Vertical, 10);
        center.set_halign(gtk::Align::Center);
        center.set_valign(gtk::Align::Fill);
        center.set_margin_top(72);
        center.set_margin_bottom(40);
        center.set_margin_start(32);
        center.set_margin_end(32);
        center.set_size_request(320, -1);
        center.append(&stack);
        center.append(&title);
        center.append(&byline);
        center.append(&controls_rev);
        let clamp = adw::Clamp::builder().maximum_size(1200).child(&center).build();

        // Top: "Up next" on the left, Song / Video in the middle, leave on the right.
        let up_next = gtk::Label::new(None);
        up_next.add_css_class("immersive-next");
        up_next.set_ellipsize(gtk::pango::EllipsizeMode::End);
        up_next.set_xalign(0.0);
        up_next.set_max_width_chars(48);
        let toggle = adw::ToggleGroup::new();
        toggle.add(adw::Toggle::builder().name("song").label("Song").build());
        toggle.add(adw::Toggle::builder().name("video").label("Video").build());
        toggle.add_css_class("round");
        let exit = button("view-restore-symbolic", "Leave immersive view (Esc)", &["osd"]);
        let top = gtk::CenterBox::new();
        top.set_start_widget(Some(&up_next));
        top.set_center_widget(Some(&toggle));
        top.set_end_widget(Some(&exit));
        top.set_margin_top(18);
        top.set_margin_start(24);
        top.set_margin_end(18);
        let top_rev = revealer(&top);
        top_rev.set_valign(gtk::Align::Start);

        let widget = gtk::Overlay::new();
        widget.add_css_class("immersive");
        widget.set_child(Some(&backdrop));
        widget.add_overlay(&shade);
        widget.add_overlay(&spectrum);
        widget.add_overlay(&clamp);
        widget.add_overlay(&top_rev);
        widget.set_visible(false);

        let this = Rc::new(Self {
            widget,
            dock,
            backdrop,
            art,
            stack,
            title,
            byline,
            up_next,
            toggle,
            seek,
            elapsed,
            total,
            play_icon,
            next: next.clone(),
            spectrum,
            chrome: vec![controls_rev, top_rev],
            shown: RefCell::default(),
            art_url: Rc::default(),
            hide_source: RefCell::new(None),
            updating: Cell::new(false),
            active: Cell::new(false),
            on_exit: RefCell::new(None),
        });

        let player = || crate::ui::ctx().player.clone();
        let q = queue.clone();
        prev.connect_clicked(move |_| {
            if player().is_music_owned() {
                q.previous();
            } else {
                player().seek_to(0.0);
            }
        });
        let q = queue.clone();
        next.connect_clicked(move |_| q.next());
        play.connect_clicked(move |_| player().toggle_pause());
        this.seek.connect_seek(move |t, done| {
            if done {
                player().seek_to(t);
            }
        });
        let wk = Rc::downgrade(&this);
        exit.connect_clicked(move |_| {
            if let Some(t) = wk.upgrade()
                && let Some(f) = t.on_exit.borrow().as_ref()
            {
                f();
            }
        });
        let wk = Rc::downgrade(&this);
        this.toggle.connect_active_name_notify(move |t| {
            let Some(i) = wk.upgrade() else { return };
            if i.updating.get() {
                return;
            }
            let video = t.active_name().as_deref() == Some("video");
            i.stack.set_visible_child_name(if video { "video" } else { "song" });
            player().set_music_video(video);
        });
        // Moving the pointer brings the controls (and the pointer) back.
        let motion = gtk::EventControllerMotion::new();
        let wk = Rc::downgrade(&this);
        let last = Cell::new((f64::NAN, f64::NAN));
        motion.connect_motion(move |_, x, y| {
            // Ignore the synthetic motion that comes with redraws.
            if last.replace((x, y)) == (x, y) {
                return;
            }
            if let Some(t) = wk.upgrade() {
                t.show_chrome();
            }
        });
        this.widget.add_controller(motion);
        let click = gtk::GestureClick::new();
        let wk = Rc::downgrade(&this);
        click.connect_released(move |_, _, _, _| {
            if let Some(t) = wk.upgrade() {
                t.show_chrome();
            }
        });
        this.widget.add_controller(click);
        this
    }

    pub fn set_active(self: &Rc<Self>, on: bool) {
        self.active.set(on);
        self.widget.set_visible(on);
        self.spectrum.update_ticking();
        if on {
            self.sync_video_switch(crate::ui::ctx().player.music_video());
            self.show_chrome();
        } else {
            self.widget.set_cursor(None);
            if let Some(id) = self.hide_source.take() {
                id.remove();
            }
        }
    }

    /// Shows the controls, and hides them again after a while of playing.
    fn show_chrome(self: &Rc<Self>) {
        for r in &self.chrome {
            r.set_reveal_child(true);
        }
        self.widget.set_cursor(None);
        if let Some(id) = self.hide_source.take() {
            id.remove();
        }
        if !self.active.get() {
            return;
        }
        let wk = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(3), move || {
            let Some(t) = wk.upgrade() else { return };
            t.hide_source.take();
            let p = crate::ui::ctx().player.clone();
            if !t.active.get() || p.is_paused() || p.current().is_none() {
                return;
            }
            for r in &t.chrome {
                r.set_reveal_child(false);
            }
            t.widget.set_cursor_from_name(Some("none"));
        });
        self.hide_source.replace(Some(id));
    }

    pub fn sync_video_switch(&self, video: bool) {
        self.updating.set(true);
        self.toggle.set_active_name(Some(if video { "video" } else { "song" }));
        self.stack.set_visible_child_name(if video { "video" } else { "song" });
        self.updating.set(false);
    }

    /// The track that is playing changed.
    pub fn update_track(&self, queue: &MusicQueue) {
        let p = crate::ui::ctx().player.clone();
        let music = p.is_music_owned();
        let (id, title, byline, art) = match (music, queue.current(), p.current()) {
            (true, Some(t), _) => {
                let mut by: Vec<String> = vec![t.artists.iter().map(|a| a.text.as_str()).collect::<Vec<_>>().join(", ")];
                by.extend(t.album.as_ref().map(|a| a.text.clone()));
                by.retain(|s| !s.is_empty());
                (t.video_id.clone(), t.title.clone(), by.join(" • "), Some(t.thumbnail_url(1200)))
            }
            (_, _, Some(v)) => (v.id.clone(), v.title.clone(), v.channel_name.clone().unwrap_or_default(), Some(v.thumbnail_url())),
            _ => (String::new(), String::new(), String::new(), None),
        };
        self.next.set_sensitive(music && queue.has_next());
        let upcoming = queue
            .index()
            .filter(|_| music)
            .and_then(|i| queue.tracks().get(i + 1).cloned())
            .map(|t| {
                let artists: Vec<String> = t.artists.iter().map(|a| a.text.clone()).collect();
                if artists.is_empty() { t.title.clone() } else { format!("{} · {}", t.title, artists.join(", ")) }
            });
        self.up_next.set_text(&upcoming.map(|u| format!("Up next: {u}")).unwrap_or_default());
        if *self.shown.borrow() == id {
            return;
        }
        self.shown.replace(id);
        self.title.set_text(&title);
        self.byline.set_text(&byline);
        let url = art.unwrap_or_default();
        if *self.art_url.borrow() == url {
            return;
        }
        self.art_url.replace(url.clone());
        self.art.set_paintable(None::<&gdk::Paintable>);
        self.backdrop.set_paintable(None::<&gdk::Paintable>);
        if url.is_empty() {
            return;
        }
        let (art, backdrop, want) = (self.art.downgrade(), self.backdrop.downgrade(), url.clone());
        let shown = self.art_url.clone();
        crate::images::load(&url, move |tex| {
            if *shown.borrow() != want {
                return;
            }
            if let Some(a) = art.upgrade() {
                a.set_paintable(Some(tex));
            }
            if let Some(b) = backdrop.upgrade() {
                b.set_paintable(super::ambient::blurred(tex).as_ref());
            }
        });
    }

    /// Position, pause and loading.
    pub fn update_state(self: &Rc<Self>, queue: &MusicQueue) {
        if !self.active.get() {
            return;
        }
        let p = crate::ui::ctx().player.clone();
        let (pos, dur) = (p.position(), p.duration());
        self.seek.set_state(pos, dur, p.buffered());
        self.seek.set_segments(p.bar_segments());
        self.elapsed.set_text(&format_time(pos));
        self.total.set_text(&format_time(dur));
        let loading = queue.is_loading() || (p.is_loading() && p.current().is_some());
        self.spectrum.set_playing(p.current().is_some() && !p.is_paused() && !loading);
        let paused = p.is_paused();
        let icon = if paused { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" };
        if self.play_icon.icon_name().as_deref() != Some(icon) {
            self.play_icon.set_icon_name(Some(icon));
            // Pausing keeps the controls up; playing lets them fade.
            self.show_chrome();
        }
    }
}
