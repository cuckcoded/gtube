//! The video player: mpv rendering into a GtkGLArea, plus YouTube-style
//! controls, SponsorBlock skipping, subtitles, audio-only mode and the
//! miniplayer.
//!
//! The player widget lives in an overlay above the whole content area and is
//! never reparented (which would destroy the GL context). Instead it is
//! positioned on top of the watch page's placeholder when that page is
//! visible, shrinks to a corner miniplayer otherwise, or covers everything
//! in fullscreen.

use super::subtitles::{self, Choice};
use super::widgets::{PlayerHost, SeekBar, format_time};
use crate::api::{self, extras, extras::Segment, models::VideoItem, stats};
use crate::config::{self, SbMode};
use crate::extract;
use crate::mpv::{self, Event, Format, Mpv};
use crate::runtime;
use gtk::{gdk, gio, glib, graphene, prelude::*};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Hidden,
    Docked,
    Mini,
    Fullscreen,
    /// The whole (small) window is the player.
    Compact,
}

struct SegState {
    seg: Segment,
    mode: SbMode,
    ignored: Cell<bool>,
}

type Callback = RefCell<Option<Box<dyn Fn()>>>;

pub struct Player {
    pub host: PlayerHost,
    mpv: Mpv,
    area: gtk::GLArea,
    fb_size: Cell<(i32, i32)>,
    bottom: gtk::Revealer,
    top: gtk::Revealer,
    title: gtk::Label,
    mini_controls: gtk::Box,
    mini_play: gtk::Button,
    mini_progress: gtk::ProgressBar,
    play_btn: gtk::Button,
    prev_btn: gtk::Button,
    next_btn: gtk::Button,
    time_label: gtk::Label,
    seek: SeekBar,
    mute_btn: gtk::Button,
    volume: gtk::Scale,
    audio_btn: gtk::ToggleButton,
    fs_btn: gtk::Button,
    speed_btn: gtk::MenuButton,
    quality_btn: gtk::MenuButton,
    subs_btn: gtk::MenuButton,
    /// `player.subtitles`: the subtitles menu's choice.
    subs_action: gio::SimpleAction,
    highlight_btn: gtk::Button,
    compact_btn: gtk::Button,
    mini_btn: gtk::Button,
    /// Controls hidden when the player gets narrow, with the width they
    /// need to be shown.
    optional: Vec<(gtk::Widget, i32)>,
    ctl_width: Cell<i32>,
    /// What prev / next / "Jump to highlight" would show if there's room.
    want_prev: Cell<bool>,
    want_next: Cell<bool>,
    want_highlight: Cell<bool>,
    spinner: adw::Spinner,
    wait_box: gtk::Box,
    wait_title: gtk::Label,
    error_box: gtk::Box,
    error_label: gtk::Label,
    cover: gtk::Box,
    cover_pic: gtk::Picture,
    notice: gtk::Revealer,
    notice_label: gtk::Label,
    notice_undo: gtk::Button,

    current: RefCell<Option<VideoItem>>,
    mode: Cell<Mode>,
    fullscreen: Cell<bool>,
    /// The window is shrunk to just the player.
    compact: Cell<bool>,
    /// Music's immersive view is up: it hosts the video itself.
    immersive: Cell<bool>,
    force_mini: Cell<bool>,
    root: RefCell<Option<glib::WeakRef<gtk::Overlay>>>,
    /// Placeholders the player covers while one of them is on screen (the
    /// watch page's, and Music's "Now playing" video).
    docks: RefCell<Vec<glib::WeakRef<gtk::Widget>>>,
    /// Music mode is showing: no miniplayer, the player is only visible
    /// docked on "Now playing" (or fullscreen) and otherwise plays in the
    /// background.
    music: Cell<bool>,
    /// Music mode's "Video" (rather than "Song") choice.
    music_video: Cell<bool>,
    /// The current item was started from Music mode.
    music_owned: Cell<bool>,
    /// The current stream was opened with a video track.
    loaded_with_video: Cell<bool>,
    /// mpv has the current file open (subtitles can be added to it).
    file_ready: Cell<bool>,
    /// The current item's subtitles.
    captions: RefCell<extract::Captions>,
    /// What to show of them; None until they are known for a new item.
    sub_choice: RefCell<Option<Choice>>,
    /// Subtitle tracks added to the open file: (URL, mpv track id, choice).
    sub_tracks: RefCell<Vec<(String, i64, Choice)>>,
    /// Bumped whenever the subtitles to show change.
    sub_gen: Cell<u64>,
    /// mpv's `af` (the visualizer's analysis filter, or nothing).
    audio_filter: RefCell<String>,
    seg_bar: RefCell<Vec<(f64, f64, gdk::RGBA, String)>>,
    rect: Cell<(f32, f32, f32, f32)>,
    anim: Cell<Option<((f32, f32, f32, f32), i64)>>,
    tick: RefCell<Option<gtk::TickCallbackId>>,
    duration: Cell<f64>,
    position: Cell<f64>,
    buffered: Cell<f64>,
    paused: Cell<bool>,
    muted: Cell<bool>,
    segments: RefCell<Vec<SegState>>,
    sb_seeking: Cell<bool>,
    sb_muted: Cell<bool>,
    undo_target: Cell<Option<usize>>,
    generation: Cell<u64>,
    /// What is reported to YouTube about the current view.
    report: RefCell<Option<stats::Report>>,
    /// Where YouTube says to continue the current item, until it starts.
    resume_at: Cell<Option<f64>>,
    /// The current item's streams were handed to mpv.
    item_started: Cell<bool>,
    speed: Cell<f64>,
    /// Bumped for every (re)load so stale yt-dlp results are dropped.
    load_gen: Cell<u64>,
    pending: RefCell<Option<(String, f64)>>,
    gl_failed: Cell<bool>,
    last_error: RefCell<String>,
    error_important: Cell<bool>,
    hide_source: RefCell<Option<glib::SourceId>>,
    notice_source: RefCell<Option<glib::SourceId>>,
    volume_save: RefCell<Option<glib::SourceId>>,
    quality: Cell<u32>,
    updating: Cell<bool>,
    eof_handled: Cell<bool>,
    pub on_ended: Callback,
    pub on_expand: Callback,
    pub on_minimize: Callback,
    pub on_download: Callback,
    pub on_segments: RefCell<Option<Box<dyn Fn(&[Segment])>>>,
    /// Position, duration, pause, loading, volume or segments changed.
    pub on_state: Callback,
    pub on_error: RefCell<Option<Box<dyn Fn(&str)>>>,
    /// Music mode's Song / Video choice changed.
    pub on_music_video: RefCell<Option<Box<dyn Fn(bool)>>>,
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tooltip));
    b.add_css_class("flat");
    b.set_valign(gtk::Align::Center);
    b
}

fn lerp(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32), t: f32) -> (f32, f32, f32, f32) {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t, a.3 + (b.3 - a.3) * t)
}

const ANIM_US: i64 = 260_000;

/// Turns mpv / yt-dlp log lines into something a person can act on.
fn describe_error(raw: &str) -> String {
    static URL_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"https?://\S+").unwrap());
    let msg = raw.trim().trim_start_matches("ERROR: ");
    let msg = msg.strip_prefix("[youtube] ").unwrap_or(msg);
    if msg.contains("Sign in to confirm") || msg.contains("not a bot") {
        return "YouTube wants a signed-in session before it serves this video.\nSign in from the account menu, then retry.".into();
    }
    if msg.contains("HTTP error 403") {
        return "YouTube refused the video stream (HTTP 403).\nThis is usually temporary: retry, and if it keeps happening update yt-dlp.".into();
    }
    if msg.contains("HTTP error 429") {
        return "YouTube is rate-limiting this connection (HTTP 429).\nWait a little or sign in, then retry.".into();
    }
    if msg.contains("Private video") || msg.contains("members-only") || msg.contains("age") && msg.contains("confirm") {
        return format!("{}\nSigning in may give access.", URL_RE.replace_all(msg, "…"));
    }
    let s = URL_RE.replace_all(msg, "…").into_owned();
    if s.chars().count() > 300 { format!("{}…", s.chars().take(300).collect::<String>()) } else { s }
}

impl Player {
    pub fn new() -> Rc<Self> {
        let s = config::settings();
        let ytdlp = extract::find_ytdlp();
        let mut opts: Vec<(&str, String)> = vec![
            ("vo", "libmpv".into()),
            ("hwdec", if s.hwdec { "auto-safe" } else { "no" }.into()),
            // Extraction is done by us (see extract.rs), not by ytdl_hook.
            ("ytdl", "no".into()),
            ("keep-open", "yes".into()),
            ("idle", "yes".into()),
            ("osc", "no".into()),
            ("input-default-bindings", "no".into()),
            ("input-vo-keyboard", "no".into()),
            ("terminal", "no".into()),
            ("cache", "yes".into()),
            ("demuxer-max-bytes", "150MiB".into()),
            ("demuxer-max-back-bytes", "50MiB".into()),
            ("audio-client-name", "GTube".into()),
            ("volume", format!("{}", s.volume)),
            ("sub-auto", "no".into()),
            ("sub-scale", format!("{}", s.subtitle_scale)),
            // We wait for `available_at` ourselves; this only covers clock
            // skew and short network hiccups.
            ("stream-lavf-o", "reconnect_on_http_error=403,reconnect_on_network_error=1".into()),
        ];
        // Audio output override (e.g. GTUBE_MPV_AO=null for headless tests).
        if let Ok(ao) = std::env::var("GTUBE_MPV_AO") {
            opts.push(("ao", ao));
        }

        let (mpv, wake_rx, redraw_rx) = Mpv::new(&opts).unwrap_or_else(|e| panic!("could not start mpv: {e}"));
        for (name, f) in [
            ("time-pos", Format::Double),
            ("duration", Format::Double),
            ("pause", Format::Flag),
            ("paused-for-cache", Format::Flag),
            ("seeking", Format::Flag),
            ("demuxer-cache-time", Format::Double),
            ("volume", Format::Double),
            ("mute", Format::Flag),
            ("eof-reached", Format::Flag),
            ("speed", Format::Double),
        ] {
            mpv.observe(name, f);
        }

        // --- widgets
        let area = gtk::GLArea::new();
        area.set_auto_render(false);
        area.set_has_depth_buffer(false);
        area.set_has_stencil_buffer(false);
        area.set_hexpand(true);
        area.set_vexpand(true);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&area));
        overlay.add_css_class("player");
        overlay.set_overflow(gtk::Overflow::Hidden);

        // Audio-only cover
        let cover_pic = gtk::Picture::new();
        cover_pic.set_content_fit(gtk::ContentFit::Contain);
        cover_pic.set_can_shrink(true);
        cover_pic.set_vexpand(true);
        let cover_label = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cover_label.set_halign(gtk::Align::Center);
        cover_label.append(&gtk::Image::from_icon_name("audio-headphones-symbolic"));
        cover_label.append(&gtk::Label::new(Some("Audio only")));
        cover_label.add_css_class("cover-label");
        let cover = gtk::Box::new(gtk::Orientation::Vertical, 8);
        cover.add_css_class("audio-cover");
        cover.append(&cover_pic);
        cover.append(&cover_label);
        cover.set_visible(false);
        cover.set_can_target(false);
        overlay.add_overlay(&cover);

        let spinner = adw::Spinner::new();
        spinner.set_size_request(48, 48);
        spinner.set_halign(gtk::Align::Center);
        spinner.set_valign(gtk::Align::Center);
        spinner.set_can_target(false);
        spinner.set_visible(false);
        overlay.add_overlay(&spinner);

        // Shown while YouTube holds the streams back for a blocked ad.
        let wait_title = gtk::Label::new(None);
        wait_title.add_css_class("heading");
        let wait_sub = gtk::Label::new(Some("YouTube delays the video by the length of the ad you're not seeing"));
        wait_sub.add_css_class("caption");
        wait_sub.set_wrap(true);
        wait_sub.set_justify(gtk::Justification::Center);
        wait_sub.set_max_width_chars(40);
        let wait_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        wait_box.append(&wait_title);
        wait_box.append(&wait_sub);
        wait_box.add_css_class("player-wait");
        wait_box.set_halign(gtk::Align::Center);
        wait_box.set_valign(gtk::Align::End);
        wait_box.set_margin_bottom(24);
        wait_box.set_margin_start(12);
        wait_box.set_margin_end(12);
        wait_box.set_can_target(false);
        wait_box.set_visible(false);
        overlay.add_overlay(&wait_box);

        let error_label = gtk::Label::new(None);
        error_label.set_wrap(true);
        error_label.set_justify(gtk::Justification::Center);
        error_label.set_max_width_chars(60);
        error_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        error_label.set_lines(6);
        error_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        error_label.set_selectable(true);
        let retry = gtk::Button::with_label("Retry");
        retry.add_css_class("pill");
        retry.set_halign(gtk::Align::Center);
        let error_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        error_box.set_valign(gtk::Align::Center);
        error_box.set_halign(gtk::Align::Center);
        error_box.set_margin_start(24);
        error_box.set_margin_end(24);
        error_box.add_css_class("player-error");
        let err_icon = gtk::Image::from_icon_name("dialog-error-symbolic");
        err_icon.set_pixel_size(48);
        error_box.append(&err_icon);
        error_box.append(&error_label);
        error_box.append(&retry);
        error_box.set_visible(false);
        overlay.add_overlay(&error_box);

        // Top bar (title, shown in fullscreen)
        let title = gtk::Label::new(None);
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("player-title");
        let top = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .valign(gtk::Align::Start)
            .child(&title)
            .reveal_child(false)
            .visible(false)
            .build();
        top.add_css_class("player-top");
        overlay.add_overlay(&top);

        // Bottom controls
        let seek = SeekBar::new();
        seek.set_margin_start(12);
        seek.set_margin_end(12);
        let play_btn = icon_button("media-playback-pause-symbolic", "Pause (k)");
        let prev_btn = icon_button("media-skip-backward-symbolic", "Previous (Shift+P)");
        prev_btn.set_action_name(Some("win.previous"));
        prev_btn.set_visible(false);
        let next_btn = icon_button("media-skip-forward-symbolic", "Next (Shift+N)");
        next_btn.set_action_name(Some("win.next"));
        next_btn.set_visible(false);
        let mute_btn = icon_button("audio-volume-high-symbolic", "Mute (m)");
        let volume = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 130.0, 1.0);
        volume.set_width_request(90);
        volume.set_valign(gtk::Align::Center);
        volume.set_value(s.volume);
        volume.add_mark(100.0, gtk::PositionType::Bottom, None);
        let time_label = gtk::Label::new(Some("0:00 / 0:00"));
        time_label.add_css_class("numeric");
        time_label.set_margin_start(8);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let highlight_btn = gtk::Button::with_label("Jump to highlight");
        highlight_btn.add_css_class("flat");
        highlight_btn.add_css_class("highlight-btn");
        highlight_btn.set_visible(false);
        let audio_btn = gtk::ToggleButton::new();
        audio_btn.set_icon_name("audio-headphones-symbolic");
        audio_btn.set_tooltip_text(Some("Audio only"));
        audio_btn.add_css_class("flat");
        audio_btn.set_active(s.audio_only);
        audio_btn.set_valign(gtk::Align::Center);

        let speed_menu = gio::Menu::new();
        for sp in ["0.25", "0.5", "0.75", "1", "1.25", "1.5", "1.75", "2", "3"] {
            let label = if sp == "1" { "Normal".to_string() } else { format!("{sp}×") };
            speed_menu.append(Some(&label), Some(&format!("player.speed::{sp}")));
        }
        let speed_btn = gtk::MenuButton::builder().label("1×").menu_model(&speed_menu).tooltip_text("Playback speed").build();
        speed_btn.add_css_class("flat");
        speed_btn.set_valign(gtk::Align::Center);
        speed_btn.set_direction(gtk::ArrowType::Up);

        let quality_menu = gio::Menu::new();
        for q in ["2160", "1440", "1080", "720", "480", "360", "240", "144"] {
            quality_menu.append(Some(&format!("{q}p")), Some(&format!("player.quality::{q}")));
        }
        let quality_btn = gtk::MenuButton::builder()
            .label(format!("{}p", s.max_quality))
            .menu_model(&quality_menu)
            .tooltip_text("Quality")
            .build();
        quality_btn.add_css_class("flat");
        quality_btn.set_valign(gtk::Align::Center);
        quality_btn.set_direction(gtk::ArrowType::Up);

        let subs_btn = gtk::MenuButton::builder().icon_name("media-view-subtitles-symbolic").tooltip_text("No subtitles").build();
        subs_btn.add_css_class("flat");
        subs_btn.set_valign(gtk::Align::Center);
        subs_btn.set_direction(gtk::ArrowType::Up);
        subs_btn.set_sensitive(false);
        let subs_action = gio::SimpleAction::new_stateful("subtitles", Some(glib::VariantTy::STRING), &"off".to_variant());

        let download_btn = icon_button("folder-download-symbolic", "Download");
        let mini_btn = icon_button("gtube-miniplayer-symbolic", "Miniplayer (i)");
        let compact_btn = icon_button("gtube-compact-symbolic", "Compact player (Ctrl+Shift+M)");
        compact_btn.set_action_name(Some("win.compact"));
        let sb_btn = icon_button("gtube-shield-symbolic", "Mark a SponsorBlock segment (;)");
        sb_btn.set_action_name(Some("win.sb-mark"));
        let fs_btn = icon_button("view-fullscreen-symbolic", "Fullscreen (f)");

        let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        row.set_margin_start(8);
        row.set_margin_end(8);
        row.set_margin_bottom(6);
        for w in [
            prev_btn.upcast_ref::<gtk::Widget>(),
            play_btn.upcast_ref(),
            next_btn.upcast_ref(),
            mute_btn.upcast_ref(),
            volume.upcast_ref(),
            time_label.upcast_ref(),
            spacer.upcast_ref(),
            highlight_btn.upcast_ref(),
            sb_btn.upcast_ref(),
            subs_btn.upcast_ref(),
            audio_btn.upcast_ref(),
            speed_btn.upcast_ref(),
            quality_btn.upcast_ref(),
            download_btn.upcast_ref(),
            mini_btn.upcast_ref(),
            compact_btn.upcast_ref(),
            fs_btn.upcast_ref(),
        ] {
            row.append(w);
        }
        let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bar.add_css_class("player-bar");
        bar.append(&seek);
        bar.append(&row);
        let bottom = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .valign(gtk::Align::End)
            .child(&bar)
            .reveal_child(true)
            .build();
        overlay.add_overlay(&bottom);

        // Skip notice
        let notice_label = gtk::Label::new(None);
        let notice_undo = gtk::Button::with_label("Undo");
        notice_undo.add_css_class("flat");
        let notice_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        notice_box.add_css_class("sb-notice");
        notice_box.append(&gtk::Image::from_icon_name("gtube-shield-symbolic"));
        notice_box.append(&notice_label);
        notice_box.append(&notice_undo);
        let notice = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideLeft)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_bottom(80)
            .margin_end(16)
            .child(&notice_box)
            .build();
        overlay.add_overlay(&notice);

        // Mini controls
        let mini_play = gtk::Button::from_icon_name("media-playback-pause-symbolic");
        mini_play.add_css_class("circular");
        mini_play.add_css_class("osd");
        mini_play.add_css_class("mini-play");
        mini_play.set_halign(gtk::Align::Center);
        mini_play.set_valign(gtk::Align::Center);
        let expand_btn = icon_button("view-restore-symbolic", "Expand");
        expand_btn.add_css_class("osd");
        expand_btn.set_halign(gtk::Align::Start);
        expand_btn.set_valign(gtk::Align::Start);
        let close_btn = icon_button("window-close-symbolic", "Close");
        close_btn.add_css_class("osd");
        close_btn.set_halign(gtk::Align::End);
        close_btn.set_valign(gtk::Align::Start);
        let mini_progress = gtk::ProgressBar::new();
        mini_progress.add_css_class("mini-progress");
        mini_progress.set_valign(gtk::Align::End);
        let mini_overlay = gtk::Overlay::new();
        let mini_fill = gtk::Box::new(gtk::Orientation::Vertical, 0);
        mini_fill.set_hexpand(true);
        mini_fill.set_vexpand(true);
        mini_overlay.set_child(Some(&mini_fill));
        let mini_click = gtk::GestureClick::new();
        mini_fill.add_controller(mini_click.clone());
        for w in [mini_play.upcast_ref::<gtk::Widget>(), expand_btn.upcast_ref(), close_btn.upcast_ref()] {
            mini_overlay.add_overlay(w);
        }
        expand_btn.set_margin_start(6);
        expand_btn.set_margin_top(6);
        close_btn.set_margin_end(6);
        close_btn.set_margin_top(6);
        let mini_controls = gtk::Box::new(gtk::Orientation::Vertical, 0);
        mini_controls.add_css_class("mini-controls");
        mini_controls.append(&mini_overlay);
        mini_controls.set_visible(false);
        overlay.add_overlay(&mini_controls);
        overlay.add_overlay(&mini_progress);
        mini_progress.set_visible(false);

        // Narrower players drop these first.
        let optional: Vec<(gtk::Widget, i32)> = vec![
            (download_btn.clone().upcast(), 760),
            (quality_btn.clone().upcast(), 680),
            (sb_btn.upcast(), 620),
            (speed_btn.clone().upcast(), 580),
            (audio_btn.clone().upcast(), 540),
            (volume.clone().upcast(), 500),
            (subs_btn.clone().upcast(), 420),
            (compact_btn.clone().upcast(), 360),
        ];

        let host = PlayerHost::new(&overlay);
        host.set_visible(false);
        host.add_css_class("player-host");

        let this = Rc::new(Self {
            host,
            mpv,
            area,
            fb_size: Cell::new((0, 0)),
            bottom,
            top,
            title,
            mini_controls,
            mini_play,
            mini_progress,
            play_btn,
            prev_btn,
            next_btn,
            time_label,
            seek,
            mute_btn,
            volume,
            audio_btn,
            fs_btn,
            speed_btn,
            quality_btn,
            subs_btn,
            subs_action,
            highlight_btn,
            compact_btn,
            mini_btn: mini_btn.clone(),
            optional,
            ctl_width: Cell::new(0),
            want_prev: Cell::new(false),
            want_next: Cell::new(false),
            want_highlight: Cell::new(false),
            spinner,
            wait_box,
            wait_title,
            error_box,
            error_label,
            cover,
            cover_pic,
            notice,
            notice_label,
            notice_undo,
            current: RefCell::new(None),
            mode: Cell::new(Mode::Hidden),
            fullscreen: Cell::new(false),
            compact: Cell::new(false),
            immersive: Cell::new(false),
            force_mini: Cell::new(false),
            root: RefCell::new(None),
            docks: RefCell::new(Vec::new()),
            music: Cell::new(false),
            music_video: Cell::new(config::settings().music_video),
            music_owned: Cell::new(false),
            loaded_with_video: Cell::new(false),
            file_ready: Cell::new(false),
            captions: RefCell::new(extract::Captions::default()),
            sub_choice: RefCell::new(None),
            sub_tracks: RefCell::new(Vec::new()),
            sub_gen: Cell::new(0),
            audio_filter: RefCell::new(String::new()),
            seg_bar: RefCell::new(Vec::new()),
            rect: Cell::new((0.0, 0.0, 1.0, 1.0)),
            anim: Cell::new(None),
            tick: RefCell::new(None),
            duration: Cell::new(0.0),
            position: Cell::new(0.0),
            buffered: Cell::new(0.0),
            paused: Cell::new(false),
            muted: Cell::new(false),
            segments: RefCell::new(Vec::new()),
            sb_seeking: Cell::new(false),
            sb_muted: Cell::new(false),
            undo_target: Cell::new(None),
            generation: Cell::new(0),
            report: RefCell::new(None),
            resume_at: Cell::new(None),
            item_started: Cell::new(false),
            speed: Cell::new(1.0),
            load_gen: Cell::new(0),
            pending: RefCell::new(None),
            gl_failed: Cell::new(false),
            last_error: RefCell::new(String::new()),
            error_important: Cell::new(false),
            hide_source: RefCell::new(None),
            notice_source: RefCell::new(None),
            volume_save: RefCell::new(None),
            quality: Cell::new(s.max_quality),
            updating: Cell::new(false),
            eof_handled: Cell::new(false),
            on_ended: RefCell::new(None),
            on_expand: RefCell::new(None),
            on_minimize: RefCell::new(None),
            on_download: RefCell::new(None),
            on_segments: RefCell::new(None),
            on_state: RefCell::new(None),
            on_error: RefCell::new(None),
            on_music_video: RefCell::new(None),
        });

        this.connect_signals(retry, download_btn, mini_btn, expand_btn, close_btn, overlay);
        let w = Rc::downgrade(&this);
        mini_click.connect_released(move |_, _, _, _| {
            if let Some(p) = w.upgrade() {
                p.expand();
            }
        });

        // mpv event pump and redraw requests
        let weak = Rc::downgrade(&this);
        runtime::spawn_local(async move {
            while wake_rx.recv().await.is_ok() {
                let Some(p) = weak.upgrade() else { break };
                p.drain_events();
            }
        });
        let area = this.area.downgrade();
        runtime::spawn_local(async move {
            while redraw_rx.recv().await.is_ok() {
                match area.upgrade() {
                    Some(a) => a.queue_render(),
                    None => break,
                }
            }
        });
        if ytdlp.is_none() {
            glib::idle_add_local_once(|| super::toast("yt-dlp was not found in PATH — playback and downloads will not work"));
        }
        this
    }

    fn connect_signals(
        self: &Rc<Self>,
        retry: gtk::Button,
        download_btn: gtk::Button,
        mini_btn: gtk::Button,
        expand_btn: gtk::Button,
        close_btn: gtk::Button,
        overlay: gtk::Overlay,
    ) {
        let w = Rc::downgrade(self);
        macro_rules! weak {
            ($body:expr) => {{
                let w = w.clone();
                move |_: &_| {
                    if let Some(p) = w.upgrade() {
                        #[allow(clippy::redundant_closure_call)]
                        ($body)(&p);
                    }
                }
            }};
        }

        // GL
        let wr = w.clone();
        self.area.connect_realize(move |a| {
            let Some(p) = wr.upgrade() else { return };
            a.make_current();
            let res = match a.error() {
                Some(e) => Err(e.to_string()),
                None => p.mpv.create_render_context(&a.display()),
            };
            if let Err(e) = res {
                crate::log::error(&format!("video output unavailable: {e}"));
                p.gl_failed.set(true);
                super::toast(&format!("Video output unavailable ({e}); playing audio only"));
            }
            if let Some((url, start)) = p.pending.take() {
                p.do_load(&url, start);
            } else {
                p.apply_video();
            }
        });
        let wr = w.clone();
        self.area.connect_unrealize(move |a| {
            if let Some(p) = wr.upgrade() {
                a.make_current();
                p.mpv.free_render_context();
            }
        });
        let wr = w.clone();
        self.area.connect_resize(move |_, width, height| {
            if let Some(p) = wr.upgrade() {
                p.fb_size.set((width, height));
                p.update_sub_margin();
            }
        });
        let wr = w.clone();
        self.area.connect_render(move |a, _| {
            if let Some(p) = wr.upgrade() {
                let (mut fw, mut fh) = p.fb_size.get();
                if fw <= 0 || fh <= 0 {
                    let scale = a.scale_factor();
                    fw = a.width() * scale;
                    fh = a.height() * scale;
                }
                p.mpv.render(mpv::gl::current_framebuffer(), fw, fh);
            }
            glib::Propagation::Stop
        });

        // Buttons
        self.play_btn.connect_clicked(weak!(|p: &Rc<Player>| p.toggle_pause()));
        self.mini_play.connect_clicked(weak!(|p: &Rc<Player>| p.toggle_pause()));
        self.mute_btn.connect_clicked(weak!(|p: &Rc<Player>| p.toggle_mute()));
        self.fs_btn.connect_clicked(weak!(|p: &Rc<Player>| p.toggle_fullscreen()));
        retry.connect_clicked(weak!(|p: &Rc<Player>| p.reload_current()));
        download_btn.connect_clicked(weak!(|p: &Rc<Player>| {
            if let Some(f) = p.on_download.borrow().as_ref() {
                f()
            }
        }));
        mini_btn.connect_clicked(weak!(|p: &Rc<Player>| p.minimize()));
        expand_btn.connect_clicked(weak!(|p: &Rc<Player>| p.expand()));
        close_btn.connect_clicked(weak!(|p: &Rc<Player>| p.stop()));
        self.highlight_btn.connect_clicked(weak!(|p: &Rc<Player>| {
            let poi = p.segments.borrow().iter().find(|s| s.seg.action == "poi").map(|s| s.seg.start);
            if let Some(t) = poi {
                p.seek_to(t);
            }
        }));
        self.notice_undo.connect_clicked(weak!(|p: &Rc<Player>| {
            if let Some(i) = p.undo_target.take() {
                let start = p.segments.borrow().get(i).map(|s| {
                    s.ignored.set(true);
                    s.seg.start
                });
                if let Some(t) = start {
                    p.mpv.command(&["seek", &format!("{t}"), "absolute+exact"]).ok();
                }
            }
            p.notice.set_reveal_child(false);
        }));
        let wr = w.clone();
        self.volume.connect_value_changed(move |s| {
            let Some(p) = wr.upgrade() else { return };
            if p.updating.get() {
                return;
            }
            let v = s.value();
            p.mpv.set_f64("volume", v);
            if let Some(id) = p.volume_save.take() {
                id.remove();
            }
            let wr2 = Rc::downgrade(&p);
            let id = glib::timeout_add_local_once(std::time::Duration::from_millis(600), move || {
                if let Some(p) = wr2.upgrade() {
                    p.volume_save.take();
                }
                config::update(|s| s.volume = v);
            });
            p.volume_save.replace(Some(id));
        });
        let wr = w.clone();
        self.audio_btn.connect_toggled(move |b| {
            if let Some(p) = wr.upgrade()
                && !p.updating.get()
            {
                p.set_audio_only(b.is_active());
            }
        });
        let wr = w.clone();
        self.seek.connect_seek(move |t, final_| {
            if let Some(p) = wr.upgrade() {
                p.mark_user_seek(t);
                let flags = if final_ { "absolute+exact" } else { "absolute+keyframes" };
                p.mpv.command(&["seek", &format!("{t}"), flags]).ok();
            }
        });

        // Actions for the speed and quality menus
        let group = gio::SimpleActionGroup::new();
        let speed = gio::SimpleAction::new_stateful("speed", Some(glib::VariantTy::STRING), &"1".to_variant());
        let wr = w.clone();
        speed.connect_activate(move |a, param| {
            let Some(v) = param.and_then(|v| v.get::<String>()) else { return };
            a.set_state(&v.to_variant());
            if let (Some(p), Ok(f)) = (wr.upgrade(), v.parse::<f64>()) {
                p.mpv.set_f64("speed", f);
            }
        });
        group.add_action(&speed);
        let quality = gio::SimpleAction::new_stateful(
            "quality",
            Some(glib::VariantTy::STRING),
            &self.quality.get().to_string().to_variant(),
        );
        let wr = w.clone();
        quality.connect_activate(move |a, param| {
            let Some(v) = param.and_then(|v| v.get::<String>()) else { return };
            a.set_state(&v.to_variant());
            if let (Some(p), Ok(q)) = (wr.upgrade(), v.parse::<u32>()) {
                p.quality.set(q);
                p.quality_btn.set_label(&format!("{q}p"));
                p.reload_current();
            }
        });
        group.add_action(&quality);
        let wr = w.clone();
        self.subs_action.connect_activate(move |_, param| {
            if let (Some(p), Some(key)) = (wr.upgrade(), param.and_then(|v| v.get::<String>())) {
                p.pick_subtitles(&key);
            }
        });
        group.add_action(&self.subs_action);
        self.host.insert_action_group("player", Some(&group));
        let wr = w.clone();
        self.bottom.connect_reveal_child_notify(move |_| {
            if let Some(p) = wr.upgrade() {
                p.update_sub_margin();
            }
        });

        // Pointer: reveal controls, click to pause, double click for fullscreen
        let motion = gtk::EventControllerMotion::new();
        let wr = w.clone();
        motion.connect_motion(move |_, _, _| {
            if let Some(p) = wr.upgrade() {
                p.show_controls();
            }
        });
        overlay.add_controller(motion);

        let click = gtk::GestureClick::new();
        click.set_button(1);
        let wr = w.clone();
        click.connect_pressed(move |_, n, _, _| {
            let Some(p) = wr.upgrade() else { return };
            if p.mode.get() == Mode::Compact {
                return;
            }
            p.toggle_pause();
            if n == 2 && p.mode.get() != Mode::Mini {
                p.toggle_fullscreen();
            }
        });
        let wr = w.clone();
        click.connect_released(move |_, n, _, _| {
            let Some(p) = wr.upgrade() else { return };
            if p.mode.get() == Mode::Compact {
                p.toggle_pause();
                if n == 2 {
                    p.toggle_fullscreen();
                }
            }
        });
        self.area.add_controller(click);
        let click2 = gtk::GestureClick::new();
        let wr = w.clone();
        click2.connect_released(move |_, _, _, _| {
            if let Some(p) = wr.upgrade() {
                p.toggle_pause();
            }
        });
        self.cover.set_can_target(true);
        self.cover.add_controller(click2);
        // The compact player has no title bar: dragging it moves the window.
        for target in [self.area.clone().upcast::<gtk::Widget>(), self.cover.clone().upcast()] {
            let drag = gtk::GestureDrag::new();
            let wr = w.clone();
            let moving = Rc::new(Cell::new(false));
            let m = moving.clone();
            drag.connect_drag_begin(move |_, _, _| m.set(false));
            drag.connect_drag_update(move |g, dx, dy| {
                let Some(p) = wr.upgrade() else { return };
                if p.mode.get() != Mode::Compact || moving.get() || dx.hypot(dy) < 6.0 {
                    return;
                }
                let (Some(widget), Some((x, y))) = (g.widget(), g.start_point()) else { return };
                let Some(native) = widget.native() else { return };
                let Some(toplevel) = native.surface().and_downcast::<gdk::Toplevel>() else { return };
                let Some(pt) = widget.compute_point(&native, &graphene::Point::new(x as f32, y as f32)) else { return };
                let (sx, sy) = native.surface_transform();
                let Some(device) = g.device() else { return };
                moving.set(true);
                g.set_state(gtk::EventSequenceState::Claimed);
                toplevel.begin_move(&device, g.current_button() as i32, pt.x() as f64 + sx, pt.y() as f64 + sy, g.current_event_time());
            });
            target.add_controller(drag);
        }

        // Scrolling over the docked player scrolls the watch page underneath.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let wr = w.clone();
        scroll.connect_scroll(move |c, _, dy| {
            let Some(p) = wr.upgrade() else { return glib::Propagation::Proceed };
            if p.mode.get() != Mode::Docked {
                return glib::Propagation::Proceed;
            }
            if let Some(sw) = p.dock_widget().and_then(|d| d.ancestor(gtk::ScrolledWindow::static_type())) {
                let adj = sw.downcast::<gtk::ScrolledWindow>().unwrap().vadjustment();
                let step = if c.unit() == gdk::ScrollUnit::Wheel { 60.0 } else { 1.0 };
                adj.set_value(adj.value() + dy * step);
            }
            glib::Propagation::Stop
        });
        self.host.add_controller(scroll);
    }

    // ------------------------------------------------------------------
    // layout

    /// Puts the player into `root` (an overlay over the content area).
    pub fn attach(self: &Rc<Self>, root: &gtk::Overlay) {
        root.add_overlay(&self.host);
        root.set_measure_overlay(&self.host, false);
        let host = self.host.clone().upcast::<gtk::Widget>();
        let w = Rc::downgrade(self);
        root.connect_get_child_position(move |root, child| {
            if *child != host {
                return None;
            }
            let p = w.upgrade()?;
            let (x, y, wd, ht) = p.compute_rect(root, true);
            Some(gdk::Rectangle::new(x.round() as i32, y.round() as i32, (wd.round() as i32).max(1), (ht.round() as i32).max(1)))
        });
        self.root.replace(Some(root.downgrade()));
    }

    /// Adds a widget the player covers while it is on screen (the watch
    /// page placeholder, Music's "Now playing" video area).
    pub fn add_dock(self: &Rc<Self>, w: &impl IsA<gtk::Widget>) {
        self.docks.borrow_mut().push(w.as_ref().downgrade());
        let wk = Rc::downgrade(self);
        w.as_ref().connect_map(move |_| {
            if let Some(p) = wk.upgrade() {
                p.force_mini.set(false);
                p.update_mode();
                p.apply_video();
            }
        });
        let wk = Rc::downgrade(self);
        w.as_ref().connect_unmap(move |_| {
            if let Some(p) = wk.upgrade() {
                p.update_mode();
                p.apply_video();
            }
        });
    }

    /// The dock currently on screen.
    fn dock_widget(&self) -> Option<gtk::Widget> {
        self.docks.borrow().iter().rev().filter_map(|w| w.upgrade()).find(|w| w.is_mapped())
    }

    fn root(&self) -> Option<gtk::Overlay> {
        self.root.borrow().as_ref().and_then(|w| w.upgrade())
    }

    fn target_rect(&self, root: &gtk::Overlay) -> ((f32, f32, f32, f32), Option<graphene::Rect>) {
        let (w, h) = (root.width() as f32, root.height() as f32);
        match self.mode.get() {
            Mode::Hidden => ((0.0, 0.0, 1.0, 1.0), None),
            Mode::Fullscreen | Mode::Compact => ((0.0, 0.0, w, h), None),
            Mode::Mini => {
                let mw = (w * 0.3).clamp(260.0, 420.0);
                let mh = mw * 9.0 / 16.0;
                ((w - mw - 16.0, h - mh - 16.0, mw, mh), None)
            }
            Mode::Docked => {
                let Some(dock) = self.dock_widget() else { return ((0.0, 0.0, 1.0, 1.0), None) };
                let Some(b) = dock.compute_bounds(root) else { return (self.rect.get(), None) };
                let mut clip = graphene::Rect::new(0.0, 0.0, w, h);
                if let Some(sw) = dock.ancestor(gtk::ScrolledWindow::static_type())
                    && let Some(sb) = sw.compute_bounds(root)
                {
                    clip = clip.intersection(&sb).unwrap_or_else(|| graphene::Rect::new(0.0, 0.0, 0.0, 0.0));
                }
                ((b.x(), b.y(), b.width(), b.height()), Some(clip))
            }
        }
    }

    fn compute_rect(&self, root: &gtk::Overlay, commit: bool) -> (f32, f32, f32, f32) {
        let (target, clip) = self.target_rect(root);
        let mut rect = target;
        let mut animating = false;
        if let Some((from, start)) = self.anim.get() {
            let now = root.frame_clock().map(|c| c.frame_time()).unwrap_or(start + ANIM_US);
            let t = ((now - start) as f32 / ANIM_US as f32).clamp(0.0, 1.0);
            if t < 1.0 {
                let eased = 1.0 - (1.0 - t).powi(3);
                rect = lerp(from, target, eased);
                animating = true;
            } else if commit {
                self.anim.set(None);
            }
        }
        if commit {
            self.rect.set(rect);
            let width = rect.2.round() as i32;
            if width != self.ctl_width.get() {
                self.ctl_width.set(width);
                self.fit_controls();
            }
            let local_clip = if animating {
                None
            } else {
                clip.map(|c| graphene::Rect::new(c.x() - rect.0, c.y() - rect.1, c.width(), c.height()))
            };
            self.host.set_clip(local_clip);
        }
        rect
    }

    fn compute_mode(&self) -> Mode {
        if self.current.borrow().is_none() {
            return Mode::Hidden;
        }
        if self.fullscreen.get() && !self.immersive.get() {
            return Mode::Fullscreen;
        }
        if self.compact.get() && !self.immersive.get() {
            return Mode::Compact;
        }
        if self.music.get() {
            return if self.dock_widget().is_some() && self.music_video.get() { Mode::Docked } else { Mode::Hidden };
        }
        match self.dock_widget() {
            Some(_) if !self.force_mini.get() => Mode::Docked,
            _ => Mode::Mini,
        }
    }

    fn update_mode(self: &Rc<Self>) {
        let new = self.compute_mode();
        let old = self.mode.replace(new);
        if old == new {
            return;
        }
        let root = self.root();
        if matches!((old, new), (Mode::Docked, Mode::Mini) | (Mode::Mini, Mode::Docked))
            && let Some(clock) = root.as_ref().and_then(|r| r.frame_clock())
        {
            self.anim.set(Some((self.rect.get(), clock.frame_time())));
        } else {
            self.anim.set(None);
        }
        self.host.set_visible(new != Mode::Hidden);
        let mini = new == Mode::Mini;
        self.mini_controls.set_visible(mini);
        self.mini_progress.set_visible(mini);
        // Music mode has its own player bar.
        self.bottom.set_visible(!mini && !(self.music.get() && new == Mode::Docked));
        self.update_sub_margin();
        self.top.set_visible(matches!(new, Mode::Fullscreen | Mode::Compact));
        for (c, on) in [
            ("mini", mini),
            ("fullscreen", new == Mode::Fullscreen),
            ("docked", new == Mode::Docked),
            ("compact", new == Mode::Compact),
        ] {
            if on {
                self.host.add_css_class(c);
            } else {
                self.host.remove_css_class(c);
            }
        }
        self.fs_btn.set_icon_name(if new == Mode::Fullscreen { "view-restore-symbolic" } else { "view-fullscreen-symbolic" });
        let compact = new == Mode::Compact;
        self.compact_btn.set_icon_name(if compact { "gtube-uncompact-symbolic" } else { "gtube-compact-symbolic" });
        self.compact_btn.set_tooltip_text(Some(if compact { "Restore window (Esc)" } else { "Compact player (Ctrl+Shift+M)" }));
        self.fit_controls();
        self.show_controls();

        // Track the placeholder every frame while docked (it moves when the
        // page scrolls or animates) and while animating between modes.
        let need_tick = new == Mode::Docked || self.anim.get().is_some();
        if need_tick && self.tick.borrow().is_none() {
            if let Some(root) = &root {
                let w = Rc::downgrade(self);
                let id = root.add_tick_callback(move |root, _| {
                    let Some(p) = w.upgrade() else { return glib::ControlFlow::Break };
                    let target = p.compute_rect(root, false);
                    if target != p.rect.get() || p.anim.get().is_some() {
                        root.queue_allocate();
                    }
                    if p.mode.get() != Mode::Docked && p.anim.get().is_none() {
                        p.tick.take();
                        return glib::ControlFlow::Break;
                    }
                    glib::ControlFlow::Continue
                });
                self.tick.replace(Some(id));
            }
        } else if !need_tick && let Some(id) = self.tick.take() {
            id.remove();
        }
        if let Some(r) = root {
            r.queue_allocate();
        }
    }

    // ------------------------------------------------------------------
    // playback

    pub fn current(&self) -> Option<VideoItem> {
        self.current.borrow().clone()
    }

    pub fn current_id(&self) -> Option<String> {
        self.current.borrow().as_ref().map(|v| v.id.clone())
    }

    /// Shows the previous / next buttons (for the watch page's queue).
    pub fn set_skip(&self, prev: bool, next: bool) {
        self.want_prev.set(prev);
        self.want_next.set(next);
        self.fit_controls();
    }

    /// Shows the controls that fit the player's width.
    fn fit_controls(&self) {
        let w = self.ctl_width.get();
        for (widget, need) in &self.optional {
            widget.set_visible(w >= *need);
        }
        self.prev_btn.set_visible(self.want_prev.get() && w >= 400);
        self.next_btn.set_visible(self.want_next.get() && w >= 300);
        self.highlight_btn.set_visible(self.want_highlight.get() && w >= 600);
        self.time_label.set_visible(w >= 280);
        self.mini_btn.set_visible(w >= 440 && self.mode.get() != Mode::Compact);
    }

    fn set_highlight_visible(&self, on: bool) {
        if self.want_highlight.replace(on) != on {
            self.fit_controls();
        }
    }

    /// Shrinks the window to just the player (the window does the rest).
    pub fn set_compact(self: &Rc<Self>, on: bool) {
        self.compact.set(on);
        self.update_mode();
    }

    /// Music's immersive view hosts the video (and covers the window).
    pub fn set_immersive(self: &Rc<Self>, on: bool) {
        self.immersive.set(on);
        self.update_mode();
        self.apply_video();
    }

    /// Shows a SponsorBlock segment being marked on the seek bar.
    pub fn set_sb_draft(&self, draft: Option<(f64, Option<f64>)>) {
        self.seek.set_draft(draft);
    }

    /// Fetches the SponsorBlock segments again (after submitting one).
    pub fn refresh_segments(self: &Rc<Self>) {
        if let Some(id) = self.current_id() {
            self.fetch_segments(&id);
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode.get()
    }

    pub fn load(self: &Rc<Self>, v: VideoItem, start: f64) {
        self.load_as(v, start, false);
    }

    /// Plays a track started from Music mode.
    pub fn load_music(self: &Rc<Self>, v: VideoItem, start: f64) {
        self.load_as(v, start, true);
    }

    pub fn is_music_owned(&self) -> bool {
        self.current.borrow().is_some() && self.music_owned.get()
    }

    fn load_as(self: &Rc<Self>, v: VideoItem, start: f64, music: bool) {
        // The previous view ends.
        self.finish_report();
        self.music_owned.set(music);
        self.generation.set(self.generation.get() + 1);
        self.resume_at.set(None);
        self.item_started.set(false);
        self.current.replace(Some(v.clone()));
        self.duration.set(0.0);
        self.position.set(0.0);
        self.buffered.set(0.0);
        self.eof_handled.set(false);
        self.segments.borrow_mut().clear();
        self.seg_bar.borrow_mut().clear();
        self.seek.set_segments(Vec::new());
        self.seek.set_state(0.0, 0.0, 0.0);
        self.set_highlight_visible(false);
        self.seek.set_draft(None);
        self.notice.set_reveal_child(false);
        self.error_box.set_visible(false);
        self.spinner.set_visible(true);
        self.set_captions(extract::Captions::default(), true);
        self.title.set_text(&v.title);
        self.time_label.set_text("0:00 / 0:00");
        let cover = v.thumbnail.clone().filter(|_| music).unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", v.id));
        crate::images::picture(&self.cover_pic, &cover);
        self.updating.set(true);
        self.audio_btn.set_active(!self.wants_video());
        self.updating.set(false);
        self.update_mode();
        self.cover.set_visible(!self.shows_video());

        let url = v.url();
        let needs_gl = self.wants_video() && self.mode.get() != Mode::Hidden;
        if self.mpv.has_render_context() || self.gl_failed.get() || !needs_gl {
            self.do_load(&url, start);
        } else {
            // The GLArea realizes as soon as the host is mapped; the load
            // continues from the realize handler.
            self.pending.replace(Some((url, start)));
        }
        self.fetch_segments(&v.id);
        self.start_report(&v, start <= 0.0 && !music && !v.is_live);
        self.notify_state();
    }

    /// Starts reporting the new view to YouTube and, for `resume`, asks
    /// YouTube where the video was left.
    fn start_report(self: &Rc<Self>, v: &VideoItem, resume: bool) {
        let s = config::settings();
        let client = api::client();
        // Dry runs log the reports instead of sending them (for testing).
        let account = client.is_logged_in() || std::env::var_os("GTUBE_STATS_DRY_RUN").is_some();
        let report = account && s.mark_watched;
        let mut resume = resume && s.resume_playback;
        if report {
            let session = client.session();
            self.report.replace(Some(stats::Report::new(&v.id, v.is_live, &session.client_version, &session.hl, &session.gl)));
        }
        // A video reported during this run continues from there right away.
        if resume && let Some(p) = stats::reported_position(&v.id) {
            if p > 0.0 {
                self.apply_resume(p);
            }
            resume = false;
        }
        if !account || !(report || resume) {
            return;
        }
        let (generation, id) = (self.generation.get(), v.id.clone());
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let vid = id.clone();
            let res = runtime::run(async move { api::client().tracking(&vid).await }).await;
            let Some(p) = w.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            let t = match res {
                Ok(t) => t,
                Err(e) => {
                    crate::log::warn(&format!("watch history: {e}"));
                    return;
                }
            };
            if resume
                && let Some(at) = t.resume
                && t.length.is_none_or(|len| at < len - 10.0)
            {
                crate::log::info(&format!("{id}: YouTube continues at {at:.0} s"));
                p.apply_resume(at);
            }
            let urls = p.report.borrow_mut().as_mut().map(|r| r.set_tracking(t, clock())).unwrap_or_default();
            send_reports(urls);
        });
    }

    /// Continues the current item at `at`: from there when it hasn't started
    /// yet, or by seeking if it only just did.
    fn apply_resume(&self, at: f64) {
        let at = (at - 3.0).max(0.0);
        if at < 5.0 {
            return;
        }
        if !self.item_started.get() {
            self.resume_at.set(Some(at));
        } else if self.position.get() < 10.0 {
            self.seek_to(at);
            self.resume_toast(at);
        }
    }

    fn resume_toast(&self, at: f64) {
        let Some(id) = self.current_id() else { return };
        super::toast_action(&format!("Continuing from {}", super::widgets::format_time(at)), "Start over", move || {
            let p = super::ctx().player.clone();
            if p.current_id().as_deref() == Some(id.as_str()) {
                p.seek_to(0.0);
            }
        });
    }

    /// Sends what the view's report says is due.
    fn report_with(&self, f: impl FnOnce(&mut stats::Report, f64) -> Vec<String>) {
        let urls = match self.report.borrow_mut().as_mut() {
            Some(r) => f(r, clock()),
            None => return,
        };
        send_reports(urls);
    }

    /// The current view ends: the last report goes out.
    fn finish_report(&self) {
        if let Some(mut r) = self.report.take() {
            send_reports(r.finish(clock()));
        }
    }

    /// Like `finish_report`, but waits (briefly) for it to be sent; for quitting.
    pub fn finish_report_blocking(&self) {
        let Some(mut r) = self.report.take() else { return };
        let urls = r.finish(clock());
        if urls.is_empty() {
            return;
        }
        let res = runtime::rt().block_on(async move {
            tokio::time::timeout(std::time::Duration::from_secs(2), async move {
                for u in urls {
                    api::client().ping(&u).await?;
                }
                anyhow::Ok(())
            })
            .await
        });
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => crate::log::warn(&format!("watch history: last report failed: {e}")),
            Err(_) => crate::log::warn("watch history: last report timed out"),
        }
    }

    /// Whether the current item should be opened with its video track.
    fn wants_video(&self) -> bool {
        !self.gl_failed.get() && if self.music_owned.get() { self.music_video.get() } else { !config::settings().audio_only }
    }

    /// Whether video is on screen right now (and worth decoding).
    fn shows_video(&self) -> bool {
        self.wants_video() && self.mode.get() != Mode::Hidden
    }

    /// Turns video decoding on or off to match what is on screen, reopening
    /// the stream with video when it was opened audio-only.
    fn apply_video(self: &Rc<Self>) {
        let show = self.shows_video();
        self.cover.set_visible(!show);
        if self.current.borrow().is_none() {
            return;
        }
        if !show {
            self.mpv.set_str("vid", "no");
        } else if !self.mpv.has_render_context() {
            // Continues from the GLArea's realize handler.
        } else if self.has_video_track() {
            self.mpv.set_str("vid", "auto");
        } else if !self.loaded_with_video.get() {
            self.reload_current();
        }
    }

    /// Music mode is shown (or hidden).
    pub fn set_music_presentation(self: &Rc<Self>, on: bool) {
        if self.music.replace(on) == on {
            return;
        }
        self.force_mini.set(false);
        self.update_mode();
        self.apply_video();
    }

    /// Music mode's Song / Video switch.
    pub fn set_music_video(self: &Rc<Self>, on: bool) {
        config::update(|s| s.music_video = on);
        if self.music_video.replace(on) == on {
            return;
        }
        self.updating.set(true);
        if self.is_music_owned() {
            self.audio_btn.set_active(!on);
        }
        self.updating.set(false);
        self.update_mode();
        self.apply_video();
        if let Some(f) = self.on_music_video.borrow().as_ref() {
            f(on);
        }
    }

    /// Whether the current item plays without its picture.
    pub fn is_audio_only(&self) -> bool {
        if self.is_music_owned() { !self.music_video.get() } else { config::settings().audio_only }
    }

    pub fn music_video(&self) -> bool {
        self.music_video.get()
    }

    fn notify_state(&self) {
        if let Some(f) = self.on_state.borrow().as_ref() {
            f();
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration.get()
    }

    pub fn buffered(&self) -> f64 {
        self.buffered.get()
    }

    pub fn is_paused(&self) -> bool {
        self.paused.get()
    }

    pub fn is_loading(&self) -> bool {
        self.spinner.is_visible() || self.wait_box.is_visible()
    }

    pub fn volume(&self) -> f64 {
        self.volume.value()
    }

    pub fn set_volume(&self, v: f64) {
        self.volume.set_value(v.clamp(0.0, 130.0));
    }

    pub fn is_muted(&self) -> bool {
        self.muted.get()
    }

    /// SponsorBlock segments for a seek bar.
    pub fn bar_segments(&self) -> Vec<(f64, f64, gdk::RGBA, String)> {
        self.seg_bar.borrow().clone()
    }

    fn do_load(self: &Rc<Self>, url: &str, start: f64) {
        let load = self.load_gen.get() + 1;
        self.load_gen.set(load);
        let audio_only = !self.wants_video();
        self.loaded_with_video.set(!audio_only);
        let Some(ytdlp) = extract::find_ytdlp() else {
            self.show_error("yt-dlp was not found in PATH. Install it to play videos.");
            return;
        };
        // Stop the previous video right away, like youtube.com does.
        self.mpv.command(&["stop"]).ok();
        self.error_box.set_visible(false);
        self.spinner.set_visible(true);
        let logged_in = api::client().is_logged_in() && config::cookies_path().exists();
        let req = extract::Request {
            ytdlp,
            url: url.to_string(),
            format: extract::format_string(audio_only, self.quality.get()),
            cookies: logged_in.then(config::cookies_path),
        };
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(extract::extract(req)).await;
            let Some(p) = w.upgrade() else { return };
            if p.load_gen.get() != load {
                return;
            }
            match res {
                Ok(streams) => p.start_streams(load, streams, start),
                Err(e) => p.show_error(&e.to_string()),
            }
        });
    }

    /// Plays extracted streams, first waiting out YouTube's ad delay.
    fn start_streams(self: &Rc<Self>, load: u64, streams: extract::Streams, start: f64) {
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        let Some(at) = streams.available_at.filter(|at| *at - now() > 0.2) else {
            self.play_streams(&streams, start);
            return;
        };
        let update = |p: &Self, left: f64| {
            p.wait_title.set_text(&format!("Starting in {} s", left.ceil() as i64));
            p.wait_box.set_visible(true);
        };
        update(self, at - now());
        let w = Rc::downgrade(self);
        glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            let Some(p) = w.upgrade() else { return glib::ControlFlow::Break };
            if p.load_gen.get() != load {
                p.wait_box.set_visible(false);
                return glib::ControlFlow::Break;
            }
            let left = at - now();
            if left > 0.0 {
                update(&p, left);
                return glib::ControlFlow::Continue;
            }
            p.wait_box.set_visible(false);
            p.play_streams(&streams, start);
            glib::ControlFlow::Break
        });
    }

    /// Adds the spectrum analysis to Music playback (when it is turned on)
    /// and removes it for everything else.
    pub fn update_audio_filter(&self) {
        let want = if self.music_owned.get() && config::settings().music_visualizer {
            crate::ui::music::spectrum::audio_filter().unwrap_or_default()
        } else {
            String::new()
        };
        if *self.audio_filter.borrow() != want {
            self.mpv.set_str("af", &want);
            self.audio_filter.replace(want);
        }
    }

    /// The position of the audio that is being heard.
    pub fn audio_pts(&self) -> Option<f64> {
        self.current.borrow().as_ref()?;
        self.mpv.get_f64("audio-pts")
    }

    fn play_streams(&self, streams: &extract::Streams, start: f64) {
        let mut start = start;
        if !self.item_started.replace(true)
            && start <= 0.5
            && let Some(at) = self.resume_at.take()
        {
            start = at;
            self.resume_toast(at);
        }
        self.update_audio_filter();
        self.set_captions(streams.captions.clone(), false);
        let video = self.shows_video() && self.mpv.has_render_context();
        self.mpv.set_str("vid", if video { "auto" } else { "no" });
        let mut opts = Vec::new();
        if start > 0.5 {
            opts.push(format!("start={start}"));
        }
        if let Some(ua) = &streams.user_agent {
            // Length-prefixed so commas in the value survive.
            opts.push(format!("user-agent=%{}%{ua}", ua.len()));
        }
        if let Err(e) = self.mpv.command(&["loadfile", &streams.url, "replace", "-1", &opts.join(",")]) {
            self.show_error(&format!("Could not start playback: {e}"));
        }
        self.mpv.set_bool("pause", false);
    }

    // ------------------------------------------------------------------
    // Subtitles

    /// The subtitles of what is about to play. For a new item (`new_item`)
    /// they aren't known yet; once they are, what to show comes from the
    /// settings. A reload of the same item keeps what was shown.
    fn set_captions(&self, c: extract::Captions, new_item: bool) {
        self.sub_tracks.borrow_mut().clear();
        self.sub_gen.set(self.sub_gen.get() + 1);
        if new_item {
            self.sub_choice.replace(None);
        } else if self.sub_choice.borrow().is_none() {
            self.sub_choice.replace(Some(self.wanted_subtitles(&c)));
        }
        self.subs_btn.set_menu_model(Some(&subtitles::menu(&c)));
        self.captions.replace(c);
        self.sync_subs_ui();
    }

    /// What the settings ask for.
    fn wanted_subtitles(&self, c: &extract::Captions) -> Choice {
        let s = config::settings();
        // Music has lyrics instead.
        if !s.subtitles || self.music_owned.get() {
            return Choice::Off;
        }
        subtitles::preferred(c, s.subtitle_language(), s.subtitle_translate)
    }

    fn sync_subs_ui(&self) {
        let choice = self.sub_choice.borrow().clone().unwrap_or(Choice::Off);
        self.subs_action.set_state(&choice.key().to_variant());
        let has = !self.captions.borrow().tracks.is_empty();
        self.subs_btn.set_sensitive(has);
        self.subs_btn.set_tooltip_text(Some(if has { "Subtitles (c)" } else { "No subtitles" }));
        if choice == Choice::Off {
            self.subs_btn.remove_css_class("subs-on");
        } else {
            self.subs_btn.add_css_class("subs-on");
        }
    }

    /// A pick from the subtitles menu, remembered for the next videos.
    fn pick_subtitles(self: &Rc<Self>, key: &str) {
        let current = self.sub_choice.borrow().clone().unwrap_or(Choice::Off);
        let c = self.captions.borrow().clone();
        let Some(choice) = Choice::from_key(key, &c, &current) else { return };
        match &choice {
            Choice::Off => config::update(|s| s.subtitles = false),
            Choice::Track(i) => {
                let lang = c.tracks[*i].lang.clone();
                config::update(|s| {
                    s.subtitles = true;
                    s.subtitle_lang = lang;
                });
            }
            Choice::Translate { to, .. } => {
                let to = to.clone();
                config::update(|s| {
                    s.subtitles = true;
                    s.subtitle_lang = to;
                    s.subtitle_translate = true;
                });
            }
        }
        self.show_subtitles(choice);
    }

    fn show_subtitles(self: &Rc<Self>, choice: Choice) {
        self.sub_choice.replace(Some(choice));
        self.sync_subs_ui();
        self.apply_subtitles();
    }

    /// Turns subtitles on or off (C), and remembers that.
    pub fn toggle_subtitles(self: &Rc<Self>) {
        if self.current.borrow().is_none() {
            return;
        }
        let choice = self.sub_choice.borrow().clone();
        match choice {
            // Not known yet: decided when they are.
            None => {
                let on = !config::settings().subtitles;
                config::update(|s| s.subtitles = on);
                super::toast(if on { "Subtitles on" } else { "Subtitles off" });
            }
            Some(Choice::Off) => {
                if self.captions.borrow().tracks.is_empty() {
                    super::toast("This video has no subtitles");
                    return;
                }
                config::update(|s| s.subtitles = true);
                let c = self.captions.borrow().clone();
                let s = config::settings();
                self.show_subtitles(subtitles::preferred(&c, s.subtitle_language(), s.subtitle_translate));
            }
            Some(_) => {
                config::update(|s| s.subtitles = false);
                self.show_subtitles(Choice::Off);
            }
        }
    }

    /// The subtitle settings changed: applies them to what is playing.
    pub fn subtitle_settings_changed(self: &Rc<Self>) {
        let Some(current) = self.sub_choice.borrow().clone() else { return };
        let wanted = self.wanted_subtitles(&self.captions.borrow());
        if wanted != current {
            self.show_subtitles(wanted);
        }
    }

    pub fn set_subtitle_scale(&self, scale: f64) {
        self.mpv.set_f64("sub-scale", scale);
    }

    /// Shows the chosen subtitles in the open file, downloading them first
    /// if they weren't yet.
    fn apply_subtitles(self: &Rc<Self>) {
        if !self.file_ready.get() {
            return;
        }
        let generation = self.sub_gen.get() + 1;
        self.sub_gen.set(generation);
        let choice = self.sub_choice.borrow().clone().unwrap_or(Choice::Off);
        let source = choice.source(&self.captions.borrow()).filter(|_| self.loaded_with_video.get());
        let Some(src) = source else {
            self.mpv.set_str("sid", "no");
            return;
        };
        if let Some((_, id, _)) = self.sub_tracks.borrow().iter().find(|(url, ..)| *url == src.url) {
            self.mpv.set_str("sid", &id.to_string());
            return;
        }
        let load = self.load_gen.get();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let url = src.url.clone();
            let res = runtime::run(async move { api::client().timedtext(&url).await }).await;
            let Some(p) = w.upgrade() else { return };
            if p.sub_gen.get() != generation || p.load_gen.get() != load || !p.file_ready.get() {
                return;
            }
            match res {
                Ok(text) => p.add_subtitles(&src, &text, choice),
                Err(e) => p.subtitles_failed(&choice, &e),
            }
        });
    }

    fn add_subtitles(&self, src: &subtitles::Source, text: &str, choice: Choice) {
        let data = format!("memory://{}", text.replace('\0', ""));
        if let Err(e) = self.mpv.command(&["sub-add", &data, "select", &src.title, &src.lang]) {
            crate::log::warn(&format!("could not add subtitles ({}): {e}", src.lang));
            super::toast("Couldn't show the subtitles");
            return;
        }
        match self.mpv.get_str("sid").and_then(|id| id.parse().ok()) {
            Some(id) => self.sub_tracks.borrow_mut().push((src.url.clone(), id, choice)),
            None => crate::log::warn(&format!("subtitles ({}) were added but aren't shown", src.lang)),
        }
    }

    fn subtitles_failed(self: &Rc<Self>, choice: &Choice, e: &anyhow::Error) {
        crate::log::warn(&format!("subtitles: {e:#}"));
        let shown = self.mpv.get_str("sid").and_then(|id| id.parse::<i64>().ok()).and_then(|id| {
            self.sub_tracks.borrow().iter().find(|(_, track, _)| *track == id).map(|(.., c)| c.clone())
        });
        // A translation that didn't come: the subtitles it translates
        // instead, unless others are on screen.
        let original = match (choice, &shown) {
            (Choice::Translate { from, .. }, None) => Some(Choice::Track(*from)),
            _ => None,
        };
        let mut msg = match (choice, e.downcast_ref::<api::HttpStatus>()) {
            (Choice::Translate { .. }, Some(api::HttpStatus(429))) => {
                "YouTube is refusing auto-translated subtitles right now (HTTP 429)".to_string()
            }
            _ => format!("Couldn't load the subtitles ({e})"),
        };
        if let Some(c) = original.as_ref().and_then(|c| c.source(&self.captions.borrow())) {
            msg.push_str(&format!("; showing {}", c.title));
        }
        super::toast(&msg);
        match original {
            Some(c) => self.show_subtitles(c),
            None => {
                self.sub_choice.replace(Some(shown.unwrap_or(Choice::Off)));
                self.sync_subs_ui();
            }
        }
    }

    /// Keeps subtitles clear of the controls while those are shown.
    fn update_sub_margin(&self) {
        let h = self.area.height();
        let bar = if self.bottom.reveals_child() && self.bottom.is_visible() { self.bottom.height() } else { 0 };
        // In pixels of a 720 pixels tall picture; 22 is mpv's default.
        let margin = 22 + if h > 0 { bar * 720 / h } else { 0 };
        self.mpv.set_str("sub-margin-y", &margin.to_string());
    }

    fn reload_current(self: &Rc<Self>) {
        let Some(v) = self.current() else { return };
        let pos = self.position.get();
        self.error_box.set_visible(false);
        self.spinner.set_visible(true);
        self.do_load(&v.url(), pos);
    }

    pub fn stop(self: &Rc<Self>) {
        self.finish_report();
        self.generation.set(self.generation.get() + 1);
        self.load_gen.set(self.load_gen.get() + 1);
        self.wait_box.set_visible(false);
        self.mpv.command(&["stop"]).ok();
        self.current.replace(None);
        self.music_owned.set(false);
        self.segments.borrow_mut().clear();
        self.force_mini.set(false);
        if self.fullscreen.get() {
            self.toggle_fullscreen();
        }
        self.update_mode();
        self.notify_state();
    }

    pub fn minimize(self: &Rc<Self>) {
        if self.fullscreen.get() {
            self.toggle_fullscreen();
        }
        if let Some(f) = self.on_minimize.borrow().as_ref() {
            f();
        }
        if self.mode.get() == Mode::Docked {
            self.force_mini.set(true);
            self.update_mode();
        }
    }

    pub fn expand(self: &Rc<Self>) {
        self.force_mini.set(false);
        if let Some(f) = self.on_expand.borrow().as_ref() {
            f();
        }
        self.update_mode();
    }

    pub fn toggle_pause(&self) {
        if self.current.borrow().is_none() {
            return;
        }
        if self.position.get() >= self.duration.get() - 0.5 && self.duration.get() > 0.0 && self.paused.get() {
            self.mpv.command(&["seek", "0", "absolute"]).ok();
        }
        self.mpv.set_bool("pause", !self.paused.get());
    }

    /// Starts the current item again from the beginning (repeat one).
    pub fn replay(&self) {
        if self.current.borrow().is_none() {
            return;
        }
        self.mpv.command(&["seek", "0", "absolute+exact"]).ok();
        self.mpv.set_bool("pause", false);
    }

    pub fn toggle_mute(&self) {
        self.mpv.set_bool("mute", !self.muted.get());
    }

    pub fn change_volume(&self, delta: f64) {
        let v = (self.volume.value() + delta).clamp(0.0, 130.0);
        self.volume.set_value(v);
    }

    fn mark_user_seek(&self, t: f64) {
        for s in self.segments.borrow().iter() {
            if t >= s.seg.start && t < s.seg.end {
                s.ignored.set(true);
            }
        }
    }

    pub fn seek_to(&self, t: f64) {
        if self.current.borrow().is_none() {
            return;
        }
        self.mark_user_seek(t);
        self.mpv.command(&["seek", &format!("{t}"), "absolute+exact"]).ok();
    }

    pub fn seek_relative(self: &Rc<Self>, dt: f64) {
        let t = (self.position.get() + dt).max(0.0);
        self.mark_user_seek(t);
        self.mpv.command(&["seek", &format!("{dt}"), "relative+exact"]).ok();
        self.show_controls();
    }

    pub fn toggle_fullscreen(self: &Rc<Self>) {
        let Some(win) = self.host.root().and_downcast::<gtk::Window>() else { return };
        if self.current.borrow().is_none() && !win.is_fullscreen() {
            return;
        }
        if win.is_fullscreen() {
            win.unfullscreen();
        } else {
            win.fullscreen();
        }
    }

    /// Called by the window when its fullscreen state changes.
    pub fn set_fullscreen(self: &Rc<Self>, on: bool) {
        self.fullscreen.set(on);
        if on {
            self.force_mini.set(false);
        }
        self.update_mode();
    }

    pub fn is_fullscreen(&self) -> bool {
        self.fullscreen.get()
    }

    pub fn set_audio_only(self: &Rc<Self>, on: bool) {
        self.updating.set(true);
        self.audio_btn.set_active(on);
        self.updating.set(false);
        if self.is_music_owned() {
            // A track from Music mode follows its Song / Video switch.
            self.set_music_video(!on);
        } else {
            config::update(|s| s.audio_only = on);
            self.apply_video();
        }
        if on && self.current.borrow().is_some() {
            super::toast("Audio only: video is hidden");
        }
    }

    fn has_video_track(&self) -> bool {
        let n: usize = self.mpv.get_str("track-list/count").and_then(|s| s.parse().ok()).unwrap_or(0);
        (0..n).any(|i| self.mpv.get_str(&format!("track-list/{i}/type")).as_deref() == Some("video"))
    }

    // ------------------------------------------------------------------
    // SponsorBlock

    fn fetch_segments(self: &Rc<Self>, id: &str) {
        let s = config::settings();
        if !s.sponsorblock {
            if let Some(f) = self.on_segments.borrow().as_ref() {
                f(&[]);
            }
            return;
        }
        let generation = self.generation.get();
        let id = id.to_string();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { extras::segments(api::client().http(), &id).await }).await;
            let Some(p) = w.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            match res {
                Ok(segs) => p.set_segments(segs),
                Err(e) => crate::log::warn(&format!("SponsorBlock request failed: {e}")),
            }
        });
    }

    fn set_segments(&self, segs: Vec<Segment>) {
        let s = config::settings();
        let states: Vec<SegState> = segs
            .into_iter()
            .map(|seg| {
                let mode = s.sb_mode(&seg.category);
                SegState { seg, mode, ignored: Cell::new(false) }
            })
            .filter(|st| st.mode != SbMode::Off)
            .collect();
        let bar: Vec<(f64, f64, gdk::RGBA, String)> = states
            .iter()
            .filter(|st| matches!(st.seg.action.as_str(), "skip" | "mute" | "poi"))
            .map(|st| {
                let color = gdk::RGBA::parse(extras::sb_color(&st.seg.category)).unwrap_or(gdk::RGBA::GREEN);
                (st.seg.start, st.seg.end, color, extras::sb_label(&st.seg.category).to_string())
            })
            .collect();
        self.seek.set_segments(bar.clone());
        self.seg_bar.replace(bar);
        if let Some(poi) = states.iter().find(|st| st.seg.action == "poi") {
            self.highlight_btn.set_tooltip_text(Some(&format!("Jump to {}", format_time(poi.seg.start))));
        }
        let raw: Vec<Segment> = states.iter().map(|s| s.seg.clone()).collect();
        self.segments.replace(states);
        if let Some(f) = self.on_segments.borrow().as_ref() {
            f(&raw);
        }
        self.notify_state();
    }

    fn check_segments(self: &Rc<Self>, t: f64) {
        if self.sb_seeking.get() {
            return;
        }
        let segs = self.segments.borrow();
        for (i, st) in segs.iter().enumerate() {
            if st.mode != SbMode::Skip || st.ignored.get() || st.seg.action != "skip" {
                continue;
            }
            if t >= st.seg.start && t < st.seg.end - 0.3 {
                self.sb_seeking.set(true);
                self.mpv.command(&["seek", &format!("{}", st.seg.end), "absolute+exact"]).ok();
                self.undo_target.set(Some(i));
                self.show_notice(&format!("Skipped: {}", extras::sb_label(&st.seg.category)));
                break;
            }
        }
        let in_mute = segs.iter().any(|st| {
            st.mode == SbMode::Skip && st.seg.action == "mute" && !st.ignored.get() && t >= st.seg.start && t < st.seg.end
        });
        if in_mute != self.sb_muted.get() {
            self.sb_muted.set(in_mute);
            self.mpv.set_bool("mute", in_mute);
        }
        let poi = segs.iter().find(|st| st.seg.action == "poi").map(|s| s.seg.start);
        self.set_highlight_visible(poi.is_some_and(|p| t < p - 2.0));
    }

    fn show_notice(self: &Rc<Self>, text: &str) {
        self.notice_label.set_text(text);
        self.notice.set_reveal_child(true);
        if let Some(id) = self.notice_source.take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_secs(5), move || {
            if let Some(p) = w.upgrade() {
                p.notice_source.take();
                p.notice.set_reveal_child(false);
            }
        });
        self.notice_source.replace(Some(id));
    }

    /// Updates the title once full video details arrive.
    pub fn set_title(&self, id: &str, title: &str) {
        if let Some(v) = self.current.borrow_mut().as_mut()
            && v.id == id
        {
            v.title = title.to_string();
            self.title.set_text(title);
        }
    }

    pub fn position(&self) -> f64 {
        self.position.get()
    }

    // ------------------------------------------------------------------
    // mpv events

    fn drain_events(self: &Rc<Self>) {
        while let Some(ev) = self.mpv.poll_event() {
            match ev {
                Event::StartFile => {
                    self.file_ready.set(false);
                    self.last_error.borrow_mut().clear();
                    self.error_important.set(false);
                    self.error_box.set_visible(false);
                    self.spinner.set_visible(true);
                }
                Event::FileLoaded => {
                    self.spinner.set_visible(false);
                    self.file_ready.set(true);
                    self.apply_subtitles();
                    self.notify_state();
                }
                Event::PlaybackRestart => {
                    self.sb_seeking.set(false);
                    self.spinner.set_visible(false);
                    self.notify_state();
                }
                Event::EndFile { reason, error } => {
                    self.file_ready.set(false);
                    if reason == mpv::END_FILE_ERROR && self.current.borrow().is_some() {
                        let log = self.last_error.borrow().clone();
                        let msg = if !log.is_empty() { log } else if !error.is_empty() { error } else { "Playback failed".into() };
                        self.show_error(&msg);
                    }
                }
                Event::Property { name, value } => self.on_property(&name, value),
                Event::Log { prefix, level, text } => {
                    let text = text.trim().to_string();
                    let line = format!("mpv[{prefix}] {text}");
                    if level == "warn" {
                        crate::log::warn(&line);
                    } else {
                        crate::log::error(&line);
                    }
                    let http = text.contains("HTTP error");
                    if level == "error" || level == "fatal" || http {
                        // HTTP errors explain more than the generic follow-ups
                        // ("No video or audio streams selected").
                        let important = http;
                        let mut le = self.last_error.borrow_mut();
                        if le.is_empty() || (important && !self.error_important.get()) {
                            *le = text;
                            self.error_important.set(important);
                        }
                    }
                }
                Event::Shutdown => {}
            }
        }
    }

    fn show_error(&self, msg: &str) {
        let id = self.current_id().unwrap_or_default();
        crate::log::error(&format!("playback of {id} failed: {msg}"));
        self.spinner.set_visible(false);
        let text = describe_error(msg);
        self.error_label.set_text(&text);
        self.error_box.set_visible(true);
        self.notify_state();
        if let Some(f) = self.on_error.borrow().as_ref() {
            f(&text);
        }
    }

    fn on_property(self: &Rc<Self>, name: &str, value: mpv::Value) {
        match name {
            "time-pos" => {
                if let Some(t) = value.as_f64() {
                    self.position.set(t);
                    if !self.paused.get() {
                        let rate = self.speed.get();
                        self.report_with(|r, now| r.time(t, rate, now));
                    }
                    self.check_segments(t);
                    self.update_time();
                }
            }
            "duration" => {
                self.duration.set(value.as_f64().unwrap_or(0.0));
                let d = self.duration.get();
                self.report_with(|r, _| {
                    r.set_length(d);
                    Vec::new()
                });
                self.update_time();
            }
            "demuxer-cache-time" => {
                self.buffered.set(value.as_f64().unwrap_or(0.0));
                self.update_time();
            }
            "pause" => {
                let p = value.as_bool().unwrap_or(false);
                self.paused.set(p);
                if p {
                    self.report_with(|r, now| r.pause(now));
                }
                let icon = if p { "media-playback-start-symbolic" } else { "media-playback-pause-symbolic" };
                self.play_btn.set_icon_name(icon);
                self.play_btn.set_tooltip_text(Some(if p { "Play (k)" } else { "Pause (k)" }));
                self.mini_play.set_icon_name(icon);
                self.show_controls();
                self.notify_state();
            }
            "paused-for-cache" | "seeking" => {
                if value.as_bool() == Some(true) {
                    self.spinner.set_visible(true);
                } else if self.duration.get() > 0.0 {
                    self.spinner.set_visible(false);
                }
                self.notify_state();
            }
            "volume" => {
                if let Some(v) = value.as_f64() {
                    self.updating.set(true);
                    self.volume.set_value(v);
                    self.updating.set(false);
                    self.update_volume_icon();
                    self.notify_state();
                }
            }
            "mute" => {
                self.muted.set(value.as_bool().unwrap_or(false));
                self.update_volume_icon();
                self.notify_state();
            }
            "speed" => {
                if let Some(s) = value.as_f64() {
                    self.speed.set(s);
                    let txt = format!("{s}").trim_end_matches(".0").to_string();
                    self.speed_btn.set_label(&format!("{txt}×"));
                }
            }
            "eof-reached" => {
                if value.as_bool() == Some(true) && self.current.borrow().is_some() && !self.eof_handled.get() {
                    self.eof_handled.set(true);
                    if let Some(f) = self.on_ended.borrow().as_ref() {
                        f();
                    }
                } else if value.as_bool() == Some(false) {
                    self.eof_handled.set(false);
                }
            }
            _ => {}
        }
    }

    fn update_volume_icon(&self) {
        let v = self.volume.value();
        let icon = if self.muted.get() || v <= 0.0 {
            "audio-volume-muted-symbolic"
        } else if v < 34.0 {
            "audio-volume-low-symbolic"
        } else if v < 67.0 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.mute_btn.set_icon_name(icon);
    }

    fn update_time(&self) {
        let (pos, dur) = (self.position.get(), self.duration.get());
        self.seek.set_state(pos, dur, self.buffered.get());
        let live = self.current.borrow().as_ref().is_some_and(|v| v.is_live);
        let text = if live && dur <= 0.0 {
            format!("{} • LIVE", format_time(pos))
        } else {
            format!("{} / {}", format_time(pos), format_time(dur))
        };
        if self.time_label.text() != text {
            self.time_label.set_text(&text);
        }
        if dur > 0.0 {
            self.mini_progress.set_fraction((pos / dur).clamp(0.0, 1.0));
        }
        self.notify_state();
    }

    fn show_controls(self: &Rc<Self>) {
        self.bottom.set_reveal_child(true);
        self.top.set_reveal_child(true);
        self.host.set_cursor(None);
        self.mini_controls.set_opacity(1.0);
        if let Some(id) = self.hide_source.take() {
            id.remove();
        }
        let w = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(2500), move || {
            let Some(p) = w.upgrade() else { return };
            p.hide_source.take();
            if p.paused.get() || p.current.borrow().is_none() {
                return;
            }
            p.bottom.set_reveal_child(false);
            p.top.set_reveal_child(false);
            p.mini_controls.set_opacity(0.0);
            if p.mode.get() == Mode::Fullscreen {
                p.host.set_cursor_from_name(Some("none"));
            }
        });
        self.hide_source.replace(Some(id));
    }
}

/// Seconds on a steady clock, for the watch-time reports.
fn clock() -> f64 {
    static START: std::sync::LazyLock<std::time::Instant> = std::sync::LazyLock::new(std::time::Instant::now);
    START.elapsed().as_secs_f64()
}

/// Sends reports to YouTube in the background.
fn send_reports(urls: Vec<String>) {
    for url in urls {
        runtime::spawn_local(async move {
            let kind = if url.contains("/watchtime") { "watch time" } else { "playback" };
            if let Err(e) = runtime::run(async move { api::client().ping(&url).await }).await {
                crate::log::warn(&format!("watch history: {kind} report failed: {e}"));
            }
        });
    }
}
