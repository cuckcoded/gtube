//! The Music play queue ("Up next"). YouTube Music's `next` endpoint builds
//! it (an album, a playlist, a radio…); radios continue endlessly and, once
//! a list ends, autoplay continues with a mix of similar songs.

use crate::api::music::{self as m, Target, Track, VideoKind};
use crate::api::{self, LikeAction};
use crate::{config, runtime};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Repeat {
    Off,
    All,
    One,
}

impl Repeat {
    fn from_setting(s: &str) -> Self {
        match s {
            "all" => Repeat::All,
            "one" => Repeat::One,
            _ => Repeat::Off,
        }
    }

    fn setting(self) -> &'static str {
        match self {
            Repeat::Off => "off",
            Repeat::All => "all",
            Repeat::One => "one",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// The list changed.
    Queue,
    /// Another track is playing.
    Current,
    /// Like status, lyrics or related of the current track arrived.
    Info,
    /// Shuffle / repeat.
    Modes,
    /// A queue request started or finished.
    Loading,
}

/// What YouTube Music knows about the current track beyond the queue.
#[derive(Clone, Default, Debug)]
pub struct Info {
    pub video_id: String,
    pub lyrics_id: Option<String>,
    pub related_id: Option<String>,
    /// "LIKE", "DISLIKE" or "INDIFFERENT".
    pub like: Option<String>,
    /// The fields above have arrived.
    pub ready: bool,
}

pub struct MusicQueue {
    tracks: RefCell<Vec<Track>>,
    /// The order before shuffling.
    unshuffled: RefCell<Option<Vec<Track>>>,
    index: Cell<Option<usize>>,
    continuation: RefCell<Option<String>>,
    playlist_id: RefCell<Option<String>>,
    automix: RefCell<Option<Target>>,
    title: RefCell<Option<String>>,
    shuffle: Cell<bool>,
    repeat: Cell<Repeat>,
    request: Cell<u64>,
    info_request: Cell<u64>,
    extending: Cell<bool>,
    advance_after_extend: Cell<bool>,
    loading: Cell<bool>,
    errors: Cell<u32>,
    info: RefCell<Info>,
    listeners: RefCell<Vec<Rc<dyn Fn(Change)>>>,
}

fn player() -> Rc<crate::ui::player::Player> {
    crate::ui::ctx().player.clone()
}

fn toast(s: &str) {
    crate::ui::toast(s);
}

fn shuffle_vec<T>(v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = glib::random_int_range(0, i as i32 + 1) as usize;
        v.swap(i, j);
    }
}

impl MusicQueue {
    pub fn new() -> Rc<Self> {
        let s = config::settings();
        Rc::new(Self {
            tracks: RefCell::new(Vec::new()),
            unshuffled: RefCell::new(None),
            index: Cell::new(None),
            continuation: RefCell::new(None),
            playlist_id: RefCell::new(None),
            automix: RefCell::new(None),
            title: RefCell::new(None),
            shuffle: Cell::new(s.music_shuffle),
            repeat: Cell::new(Repeat::from_setting(&s.music_repeat)),
            request: Cell::new(0),
            info_request: Cell::new(0),
            extending: Cell::new(false),
            advance_after_extend: Cell::new(false),
            loading: Cell::new(false),
            errors: Cell::new(0),
            info: RefCell::new(Info::default()),
            listeners: RefCell::new(Vec::new()),
        })
    }

    pub fn connect(&self, f: impl Fn(Change) + 'static) {
        self.listeners.borrow_mut().push(Rc::new(f));
    }

    fn emit(&self, c: Change) {
        let list: Vec<_> = self.listeners.borrow().clone();
        for f in list {
            f(c);
        }
    }

    pub fn tracks(&self) -> Vec<Track> {
        self.tracks.borrow().clone()
    }

    pub fn len(&self) -> usize {
        self.tracks.borrow().len()
    }

    pub fn index(&self) -> Option<usize> {
        self.index.get()
    }

    pub fn current(&self) -> Option<Track> {
        self.index.get().and_then(|i| self.tracks.borrow().get(i).cloned())
    }

    pub fn title(&self) -> Option<String> {
        self.title.borrow().clone()
    }

    pub fn info(&self) -> Info {
        self.info.borrow().clone()
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle.get()
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat.get()
    }

    pub fn is_loading(&self) -> bool {
        self.loading.get()
    }

    pub fn has_next(&self) -> bool {
        let (i, len) = (self.index.get(), self.len());
        i.is_some_and(|i| i + 1 < len) || self.continuation.borrow().is_some() || self.automix.borrow().is_some() || self.repeat.get() == Repeat::All
    }

    /// Plays `target` (an album, playlist, song with its radio…), replacing
    /// the queue.
    pub fn play(self: &Rc<Self>, target: Target) {
        // A lone song plays with its radio, like on music.youtube.com.
        let target = match target {
            Target::Watch { video_id: Some(v), playlist_id: None, params, index } => Target::Watch {
                playlist_id: Some(format!("RDAMVM{v}")),
                params: params.or_else(|| Some("wAEB".into())),
                video_id: Some(v),
                index,
            },
            t => t,
        };
        let req = self.request.get() + 1;
        self.request.set(req);
        self.loading.set(true);
        self.emit(Change::Loading);
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let t = target.clone();
            let res = runtime::run(async move { api::client().music_next(&t).await.map(|v| m::parse_queue(&v)) }).await;
            let Some(q) = wk.upgrade() else { return };
            if q.request.get() != req {
                return;
            }
            q.loading.set(false);
            match res {
                Ok(page) if !page.tracks.is_empty() => q.replace(page),
                Ok(_) => {
                    q.emit(Change::Loading);
                    toast("Nothing to play here");
                }
                Err(e) => {
                    q.emit(Change::Loading);
                    crate::log::warn(&format!("music queue for {target:?} failed: {e:#}"));
                    toast(&format!("Couldn't start playback: {e}"));
                }
            }
        });
    }

    fn replace(self: &Rc<Self>, page: m::Queue) {
        let mut tracks = page.tracks;
        let mut index = page.selected.unwrap_or(0).min(tracks.len() - 1);
        self.unshuffled.replace(None);
        if self.shuffle.get() && tracks.len() > 2 {
            self.unshuffled.replace(Some(tracks.clone()));
            let cur = tracks.remove(index);
            shuffle_vec(&mut tracks);
            tracks.insert(0, cur);
            index = 0;
        }
        self.tracks.replace(tracks);
        self.index.set(Some(index));
        self.continuation.replace(page.continuation);
        self.playlist_id.replace(page.playlist_id);
        self.automix.replace(page.automix);
        self.title.replace(page.title);
        self.errors.set(0);
        let video_id = self.current().map(|t| t.video_id).unwrap_or_default();
        self.info.replace(Info { video_id, lyrics_id: page.lyrics_id, related_id: page.related_id, like: page.like_status, ready: true });
        self.emit(Change::Loading);
        self.emit(Change::Queue);
        // The first track's info came with the queue.
        self.start_current(false);
        self.emit(Change::Info);
    }

    /// Plays the version (song or music video) that matches the Song /
    /// Video switch, when YouTube Music offers both.
    fn prefer_version(&self, video: bool) -> bool {
        let Some(i) = self.index.get() else { return false };
        let mut tracks = self.tracks.borrow_mut();
        let Some(t) = tracks.get_mut(i) else { return false };
        let is_video = t.kind == VideoKind::Video;
        let Some(cp) = t.counterpart.as_ref() else { return false };
        let cp_video = cp.kind == VideoKind::Video;
        if is_video == video || cp_video != video {
            return false;
        }
        let mut new = *t.counterpart.take().unwrap();
        let mut old = std::mem::take(t);
        old.counterpart = None;
        new.counterpart = Some(Box::new(old));
        *t = new;
        true
    }

    fn start_current(self: &Rc<Self>, fetch_info: bool) {
        let p = player();
        self.prefer_version(p.music_video());
        let Some(t) = self.current() else { return };
        p.load_music(t.video_item(), 0.0);
        self.emit(Change::Current);
        if fetch_info {
            self.fetch_info(&t);
        }
        self.extend_if_needed();
    }

    pub fn play_index(self: &Rc<Self>, i: usize) {
        if i >= self.len() {
            return;
        }
        self.index.set(Some(i));
        self.start_current(true);
    }

    /// Like status, lyrics and related for a track picked inside the queue.
    fn fetch_info(self: &Rc<Self>, t: &Track) {
        let req = self.info_request.get() + 1;
        self.info_request.set(req);
        self.info.replace(Info { video_id: t.video_id.clone(), ..Default::default() });
        self.emit(Change::Info);
        let target = Target::Watch {
            video_id: Some(t.video_id.clone()),
            playlist_id: self.playlist_id.borrow().clone().or_else(|| t.playlist_id.clone()),
            params: t.params.clone(),
            index: None,
        };
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { api::client().music_next(&target).await.map(|v| m::parse_queue(&v)) }).await;
            let Some(q) = wk.upgrade() else { return };
            if q.info_request.get() != req {
                return;
            }
            if let Ok(page) = res {
                {
                    let mut info = q.info.borrow_mut();
                    info.lyrics_id = page.lyrics_id;
                    info.related_id = page.related_id;
                    info.like = page.like_status;
                    info.ready = true;
                }
                q.emit(Change::Info);
            }
        });
    }

    /// The user asked for the next track.
    pub fn next(self: &Rc<Self>) {
        self.advance(true);
    }

    /// The current track finished.
    pub fn on_ended(self: &Rc<Self>) {
        if self.repeat.get() == Repeat::One {
            player().replay();
            return;
        }
        self.advance(false);
    }

    fn advance(self: &Rc<Self>, user: bool) {
        let len = self.len();
        let Some(i) = self.index.get() else { return };
        if i + 1 < len {
            self.play_index(i + 1);
        } else if self.continuation.borrow().is_some() {
            self.extend(true);
        } else if self.repeat.get() == Repeat::All && len > 0 {
            self.play_index(0);
        } else if (user || config::settings().autoplay) && self.automix.borrow().is_some() {
            self.extend(true);
        } else if user {
            toast("This is the end of the queue");
        }
    }

    pub fn previous(self: &Rc<Self>) {
        let p = player();
        match self.index.get() {
            Some(i) if i > 0 && p.position() < 3.0 => self.play_index(i - 1),
            _ => p.seek_to(0.0),
        }
    }

    fn extend_if_needed(self: &Rc<Self>) {
        let Some(i) = self.index.get() else { return };
        if i + 3 >= self.len() && self.continuation.borrow().is_some() {
            self.extend(false);
        }
    }

    /// Fetches more tracks: the radio's continuation or, at the end of a
    /// list, autoplay's mix. With `advance`, plays the next track after.
    fn extend(self: &Rc<Self>, advance: bool) {
        if advance {
            self.advance_after_extend.set(true);
        }
        if self.extending.get() {
            return;
        }
        let cont = self.continuation.borrow().clone();
        let pid = self.playlist_id.borrow().clone();
        let automix = if cont.is_none() { self.automix.borrow().clone() } else { None };
        if cont.is_none() && automix.is_none() {
            self.advance_after_extend.set(false);
            return;
        }
        self.extending.set(true);
        let req = self.request.get();
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move {
                let c = api::client();
                match (cont, automix) {
                    (Some(token), _) => {
                        let v = c.music_next_continuation(pid.as_deref(), &token).await?;
                        let (tracks, more, _) = m::parse_queue_more(&v);
                        Ok((tracks, more, None, false))
                    }
                    (None, Some(target)) => {
                        let q = m::parse_queue(&c.music_next(&target).await?);
                        Ok((q.tracks, q.continuation, q.playlist_id, true))
                    }
                    (None, None) => Ok((Vec::new(), None, None, false)),
                }
            })
            .await;
            let Some(q) = wk.upgrade() else { return };
            q.extending.set(false);
            if q.request.get() != req {
                return;
            }
            let advance = q.advance_after_extend.replace(false);
            match res {
                Ok((new, more, new_pid, mix)) => {
                    let current = q.current().map(|t| t.video_id);
                    let new: Vec<Track> = new.into_iter().filter(|t| Some(&t.video_id) != current.as_ref()).collect();
                    q.continuation.replace(more);
                    if mix {
                        // Autoplay went on with a new mix: keep going with it.
                        q.automix.replace(None);
                        if new_pid.is_some() {
                            q.playlist_id.replace(new_pid);
                        }
                    }
                    let added = !new.is_empty();
                    q.tracks.borrow_mut().extend(new.iter().cloned());
                    if let Some(u) = q.unshuffled.borrow_mut().as_mut() {
                        u.extend(new);
                    }
                    q.emit(Change::Queue);
                    if advance && added {
                        if let Some(i) = q.index.get() {
                            q.play_index(i + 1);
                        }
                    } else if advance {
                        toast("This is the end of the queue");
                    }
                }
                Err(e) => {
                    crate::log::warn(&format!("music queue continuation failed: {e:#}"));
                    if advance {
                        toast(&format!("Couldn't load more songs: {e}"));
                    }
                }
            }
        });
    }

    fn insert(self: &Rc<Self>, tracks: Vec<Track>, next: bool) {
        if tracks.is_empty() {
            return;
        }
        let n = tracks.len();
        let at = {
            let mut list = self.tracks.borrow_mut();
            let at = match self.index.get() {
                Some(i) if next => (i + 1).min(list.len()),
                _ => list.len(),
            };
            for (k, t) in tracks.iter().enumerate() {
                list.insert(at + k, t.clone());
            }
            at
        };
        if let Some(u) = self.unshuffled.borrow_mut().as_mut() {
            u.extend(tracks);
        }
        self.emit(Change::Queue);
        if !player().is_music_owned() {
            // Nothing from Music is playing: start with what was added.
            self.play_index(at);
            return;
        }
        let what = if n == 1 { "Song" } else { "Songs" };
        toast(&if next { format!("{what} will play next") } else { format!("{what} added to the queue") });
    }

    pub fn add_next(self: &Rc<Self>, tracks: Vec<Track>) {
        self.insert(tracks, true);
    }

    pub fn add_last(self: &Rc<Self>, tracks: Vec<Track>) {
        self.insert(tracks, false);
    }

    /// Adds what `target` plays (an album, a playlist) to the queue.
    pub fn add_target(self: &Rc<Self>, target: Target, next: bool) {
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { api::client().music_next(&target).await.map(|v| m::parse_queue(&v)) }).await;
            let Some(q) = wk.upgrade() else { return };
            match res {
                Ok(page) if !page.tracks.is_empty() => q.insert(page.tracks, next),
                Ok(_) => toast("Nothing to add"),
                Err(e) => toast(&format!("Couldn't add to the queue: {e}")),
            }
        });
    }

    pub fn remove(self: &Rc<Self>, i: usize) {
        if Some(i) == self.index.get() || i >= self.len() {
            return;
        }
        let removed = self.tracks.borrow_mut().remove(i);
        if let Some(u) = self.unshuffled.borrow_mut().as_mut()
            && let Some(p) = u.iter().position(|t| t.video_id == removed.video_id)
        {
            u.remove(p);
        }
        if let Some(cur) = self.index.get()
            && i < cur
        {
            self.index.set(Some(cur - 1));
        }
        self.emit(Change::Queue);
    }

    /// Moves a queued track to play right after the current one.
    pub fn move_next(self: &Rc<Self>, i: usize) {
        let Some(cur) = self.index.get() else { return };
        if i == cur || i >= self.len() {
            return;
        }
        {
            let mut list = self.tracks.borrow_mut();
            let t = list.remove(i);
            let cur = if i < cur { cur - 1 } else { cur };
            list.insert(cur + 1, t);
            self.index.set(Some(cur));
        }
        self.emit(Change::Queue);
    }

    pub fn set_shuffle(self: &Rc<Self>, on: bool) {
        if self.shuffle.replace(on) == on {
            return;
        }
        config::update(|s| s.music_shuffle = on);
        let cur = self.index.get();
        if on {
            let mut tracks = self.tracks.borrow().clone();
            self.unshuffled.replace(Some(tracks.clone()));
            let from = cur.map(|i| i + 1).unwrap_or(0).min(tracks.len());
            shuffle_vec(&mut tracks[from..]);
            self.tracks.replace(tracks);
        } else if let Some(orig) = self.unshuffled.take() {
            let id = self.current().map(|t| t.video_id);
            let pos = id.and_then(|id| orig.iter().position(|t| t.video_id == id));
            self.tracks.replace(orig);
            if cur.is_some() {
                self.index.set(pos.or(Some(0)));
            }
        }
        self.emit(Change::Queue);
        self.emit(Change::Modes);
    }

    pub fn cycle_repeat(&self) {
        let r = match self.repeat.get() {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
        self.repeat.set(r);
        config::update(|s| s.music_repeat = r.setting().into());
        self.emit(Change::Modes);
    }

    /// The Song / Video switch changed: switch to the other version of the
    /// current track if there is one, at the same position.
    pub fn video_switched(self: &Rc<Self>, video: bool) {
        let p = player();
        if !p.is_music_owned() || !self.prefer_version(video) {
            return;
        }
        let Some(t) = self.current() else { return };
        let pos = p.position();
        p.load_music(t.video_item(), pos);
        self.emit(Change::Queue);
        self.emit(Change::Current);
        self.fetch_info(&t);
    }

    pub fn set_like(self: &Rc<Self>, status: &str) {
        let Some(t) = self.current() else { return };
        if !api::client().is_logged_in() {
            toast("Sign in to rate songs");
            self.emit(Change::Info);
            return;
        }
        let action = match status {
            "LIKE" => LikeAction::Like,
            "DISLIKE" => LikeAction::Dislike,
            _ => LikeAction::Remove,
        };
        self.info.borrow_mut().like = Some(status.to_string());
        self.emit(Change::Info);
        let id = t.video_id.clone();
        let liked = status == "LIKE";
        runtime::spawn_local(async move {
            match runtime::run(async move { api::client().like(&id, action).await }).await {
                Ok(()) if liked => toast("Added to Liked music"),
                Ok(()) => {}
                Err(e) => toast(&format!("Couldn't rate the song: {e}")),
            }
        });
    }

    /// The player couldn't play the current track: skip it.
    pub fn on_error(self: &Rc<Self>) {
        let n = self.errors.get() + 1;
        self.errors.set(n);
        if n > 3 {
            toast("Several songs in a row couldn't be played; stopped");
            return;
        }
        let title = self.current().map(|t| t.title).unwrap_or_default();
        toast(&format!("Skipping “{title}”: it can't be played"));
        let (req, idx) = (self.request.get(), self.index.get());
        let wk = Rc::downgrade(self);
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            if let Some(q) = wk.upgrade()
                && q.request.get() == req
                && q.index.get() == idx
            {
                q.advance(false);
            }
        });
    }

    /// Playback is going fine again.
    pub fn playing_ok(&self) {
        self.errors.set(0);
    }
}
