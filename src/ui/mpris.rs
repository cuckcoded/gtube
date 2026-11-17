//! MPRIS: lets the desktop see and control what is playing (GNOME's and
//! KDE's media controls, media keys, playerctl).
//!
//! The objects live on the application's own D-Bus connection, so where
//! there is none (Windows, macOS) this does nothing.

use super::music::Repeat;
use crate::api::parse;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{OnceCell, RefCell};
use std::collections::BTreeMap;

const BUS_NAME: &str = "org.mpris.MediaPlayer2.gtube";
const PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT: &str = "org.mpris.MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";
const NO_TRACK: &str = "/org/mpris/MediaPlayer2/TrackList/NoTrack";

const XML: &str = r#"<node>
  <interface name="org.mpris.MediaPlayer2">
    <method name="Raise"/>
    <method name="Quit"/>
    <property name="CanQuit" type="b" access="read"/>
    <property name="CanRaise" type="b" access="read"/>
    <property name="HasTrackList" type="b" access="read"/>
    <property name="Identity" type="s" access="read"/>
    <property name="DesktopEntry" type="s" access="read"/>
    <property name="SupportedUriSchemes" type="as" access="read"/>
    <property name="SupportedMimeTypes" type="as" access="read"/>
  </interface>
  <interface name="org.mpris.MediaPlayer2.Player">
    <method name="Next"/>
    <method name="Previous"/>
    <method name="Pause"/>
    <method name="PlayPause"/>
    <method name="Stop"/>
    <method name="Play"/>
    <method name="Seek">
      <arg name="Offset" type="x" direction="in"/>
    </method>
    <method name="SetPosition">
      <arg name="TrackId" type="o" direction="in"/>
      <arg name="Position" type="x" direction="in"/>
    </method>
    <method name="OpenUri">
      <arg name="Uri" type="s" direction="in"/>
    </method>
    <signal name="Seeked">
      <arg name="Position" type="x"/>
    </signal>
    <property name="PlaybackStatus" type="s" access="read"/>
    <property name="LoopStatus" type="s" access="readwrite"/>
    <property name="Rate" type="d" access="readwrite"/>
    <property name="Shuffle" type="b" access="readwrite"/>
    <property name="Metadata" type="a{sv}" access="read"/>
    <property name="Volume" type="d" access="readwrite"/>
    <property name="Position" type="x" access="read">
      <annotation name="org.freedesktop.DBus.Property.EmitsChangedSignal" value="false"/>
    </property>
    <property name="MinimumRate" type="d" access="read"/>
    <property name="MaximumRate" type="d" access="read"/>
    <property name="CanGoNext" type="b" access="read"/>
    <property name="CanGoPrevious" type="b" access="read"/>
    <property name="CanPlay" type="b" access="read"/>
    <property name="CanPause" type="b" access="read"/>
    <property name="CanSeek" type="b" access="read"/>
    <property name="CanControl" type="b" access="read"/>
  </interface>
</node>"#;

struct Mpris {
    conn: gio::DBusConnection,
    /// The player properties last announced (all but Position, which
    /// clients ask for).
    sent: RefCell<BTreeMap<&'static str, glib::Variant>>,
}

thread_local! {
    static MPRIS: OnceCell<Mpris> = const { OnceCell::new() };
}

/// Puts the player on the session bus (once the UI exists).
pub fn start(app: &adw::Application) {
    let Some(conn) = app.dbus_connection() else { return };
    if MPRIS.with(|m| m.get().is_some()) {
        return;
    }
    let node = match gio::DBusNodeInfo::for_xml(XML) {
        Ok(n) => n,
        Err(e) => return crate::log::warn(&format!("MPRIS: {e}")),
    };
    for name in [ROOT, PLAYER] {
        let Some(info) = node.lookup_interface(name) else { return };
        let registered = conn
            .register_object(PATH, &info)
            .method_call(|_, _, _, _, method, params, invocation| call(method, &params, invocation))
            .property(|_, _, _, iface, prop| get(iface, prop))
            .set_property(|_, _, _, _, prop, value| set(prop, &value))
            .build();
        if let Err(e) = registered {
            return crate::log::warn(&format!("MPRIS: {e}"));
        }
    }
    gio::bus_own_name_on_connection(
        &conn,
        BUS_NAME,
        gio::BusNameOwnerFlags::NONE,
        |_, _| {},
        |_, name| crate::log::warn(&format!("MPRIS: could not own {name}")),
    );
    MPRIS.with(|m| {
        let _ = m.set(Mpris { conn, sent: RefCell::new(player_props()) });
    });
    // Shuffle / repeat, and what can be skipped to.
    super::ctx().music.queue.connect(|_| update());
}

/// Announces the player properties that changed (call whenever the player's
/// state may have).
pub fn update() {
    MPRIS.with(|m| {
        let Some(m) = m.get() else { return };
        let changed: BTreeMap<String, glib::Variant> = {
            let mut sent = m.sent.borrow_mut();
            player_props()
                .into_iter()
                .filter_map(|(k, v)| {
                    let same = sent.get(k) == Some(&v);
                    sent.insert(k, v.clone());
                    (!same).then(|| (k.to_string(), v))
                })
                .collect()
        };
        if !changed.is_empty() {
            let args = (PLAYER.to_string(), changed, Vec::<String>::new()).to_variant();
            emit(&m.conn, "org.freedesktop.DBus.Properties", "PropertiesChanged", &args);
        }
    });
}

/// Playback continues from `position` (seconds) after a seek or a new start.
pub fn seeked(position: f64) {
    MPRIS.with(|m| {
        if let Some(m) = m.get() {
            update();
            emit(&m.conn, PLAYER, "Seeked", &(micros(position),).to_variant());
        }
    });
}

fn emit(conn: &gio::DBusConnection, iface: &str, signal: &str, args: &glib::Variant) {
    if let Err(e) = conn.emit_signal(None, PATH, iface, signal, Some(args)) {
        crate::log::warn(&format!("MPRIS: {signal}: {e}"));
    }
}

fn micros(seconds: f64) -> i64 {
    (seconds.max(0.0) * 1_000_000.0) as i64
}

/// MPRIS track IDs are object paths, which only allow `[A-Za-z0-9_]`.
fn track_id(video_id: &str) -> String {
    let mut path = String::from("/io/github/gtube/track/v");
    for c in video_id.chars() {
        if c.is_ascii_alphanumeric() {
            path.push(c);
        } else {
            path.push_str(&format!("_{:02x}", c as u32));
        }
    }
    path
}

fn object_path(path: &str) -> glib::Variant {
    glib::variant::ObjectPath::try_from(path)
        .unwrap_or_else(|_| glib::variant::ObjectPath::try_from(NO_TRACK).unwrap())
        .to_variant()
}

fn metadata(c: &super::Ctx) -> glib::Variant {
    let mut m: BTreeMap<String, glib::Variant> = BTreeMap::new();
    let p = &c.player;
    let Some(v) = p.current() else {
        m.insert("mpris:trackid".into(), object_path(NO_TRACK));
        return m.to_variant();
    };
    m.insert("mpris:trackid".into(), object_path(&track_id(&v.id)));
    m.insert("xesam:title".into(), v.title.to_variant());
    m.insert("xesam:url".into(), v.url().to_variant());
    m.insert("mpris:artUrl".into(), v.thumbnail_url().to_variant());
    if p.duration() > 0.0 {
        m.insert("mpris:length".into(), micros(p.duration()).to_variant());
    }
    // Songs have their artists and album in the queue (also while their
    // music video plays).
    let track = p
        .is_music_owned()
        .then(|| c.music.queue.current())
        .flatten()
        .filter(|t| t.video_id == v.id || t.counterpart.as_ref().is_some_and(|o| o.video_id == v.id));
    let artists: Vec<String> = match &track {
        Some(t) if !t.artists.is_empty() => t.artists.iter().map(|a| a.text.clone()).collect(),
        _ => v.channel_name.iter().cloned().collect(),
    };
    if !artists.is_empty() {
        m.insert("xesam:artist".into(), artists.to_variant());
    }
    if let Some(album) = track.and_then(|t| t.album) {
        m.insert("xesam:album".into(), album.text.to_variant());
    }
    m.to_variant()
}

/// The player interface's properties, but Position.
fn player_props() -> BTreeMap<&'static str, glib::Variant> {
    let mut m = BTreeMap::new();
    let Some(c) = super::try_ctx() else { return m };
    let p = &c.player;
    let loaded = p.current_id().is_some();
    let music = p.is_music_owned();
    let status = if !loaded || p.has_failed() {
        "Stopped"
    } else if p.is_paused() {
        "Paused"
    } else {
        "Playing"
    };
    let repeat = if music { c.music.queue.repeat() } else { Repeat::Off };
    let loop_status = match repeat {
        Repeat::Off => "None",
        Repeat::All => "Playlist",
        Repeat::One => "Track",
    };
    let next = loaded && if music { c.music.queue.has_next() } else { p.can_skip_next() };
    let props: [(&'static str, glib::Variant); 14] = [
        ("PlaybackStatus", status.to_variant()),
        ("LoopStatus", loop_status.to_variant()),
        ("Rate", p.speed().to_variant()),
        ("Shuffle", (music && c.music.queue.shuffle()).to_variant()),
        ("Metadata", metadata(&c)),
        ("Volume", (p.volume() / 100.0).to_variant()),
        ("MinimumRate", super::player::Player::MIN_SPEED.to_variant()),
        ("MaximumRate", super::player::Player::MAX_SPEED.to_variant()),
        ("CanGoNext", next.to_variant()),
        // Previous goes back to the start when there is nothing before.
        ("CanGoPrevious", loaded.to_variant()),
        ("CanPlay", loaded.to_variant()),
        ("CanPause", loaded.to_variant()),
        ("CanSeek", (loaded && p.duration() > 0.0).to_variant()),
        ("CanControl", true.to_variant()),
    ];
    m.extend(props);
    m
}

fn get(iface: &str, prop: &str) -> glib::Variant {
    if iface == ROOT {
        return match prop {
            "CanQuit" | "CanRaise" => true.to_variant(),
            "HasTrackList" => false.to_variant(),
            "Identity" => "GTube".to_variant(),
            "DesktopEntry" => "io.github.gtube".to_variant(),
            "SupportedUriSchemes" => vec!["https".to_string(), "http".to_string()].to_variant(),
            _ => Vec::<String>::new().to_variant(),
        };
    }
    if prop == "Position" {
        let pos = super::try_ctx().map_or(0.0, |c| c.player.position());
        return micros(pos).to_variant();
    }
    player_props().remove(prop).unwrap_or_else(|| false.to_variant())
}

fn set(prop: &str, value: &glib::Variant) -> bool {
    let Some(c) = super::try_ctx() else { return false };
    let p = c.player.clone();
    let music = p.is_music_owned();
    match prop {
        "Volume" => {
            if let Some(v) = value.get::<f64>() {
                p.set_volume(v.max(0.0) * 100.0);
            }
        }
        "Rate" => match value.get::<f64>() {
            Some(r) if r <= 0.0 => {
                if !p.is_paused() {
                    p.toggle_pause();
                }
            }
            Some(r) => p.set_speed(r),
            None => {}
        },
        "Shuffle" if music => {
            if let Some(on) = value.get::<bool>() {
                c.music.queue.set_shuffle(on);
            }
        }
        "LoopStatus" if music => {
            let want = match value.str() {
                Some("Playlist") => Repeat::All,
                Some("Track") => Repeat::One,
                _ => Repeat::Off,
            };
            // Off → All → One → Off
            for _ in 0..3 {
                if c.music.queue.repeat() == want {
                    break;
                }
                c.music.queue.cycle_repeat();
            }
        }
        _ => {}
    }
    update();
    true
}

/// Next / previous in whichever queue is playing.
fn win_action(c: &super::Ctx, name: &str) {
    gio::prelude::ActionGroupExt::activate_action(&c.window, name, None);
}

fn call(method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
    let Some(c) = super::try_ctx() else { return invocation.return_value(None) };
    let p = c.player.clone();
    let loaded = p.current_id().is_some();
    match method {
        "Raise" => c.window.present(),
        "Quit" => {
            if let Some(app) = c.window.application() {
                app.activate_action("quit", None);
            }
        }
        "Next" if loaded => {
            win_action(&c, "next");
        }
        "Previous" if loaded => {
            win_action(&c, "previous");
        }
        "PlayPause" if loaded => p.toggle_pause(),
        "Play" if loaded && p.is_paused() => p.toggle_pause(),
        "Pause" if loaded && !p.is_paused() => p.toggle_pause(),
        "Stop" if loaded => p.stop(),
        "Seek" if loaded && p.duration() > 0.0 => {
            if let Some((offset,)) = params.get::<(i64,)>() {
                let to = p.position() + offset as f64 / 1_000_000.0;
                if to >= p.duration() {
                    win_action(&c, "next");
                } else {
                    p.seek_to(to.max(0.0));
                }
            }
        }
        "SetPosition" if loaded && p.duration() > 0.0 => {
            if let Some((track, pos)) = params.get::<(glib::variant::ObjectPath, i64)>() {
                let current = p.current_id().map(|id| track_id(&id));
                let t = pos as f64 / 1_000_000.0;
                if current.as_deref() == Some(track.as_str()) && (0.0..=p.duration()).contains(&t) {
                    p.seek_to(t);
                }
            }
        }
        "OpenUri" => {
            let uri = params.get::<(String,)>().map(|(u,)| u).unwrap_or_default();
            if parse::parse_youtube_url(&uri).is_none() && crate::api::music::parse_music_url(&uri).is_none() {
                return invocation.return_dbus_error("org.freedesktop.DBus.Error.InvalidArgs", "Not a YouTube link");
            }
            super::search(&uri);
            c.window.present();
        }
        _ => {}
    }
    invocation.return_value(None);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_ids_are_object_paths() {
        let id = track_id("dQw4w9-gX_Q");
        assert_eq!(id, "/io/github/gtube/track/vdQw4w9_2dgX_5fQ");
        assert!(glib::variant::ObjectPath::try_from(id.as_str()).is_ok());
        assert_eq!(micros(1.5), 1_500_000);
    }
}
