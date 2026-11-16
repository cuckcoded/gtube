//! Lyrics on "Now playing" and in the immersive view. Timed lyrics light up
//! the line being sung and keep it in view (scrolling by hand pauses that
//! for a moment); clicking a line plays from there.

use super::queue::MusicQueue;
use crate::api::lyrics::{self, Lyrics, Song};
use crate::{api, config, runtime};
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

/// Lines show this much early, as the words are about to be sung.
const LEAD: f64 = 0.2;
/// How long scrolling by hand stops the view from following the song.
const HOLD: Duration = Duration::from_secs(4);

#[derive(Clone)]
pub enum State {
    Loading,
    Missing,
    Ready(Rc<Lyrics>),
}

pub struct LyricsView {
    pub widget: gtk::Stack,
    scroller: gtk::ScrolledWindow,
    content: gtk::Box,
    list: gtk::ListBox,
    plain: gtk::Label,
    source: gtk::Label,
    missing: gtk::Label,
    lyrics: RefCell<Option<Rc<Lyrics>>>,
    current: Cell<Option<usize>>,
    hold_until: Cell<Option<Instant>>,
    animation: RefCell<Option<adw::TimedAnimation>>,
}

impl LyricsView {
    /// `big`: the immersive view's large type.
    pub fn new(big: bool) -> Rc<Self> {
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        list.set_activate_on_single_click(true);
        list.add_css_class("lyrics-lines");
        let plain = gtk::Label::new(None);
        plain.set_wrap(true);
        plain.set_xalign(0.0);
        plain.set_selectable(true);
        plain.add_css_class("lyrics-plain");
        let source = gtk::Label::new(None);
        source.set_xalign(0.0);
        source.set_wrap(true);
        source.add_css_class("dim-label");
        source.add_css_class("caption");
        source.add_css_class("lyrics-source");
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(8);
        // Room below the last lines so they can come up to the reading spot.
        content.set_margin_bottom(if big { 280 } else { 120 });
        content.set_margin_start(8);
        content.set_margin_end(8);
        content.append(&list);
        content.append(&plain);
        content.append(&source);
        let scroller = crate::ui::widgets::scrolled(&content);
        scroller.set_vexpand(true);

        let spinner = adw::Spinner::new();
        spinner.set_size_request(32, 32);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);
        let missing = gtk::Label::new(Some("Lyrics aren't available for this song"));
        missing.add_css_class("dim-label");
        missing.set_wrap(true);
        missing.set_justify(gtk::Justification::Center);
        missing.set_valign(gtk::Align::Center);

        let widget = gtk::Stack::new();
        widget.set_transition_type(gtk::StackTransitionType::Crossfade);
        widget.add_named(&scroller, Some("text"));
        widget.add_named(&spinner, Some("loading"));
        widget.add_named(&missing, Some("none"));
        widget.add_css_class(if big { "lyrics-big" } else { "lyrics-small" });

        let this = Rc::new(Self {
            widget,
            scroller,
            content,
            list,
            plain,
            source,
            missing,
            lyrics: RefCell::new(None),
            current: Cell::new(None),
            hold_until: Cell::new(None),
            animation: RefCell::new(None),
        });

        let wk = Rc::downgrade(&this);
        this.list.connect_row_activated(move |_, row| {
            let Some(v) = wk.upgrade() else { return };
            let start = usize::try_from(row.index())
                .ok()
                .and_then(|i| v.lyrics.borrow().as_ref().and_then(|l| l.lines.get(i).map(|l| l.start)));
            if let Some(s) = start {
                v.hold_until.set(None);
                crate::ui::ctx().player.seek_to(s);
            }
        });
        // Scrolling by hand (wheel, touchpad, scrollbar, touch): stop
        // following the song for a while.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
        let wk = Rc::downgrade(&this);
        scroll.connect_scroll(move |_, _, _| {
            if let Some(v) = wk.upgrade() {
                v.hold();
            }
            glib::Propagation::Proceed
        });
        this.scroller.add_controller(scroll);
        let press = gtk::GestureClick::new();
        press.set_propagation_phase(gtk::PropagationPhase::Capture);
        let wk = Rc::downgrade(&this);
        press.connect_pressed(move |_, _, _, _| {
            if let Some(v) = wk.upgrade() {
                v.hold();
            }
        });
        this.scroller.add_controller(press);
        // Coming into view: jump to where the song is.
        let wk = Rc::downgrade(&this);
        this.widget.connect_map(move |_| {
            if let Some(v) = wk.upgrade() {
                let v2 = v.clone();
                glib::idle_add_local_once(move || v2.scroll_to_current(false));
            }
        });
        this
    }

    pub fn set_state(&self, state: &State) {
        match state {
            State::Loading => {
                self.widget.set_visible_child_name("loading");
                self.lyrics.replace(None);
            }
            State::Missing => {
                self.missing.set_text("Lyrics aren't available for this song");
                self.widget.set_visible_child_name("none");
                self.lyrics.replace(None);
            }
            State::Ready(l) => self.show(l),
        }
    }

    fn show(&self, l: &Rc<Lyrics>) {
        if self.lyrics.borrow().as_ref().is_some_and(|x| Rc::ptr_eq(x, l)) {
            return;
        }
        self.lyrics.replace(Some(l.clone()));
        self.current.set(None);
        self.hold_until.set(None);
        if l.instrumental {
            self.missing.set_text("Instrumental");
            self.widget.set_visible_child_name("none");
            return;
        }
        self.list.remove_all();
        for line in &l.lines {
            let text = if line.text.is_empty() { "♪" } else { line.text.as_str() };
            let label = gtk::Label::new(Some(text));
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_xalign(0.0);
            let row = gtk::ListBoxRow::builder().child(&label).build();
            row.add_css_class("lyric-line");
            self.list.append(&row);
        }
        self.list.set_visible(l.is_timed());
        self.plain.set_text(if l.is_timed() { "" } else { &l.text });
        self.plain.set_visible(!l.is_timed());
        self.source.set_text(l.source.as_deref().unwrap_or(""));
        self.source.set_visible(l.source.is_some());
        self.widget.set_visible_child_name("text");
        self.set_scroll(0.0);
    }

    /// The song is at `t` seconds: light up its line.
    pub fn set_position(&self, t: f64) {
        let index = match self.lyrics.borrow().as_ref() {
            Some(l) if l.is_timed() => l.line_at(t + LEAD),
            _ => return,
        };
        if index == self.current.get() {
            return;
        }
        let row = |i: Option<usize>| i.and_then(|i| self.list.row_at_index(i as i32));
        if let Some(r) = row(self.current.get()) {
            r.remove_css_class("current");
        }
        // Lines already sung look different from the ones to come.
        let (from, to) = match (self.current.get(), index) {
            (Some(a), Some(b)) => (a.min(b), a.max(b)),
            _ => (0, self.lyrics.borrow().as_ref().map_or(0, |l| l.lines.len())),
        };
        for i in from..=to {
            if let Some(r) = row(Some(i)) {
                if index.is_some_and(|c| i < c) {
                    r.add_css_class("sung");
                } else {
                    r.remove_css_class("sung");
                }
            }
        }
        if let Some(r) = row(index) {
            r.add_css_class("current");
        }
        self.current.set(index);
        self.scroll_to_current(true);
    }

    /// Brings the current line to the upper part of the view.
    fn scroll_to_current(&self, animate: bool) {
        if !self.widget.is_mapped() || self.hold_until.get().is_some_and(|t| Instant::now() < t) {
            return;
        }
        self.hold_until.set(None);
        let Some(row) = self.current.get().and_then(|i| self.list.row_at_index(i as i32)) else { return };
        let Some(b) = row.compute_bounds(&self.content) else { return };
        let adj = self.scroller.vadjustment();
        let page = adj.page_size();
        let to = (b.y() as f64 + b.height() as f64 / 2.0 - page * 0.38).clamp(0.0, (adj.upper() - page).max(0.0));
        if (to - adj.value()).abs() < 1.0 {
            return;
        }
        if let Some(a) = self.animation.take() {
            a.pause();
        }
        if !animate {
            self.set_scroll(to);
            return;
        }
        let wk = self.scroller.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if let Some(sw) = wk.upgrade() {
                sw.vadjustment().set_value(v);
            }
        });
        let a = adw::TimedAnimation::new(&self.scroller, adj.value(), to, 450, target);
        a.set_easing(adw::Easing::EaseOutCubic);
        a.play();
        self.animation.replace(Some(a));
    }

    /// The user scrolls: leave the view where they put it for a while.
    fn hold(&self) {
        self.hold_until.set(Some(Instant::now() + HOLD));
        if let Some(a) = self.animation.take() {
            a.pause();
        }
    }

    fn set_scroll(&self, to: f64) {
        self.scroller.vadjustment().set_value(to);
    }
}

/// "3:45" / "1:02:03" in seconds.
fn parse_length(s: &str) -> Option<f64> {
    s.trim().split(':').try_fold(0.0, |acc, p| p.parse::<f64>().ok().map(|n| acc * 60.0 + n)).filter(|d| *d > 0.0)
}

/// Fetches the lyrics of the track that is playing (once per track) for
/// every view that shows them.
pub struct LyricsSource {
    /// The track (and lyrics tab) the state belongs to.
    key: RefCell<String>,
    generation: Cell<u64>,
    state: RefCell<State>,
    views: RefCell<Vec<Weak<LyricsView>>>,
}

impl LyricsSource {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            key: RefCell::new(String::new()),
            generation: Cell::new(0),
            state: RefCell::new(State::Missing),
            views: RefCell::new(Vec::new()),
        })
    }

    pub fn add_view(&self, v: &Rc<LyricsView>) {
        v.set_state(&self.state.borrow());
        self.views.borrow_mut().push(Rc::downgrade(v));
    }

    fn set_state(&self, state: State) {
        self.state.replace(state.clone());
        self.views.borrow_mut().retain(|v| v.strong_count() > 0);
        for v in self.views.borrow().iter().filter_map(Weak::upgrade) {
            v.set_state(&state);
        }
    }

    /// Forgets what was loaded (the lyrics settings changed).
    pub fn reset(&self) {
        self.key.replace(String::new());
    }

    /// Loads the lyrics of the track that is playing, unless they are
    /// loaded (or loading) already.
    pub fn load(self: &Rc<Self>, queue: &MusicQueue) {
        let p = crate::ui::ctx().player.clone();
        let track = queue.current().filter(|_| p.is_music_owned());
        let Some(track) = track else {
            if !self.key.replace(String::new()).is_empty() || !matches!(*self.state.borrow(), State::Missing) {
                self.generation.set(self.generation.get() + 1);
                self.set_state(State::Missing);
            }
            return;
        };
        let info = queue.info();
        if info.video_id != track.video_id || !info.ready {
            // Waiting for the lyrics tab (the queue's info) to arrive.
            let key = format!("{}|pending", track.video_id);
            if self.key.replace(key.clone()) != key {
                self.generation.set(self.generation.get() + 1);
                self.set_state(State::Loading);
            }
            return;
        }
        let use_lrclib = config::settings().lyrics_lrclib;
        let key = format!("{}|{:?}|{use_lrclib}", track.video_id, info.lyrics_id);
        if *self.key.borrow() == key {
            return;
        }
        self.key.replace(key);
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.set_state(State::Loading);
        let duration = track.duration.as_deref().and_then(parse_length).or(Some(p.duration()).filter(|d| *d > 0.0));
        let song = Song { title: track.title.clone(), artists: track.artists.iter().map(|a| a.text.clone()).collect(), duration };
        let lyrics_id = info.lyrics_id.clone();
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let found = runtime::run(async move { Ok::<_, anyhow::Error>(lyrics::find(&api::client(), lyrics_id.as_deref(), &song, use_lrclib).await) })
                .await
                .ok()
                .flatten();
            let Some(s) = wk.upgrade() else { return };
            if s.generation.get() != generation {
                return;
            }
            s.set_state(match found {
                Some(l) => State::Ready(Rc::new(l)),
                None => State::Missing,
            });
            s.set_position(crate::ui::ctx().player.position());
        });
    }

    pub fn set_position(&self, t: f64) {
        if !matches!(*self.state.borrow(), State::Ready(_)) {
            return;
        }
        for v in self.views.borrow().iter().filter_map(Weak::upgrade) {
            v.set_position(t);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reads_track_lengths() {
        assert_eq!(super::parse_length("3:45"), Some(225.0));
        assert_eq!(super::parse_length("1:02:03"), Some(3723.0));
        assert_eq!(super::parse_length(""), None);
        assert_eq!(super::parse_length("live"), None);
    }
}
