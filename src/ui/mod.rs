pub mod cards;
pub mod channel;
pub mod comments;
pub mod downloads;
pub mod feed;
pub mod library;
pub mod login;
pub mod mpris;
pub mod music;
pub mod player;
pub mod playlist;
pub mod queue;
pub mod settings;
pub mod sponsorblock;
pub mod subtitles;
pub mod text;
pub mod watch;
pub mod widgets;
pub mod window;

use crate::api::{self, cookies::CookieJar, models::*};
use crate::{config, runtime};
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell};
use std::rc::Rc;

/// Application-wide UI state, only ever touched from the GTK main thread.
pub struct Ctx {
    pub window: adw::ApplicationWindow,
    pub toasts: adw::ToastOverlay,
    pub nav: adw::NavigationView,
    pub player: Rc<player::Player>,
    pub watch: Rc<watch::WatchPage>,
    pub downloads: Rc<downloads::Downloads>,
    pub shell: Rc<window::Shell>,
    pub music: Rc<music::MusicShell>,
}

thread_local! {
    static CTX: OnceCell<Rc<Ctx>> = const { OnceCell::new() };
    static KEEP: Cell<u64> = const { Cell::new(0) };
}

pub fn set_ctx(c: Ctx) {
    CTX.with(|cell| {
        let _ = cell.set(Rc::new(c));
    });
}

pub fn ctx() -> Rc<Ctx> {
    CTX.with(|c| c.get().expect("UI not initialised").clone())
}

pub fn try_ctx() -> Option<Rc<Ctx>> {
    CTX.with(|c| c.get().cloned())
}

pub fn toast(msg: &str) {
    crate::log::info(&format!("toast: {msg}"));
    match try_ctx() {
        Some(c) => {
            let t = adw::Toast::new(&glib::markup_escape_text(msg));
            t.set_timeout(4);
            c.toasts.add_toast(t);
        }
        None => {}
    }
}

/// A toast with a button.
pub fn toast_action(msg: &str, button: &str, f: impl Fn() + 'static) {
    crate::log::info(&format!("toast: {msg}"));
    if let Some(c) = try_ctx() {
        let t = adw::Toast::new(&glib::markup_escape_text(msg));
        t.set_timeout(6);
        t.set_button_label(Some(button));
        t.connect_button_clicked(move |_| f());
        c.toasts.add_toast(t);
    }
}

/// Ties `value`'s lifetime to `obj`.
pub fn keep_alive<T: 'static>(obj: &impl IsA<glib::Object>, value: T) {
    let n = KEEP.with(|k| {
        k.set(k.get() + 1);
        k.get()
    });
    unsafe {
        obj.as_ref().set_data(&format!("gtube-keep-{n}"), value);
    }
}

pub fn open_video(v: VideoItem) {
    ctx().watch.open(v, None, 0.0);
}

pub fn open_channel(id: &str) {
    if id.starts_with("UC") {
        ctx().nav.push(&channel::page(id));
    } else {
        // @handles and custom URLs need resolving first.
        let url = if id.starts_with("http") { id.to_string() } else { format!("https://www.youtube.com/{id}") };
        open_resolved(Resolved::Search(url));
    }
}

pub fn open_playlist(id: &str) {
    if crate::api::models::is_mix(id) {
        return queue::open_mix(None, id.to_string(), 0.0);
    }
    ctx().nav.push(&playlist::page(id));
}

/// Searches (or opens a link) in the mode that is showing; a YouTube Music
/// link switches to Music mode.
pub fn search(q: &str) {
    let c = ctx();
    if api::music::parse_music_url(q).is_some() {
        c.shell.set_mode("music");
        c.music.search(q);
    } else if c.shell.is_music() {
        c.music.search(q);
    } else {
        c.shell.search(q);
    }
}

pub fn open_in_browser(url: &str) {
    gtk::UriLauncher::new(url).launch(Some(&ctx().window), None::<&gio::Cancellable>, |r| {
        if let Err(e) = r {
            toast(&format!("Could not open browser: {e}"));
        }
    });
}

pub fn open_resolved(r: Resolved) {
    match r {
        // Like youtube.com, a watch link with &list= plays the playlist (or
        // mix) as the queue.
        Resolved::Video { id, playlist: Some(list), start } => {
            queue::open_playlist(Some(id), list, start.unwrap_or(0.0), false);
        }
        Resolved::Video { id, start, .. } => {
            let v = VideoItem { id, ..Default::default() };
            ctx().watch.open(v, None, start.unwrap_or(0.0));
        }
        Resolved::Channel(id) => open_channel(&id),
        Resolved::Playlist(id) => open_playlist(&id),
        Resolved::Search(url) => {
            runtime::spawn_local(async move {
                let u = url.clone();
                match runtime::run(async move { api::client().resolve_url(&u).await }).await {
                    Ok(Resolved::Search(_)) | Err(_) => toast(&format!("Could not open {url}")),
                    Ok(r) => open_resolved(r),
                }
            });
        }
    }
}

/// Saves a new session as an account and switches to it.
pub fn add_account(jar: CookieJar) {
    match config::add_account(&jar) {
        Ok(()) => use_active_account("Signed in"),
        Err(e) => toast(&format!("Could not save session: {e}")),
    }
}

/// Switches to a saved account ("" browses without one).
pub fn switch_account(id: &str) {
    if config::settings().active_account == id {
        return;
    }
    config::update(|s| s.active_account = id.into());
    use_active_account(if id.is_empty() { "Browsing without an account" } else { "Switched account" });
}

/// Loads the active account's session and refreshes everything.
pub fn use_active_account(msg: &str) {
    api::client().set_cookies(config::load_cookies());
    toast(msg);
    ctx().shell.start_session();
}

pub fn apply_locale() {
    let s = config::settings();
    api::client().set_locale(s.language, s.region);
    ctx().shell.start_session();
}
