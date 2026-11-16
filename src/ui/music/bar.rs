//! The player bar along the bottom of Music mode: the current track and its
//! rating on the left, transport controls in the middle, volume, repeat /
//! shuffle and the "Now playing" toggle on the right, over a spectrum
//! visualizer.

use super::queue::{Info, MusicQueue, Repeat};
use super::spectrum::Spectrum;
use super::widgets::{self as w, art, link_label, track_byline, track_menu};
use crate::api::music::Shape;
use crate::ui::widgets::{SeekBar, format_time};
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct PlayerBar {
    pub widget: gtk::Box,
    seek: SeekBar,
    prev: gtk::Button,
    play: gtk::Button,
    play_stack: gtk::Stack,
    play_icon: gtk::Image,
    next: gtk::Button,
    elapsed: gtk::Label,
    total: gtk::Label,
    pub spectrum: Spectrum,
    art_slot: gtk::Box,
    /// The art shown for the current item.
    pub art_url: RefCell<Option<String>>,
    title: gtk::Label,
    byline: gtk::Label,
    like: gtk::ToggleButton,
    dislike: gtk::ToggleButton,
    menu: gtk::MenuButton,
    mute: gtk::Button,
    volume: gtk::Scale,
    repeat: gtk::Button,
    shuffle: gtk::ToggleButton,
    pub expand: gtk::Button,
    pub immersive: gtk::Button,
    /// Hidden on narrow windows.
    pub optional: Vec<gtk::Widget>,
    updating: Cell<bool>,
    shown: RefCell<String>,
    pub on_toggle_now: RefCell<Option<Box<dyn Fn()>>>,
}

fn flat(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    b
}

fn flat_toggle(icon: &str, tooltip: &str) -> gtk::ToggleButton {
    let b = gtk::ToggleButton::builder().icon_name(icon).tooltip_text(tooltip).valign(gtk::Align::Center).build();
    b.add_css_class("flat");
    b
}

impl PlayerBar {
    pub fn new(queue: &Rc<MusicQueue>) -> Rc<Self> {
        let seek = SeekBar::new();
        seek.add_css_class("music-seek");

        let prev = flat("media-skip-backward-symbolic", "Previous (Shift+P)");
        let play_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        play_icon.set_pixel_size(26);
        let spinner = adw::Spinner::new();
        let play_stack = gtk::Stack::new();
        play_stack.add_named(&play_icon, Some("icon"));
        play_stack.add_named(&spinner, Some("spinner"));
        let play = gtk::Button::builder().child(&play_stack).tooltip_text("Play / pause (Space)").valign(gtk::Align::Center).build();
        play.add_css_class("flat");
        play.add_css_class("circular");
        play.add_css_class("music-play");
        let next = flat("media-skip-forward-symbolic", "Next (Shift+N)");
        for b in [&prev, &next] {
            b.add_css_class("circular");
            b.add_css_class("music-skip");
        }
        // Fixed widths keep the buttons exactly centered while the times
        // change.
        let time_label = |xalign: f32| {
            let l = gtk::Label::builder().label("0:00").xalign(xalign).width_chars(7).build();
            l.add_css_class("dim-label");
            l.add_css_class("numeric");
            l.add_css_class("caption");
            l
        };
        let elapsed = time_label(1.0);
        let total = time_label(0.0);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        controls.set_valign(gtk::Align::Center);
        controls.append(&elapsed);
        controls.append(&prev);
        controls.append(&play);
        controls.append(&next);
        controls.append(&total);

        let art_slot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        art_slot.set_valign(gtk::Align::Center);
        let title = w::label("", &["track-title"]);
        let byline = link_label("", &["dim-label", "caption"]);
        let texts = gtk::Box::new(gtk::Orientation::Vertical, 2);
        texts.set_valign(gtk::Align::Center);
        texts.set_hexpand(true);
        texts.append(&title);
        texts.append(&byline);
        let dislike = flat_toggle("gtube-dislike-symbolic", "Dislike");
        let like = flat_toggle("gtube-like-symbolic", "Like");
        let menu = gtk::MenuButton::builder().icon_name("view-more-symbolic").tooltip_text("More").valign(gtk::Align::Center).build();
        menu.add_css_class("flat");
        let info = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        info.set_hexpand(true);
        info.set_margin_end(12);
        info.add_css_class("music-bar-info");
        // Rating is shown for Music tracks only (update_track), and hidden on
        // narrow windows as a whole (the breakpoint).
        let rating = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        rating.append(&dislike);
        rating.append(&like);
        info.append(&art_slot);
        info.append(&texts);
        info.append(&rating);
        info.append(&menu);

        let mute = flat("audio-volume-high-symbolic", "Mute (M)");
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 130.0, 1.0);
        volume.set_size_request(100, -1);
        volume.set_valign(gtk::Align::Center);
        volume.set_draw_value(false);
        volume.add_mark(100.0, gtk::PositionType::Bottom, None);
        let vol_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        vol_box.append(&mute);
        vol_box.append(&volume);
        let repeat = flat("media-playlist-repeat-symbolic", "Repeat");
        let shuffle = flat_toggle("media-playlist-shuffle-symbolic", "Shuffle");
        let expand = flat("go-up-symbolic", "Now playing");
        let immersive = flat("view-fullscreen-symbolic", "Immersive view (f)");
        let end = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        end.set_halign(gtk::Align::End);
        end.set_margin_start(12);
        end.append(&vol_box);
        end.append(&repeat);
        end.append(&shuffle);
        end.append(&immersive);
        end.append(&expand);

        let row = gtk::CenterBox::new();
        row.add_css_class("music-bar-row");
        row.set_start_widget(Some(&info));
        row.set_center_widget(Some(&controls));
        row.set_end_widget(Some(&end));

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&seek);
        content.append(&row);
        let spectrum = Spectrum::new();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&spectrum));
        overlay.add_overlay(&content);
        overlay.set_measure_overlay(&content, true);

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.add_css_class("music-bar");
        widget.append(&overlay);

        let optional: Vec<gtk::Widget> = vec![
            elapsed.clone().upcast(),
            total.clone().upcast(),
            vol_box.upcast(),
            repeat.clone().upcast(),
            shuffle.clone().upcast(),
            rating.upcast(),
            immersive.clone().upcast(),
        ];

        let this = Rc::new(Self {
            widget,
            seek,
            prev,
            play,
            play_stack,
            play_icon,
            next,
            elapsed,
            total,
            spectrum,
            art_slot,
            art_url: RefCell::new(None),
            title,
            byline,
            like,
            dislike,
            menu,
            mute,
            volume,
            repeat,
            shuffle,
            expand,
            immersive,
            optional,
            updating: Cell::new(false),
            shown: RefCell::new(String::new()),
            on_toggle_now: RefCell::new(None),
        });
        this.connect(queue, &info);
        this.update_modes(queue);
        this
    }

    fn connect(self: &Rc<Self>, queue: &Rc<MusicQueue>, info: &gtk::Box) {
        let player = || crate::ui::ctx().player.clone();
        let q = queue.clone();
        self.prev.connect_clicked(move |_| {
            if player().is_music_owned() {
                q.previous();
            } else {
                player().seek_to(0.0);
            }
        });
        let q = queue.clone();
        self.next.connect_clicked(move |_| q.next());
        self.play.connect_clicked(move |_| player().toggle_pause());
        self.mute.connect_clicked(move |_| player().toggle_mute());
        let wk = Rc::downgrade(self);
        self.volume.connect_value_changed(move |s| {
            if let Some(b) = wk.upgrade()
                && !b.updating.get()
            {
                player().set_volume(s.value());
            }
        });
        self.seek.connect_seek(move |t, done| {
            if done {
                player().seek_to(t);
            }
        });
        let q = queue.clone();
        self.repeat.connect_clicked(move |_| q.cycle_repeat());
        let (q, wk) = (queue.clone(), Rc::downgrade(self));
        self.shuffle.connect_toggled(move |b| {
            if wk.upgrade().is_some_and(|s| !s.updating.get()) {
                q.set_shuffle(b.is_active());
            }
        });
        for (btn, status) in [(self.like.clone(), "LIKE"), (self.dislike.clone(), "DISLIKE")] {
            let (q, wk) = (queue.clone(), Rc::downgrade(self));
            btn.connect_toggled(move |b| {
                if wk.upgrade().is_some_and(|s| !s.updating.get()) {
                    q.set_like(if b.is_active() { status } else { "INDIFFERENT" });
                }
            });
        }
        let wk = Rc::downgrade(self);
        self.expand.connect_clicked(move |_| {
            if let Some(b) = wk.upgrade()
                && let Some(f) = b.on_toggle_now.borrow().as_ref()
            {
                f();
            }
        });
        // Clicking the track area (not its links or buttons) toggles "Now
        // playing", like on music.youtube.com.
        let click = gtk::GestureClick::new();
        let wk = Rc::downgrade(self);
        click.connect_released(move |g, _, x, y| {
            let Some(b) = wk.upgrade() else { return };
            let Some(w) = g.widget() else { return };
            let picked = w.pick(x, y, gtk::PickFlags::DEFAULT);
            let on_control = picked.is_some_and(|p| p.ancestor(gtk::Button::static_type()).is_some() || p.is::<gtk::Button>());
            if on_control {
                return;
            }
            if let Some(f) = b.on_toggle_now.borrow().as_ref() {
                f();
            }
        });
        info.add_controller(click);
    }

    pub fn set_now_open(&self, open: bool) {
        self.expand.set_icon_name(if open { "go-down-symbolic" } else { "go-up-symbolic" });
        self.expand.set_tooltip_text(Some(if open { "Close Now playing" } else { "Now playing" }));
    }

    pub fn update_modes(&self, queue: &MusicQueue) {
        self.updating.set(true);
        self.shuffle.set_active(queue.shuffle());
        let (icon, tip) = match queue.repeat() {
            Repeat::Off => ("media-playlist-consecutive-symbolic", "Repeat: off"),
            Repeat::All => ("media-playlist-repeat-symbolic", "Repeat: all"),
            Repeat::One => ("media-playlist-repeat-song-symbolic", "Repeat: one"),
        };
        self.repeat.set_icon_name(icon);
        self.repeat.set_tooltip_text(Some(tip));
        if queue.repeat() == Repeat::Off {
            self.repeat.remove_css_class("accent");
        } else {
            self.repeat.add_css_class("accent");
        }
        self.updating.set(false);
    }

    pub fn update_info(&self, info: &Info) {
        self.updating.set(true);
        let like = info.like.as_deref();
        self.like.set_active(like == Some("LIKE"));
        self.dislike.set_active(like == Some("DISLIKE"));
        self.updating.set(false);
    }

    /// Shows the track that is playing (a queue track, or a YouTube video
    /// that is playing while Music mode is shown).
    pub fn update_track(&self, queue: &MusicQueue) {
        let p = crate::ui::ctx().player.clone();
        let music = p.is_music_owned();
        let (id, title, markup, thumb, track) = match (music, queue.current(), p.current()) {
            (true, Some(t), _) => (t.video_id.clone(), t.title.clone(), track_byline(&t, true, false), Some(t.thumbnail_url(240)), Some(t)),
            (_, _, Some(v)) => {
                let by = glib::markup_escape_text(v.channel_name.as_deref().unwrap_or("")).to_string();
                (v.id.clone(), v.title.clone(), by, Some(v.thumbnail_url()), None)
            }
            _ => (String::new(), String::new(), String::new(), None, None),
        };
        self.next.set_sensitive(music && queue.has_next());
        self.prev.set_sensitive(music || p.current().is_some());
        self.like.set_visible(music);
        self.dislike.set_visible(music);
        if *self.shown.borrow() == id && !id.is_empty() {
            return;
        }
        self.shown.replace(id);
        self.art_url.replace(thumb.clone());
        self.title.set_text(&title);
        self.title.set_tooltip_text(Some(&title));
        self.byline.set_markup(&markup);
        while let Some(c) = self.art_slot.first_child() {
            self.art_slot.remove(&c);
        }
        self.art_slot.append(&art(thumb.as_deref(), 56, Shape::Square));
        self.menu.set_menu_model(None::<&gtk::gio::MenuModel>);
        match track {
            Some(t) => {
                let mut mb = w::MenuBuilder::new();
                track_menu(&mut mb, &t);
                mb.install(&self.menu);
                self.menu.set_visible(true);
            }
            None => self.menu.set_visible(false),
        }
    }

    /// Position, pause, loading and volume.
    pub fn update_state(&self, queue: &MusicQueue) {
        let p = crate::ui::ctx().player.clone();
        let (pos, dur) = (p.position(), p.duration());
        self.seek.set_state(pos, dur, p.buffered());
        self.seek.set_segments(p.bar_segments());
        self.elapsed.set_text(&format_time(pos));
        self.total.set_text(&format_time(dur));
        let loading = queue.is_loading() || (p.is_loading() && p.current().is_some());
        self.spectrum.set_playing(p.current().is_some() && !p.is_paused() && !loading);
        self.play_stack.set_visible_child_name(if loading { "spinner" } else { "icon" });
        self.play_icon.set_icon_name(Some(if p.is_paused() { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" }));
        self.updating.set(true);
        if (self.volume.value() - p.volume()).abs() > 0.5 {
            self.volume.set_value(p.volume());
        }
        self.updating.set(false);
        let v = p.volume();
        let icon = if p.is_muted() || v <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if v < 34.0 {
            "audio-volume-low-symbolic"
        } else if v < 67.0 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.mute.set_icon_name(icon);
    }
}
