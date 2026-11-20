//! Downloads through yt-dlp, with progress, cancel and optional SponsorBlock
//! segment removal.

use crate::api::models::{PlaylistItem, VideoItem};
use crate::config::{self, SbMode};
use crate::runtime;
use adw::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

#[derive(Clone)]
pub struct Options {
    pub audio_only: bool,
    pub quality: u32,
    pub audio_format: String,
    pub sb_remove: bool,
}

#[derive(Clone, PartialEq)]
enum State {
    Running,
    Done,
    Failed(String),
    Cancelled,
}

struct Task {
    row: gtk::Box,
    progress: gtk::ProgressBar,
    status: gtk::Label,
    cancel: gtk::Button,
    open: gtk::Button,
    folder: gtk::Button,
    remove: gtk::Button,
    state: RefCell<State>,
    process: RefCell<Option<gio::Subprocess>>,
    path: RefCell<Option<String>>,
}

pub struct Downloads {
    pub page: adw::NavigationPage,
    list: gtk::Box,
    empty: gtk::Box,
    tasks: RefCell<Vec<Rc<Task>>>,
    running: Cell<usize>,
}

impl Downloads {
    pub fn new() -> Rc<Self> {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let (empty, _, _, _) = super::feed::status_box(
            "folder-download-symbolic",
            "No downloads yet",
            "Use the download button on a video or right-click any video, then choose Download.",
        );
        let open_dir = gtk::Button::with_label("Open download folder");
        open_dir.add_css_class("pill");
        open_dir.set_halign(gtk::Align::Center);
        open_dir.connect_clicked(|_| open_path(&config::settings().download_dir));
        let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
        content.set_margin_top(24);
        content.set_margin_bottom(24);
        content.append(&empty);
        content.append(&list);
        content.append(&open_dir);
        let clamp = adw::Clamp::builder().maximum_size(900).child(&content).build();
        clamp.set_margin_start(16);
        clamp.set_margin_end(16);
        let scroll = super::widgets::scrolled(&clamp);
        let page = adw::NavigationPage::builder().title("Downloads").tag("downloads").child(&scroll).build();
        Rc::new(Self { page, list, empty, tasks: RefCell::new(Vec::new()), running: Cell::new(0) })
    }

    pub fn active(&self) -> usize {
        self.running.get()
    }

    pub fn cancel_all(&self) {
        for t in self.tasks.borrow().iter() {
            if let Some(p) = t.process.borrow().as_ref() {
                p.force_exit();
            }
        }
    }

    pub fn start(self: &Rc<Self>, title: &str, url: &str, opts: Options, playlist: bool) {
        let s = config::settings();
        let dir = std::path::PathBuf::from(&s.download_dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            super::toast(&format!("Cannot create {}: {e}", dir.display()));
            return;
        }
        let template = if playlist {
            dir.join("%(playlist_title)s/%(playlist_index)03d - %(title)s [%(id)s].%(ext)s")
        } else {
            dir.join("%(title)s [%(id)s].%(ext)s")
        };
        let mut args: Vec<String> = vec![
            crate::extract::ytdlp(),
            "--newline".into(),
            "--no-colors".into(),
            "--progress".into(),
            "--progress-template".into(),
            "download:GTPROG %(progress._percent_str)s|%(progress._speed_str)s|%(progress._eta_str)s|%(info.playlist_index)s|%(info.n_entries)s".into(),
            "--print".into(),
            "after_move:GTPATH %(filepath)s".into(),
            // --print implies --quiet, which would hide the post-processing
            // status lines ([Merger], [SponsorBlock], …) shown in the row.
            "--no-quiet".into(),
            "-o".into(),
            template.to_string_lossy().into_owned(),
        ];
        if !playlist {
            args.push("--no-playlist".into());
        }
        if let Some(path) = config::cookies_path().filter(|_| crate::api::client().is_logged_in()) {
            args.push("--cookies".into());
            args.push(path.to_string_lossy().into_owned());
        }
        let kind;
        if opts.audio_only {
            kind = format!("Audio • {}", opts.audio_format.to_uppercase());
            args.extend(["-f".into(), "bestaudio/best".into(), "-x".into(), "--audio-format".into(), opts.audio_format.clone()]);
            args.extend(["--audio-quality".into(), "0".into()]);
        } else {
            kind = format!("Video • up to {}p", opts.quality);
            args.extend([
                "-f".into(),
                format!("bv*[height<=?{q}]+ba/b[height<=?{q}]/b", q = opts.quality),
                "-S".into(),
                format!("res:{},vcodec:h264,acodec:m4a", opts.quality),
                "--merge-output-format".into(),
                "mp4/mkv".into(),
            ]);
        }
        if s.download_embed_metadata {
            args.extend(["--embed-metadata".into(), "--embed-thumbnail".into(), "--embed-chapters".into()]);
        }
        if opts.sb_remove {
            let cats: Vec<String> = s
                .sb_modes
                .iter()
                .filter(|(c, m)| **m == SbMode::Skip && c.as_str() != "poi_highlight")
                .map(|(c, _)| c.clone())
                .collect();
            if !cats.is_empty() {
                args.extend(["--sponsorblock-remove".into(), cats.join(",")]);
            }
        }
        args.push(url.to_string());

        let task = self.add_row(title, &kind);
        let argv: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
        let proc = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_PIPE) {
            Ok(p) => p,
            Err(e) => {
                task.finish(State::Failed(format!("Could not run yt-dlp: {e}")));
                return;
            }
        };
        task.process.replace(Some(proc.clone()));
        self.running.set(self.running.get() + 1);
        super::toast(&format!("Downloading “{title}”"));

        let last_err: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        if let Some(err) = proc.stderr_pipe() {
            let last_err = last_err.clone();
            runtime::spawn_local(async move {
                let input = gio::DataInputStream::new(&err);
                while let Ok(Some(line)) = input.read_line_utf8_future(glib::Priority::DEFAULT).await {
                    let line = line.to_string();
                    if line.starts_with("ERROR") || last_err.borrow().is_empty() {
                        *last_err.borrow_mut() = line;
                    }
                }
            });
        }
        let this = Rc::downgrade(self);
        let title = title.to_string();
        runtime::spawn_local(async move {
            if let Some(out) = proc.stdout_pipe() {
                let input = gio::DataInputStream::new(&out);
                while let Ok(Some(line)) = input.read_line_utf8_future(glib::Priority::DEFAULT).await {
                    task.handle_line(&line);
                }
            }
            let _ = proc.wait_future().await;
            if let Some(t) = this.upgrade() {
                t.running.set(t.running.get().saturating_sub(1));
            }
            if *task.state.borrow() == State::Cancelled {
                return;
            }
            if proc.is_successful() {
                task.finish(State::Done);
                super::toast(&format!("Downloaded “{title}”"));
            } else {
                let msg = last_err.borrow().trim_start_matches("ERROR: ").to_string();
                crate::log::error(&format!("download failed: {msg}"));
                task.finish(State::Failed(if msg.is_empty() { "yt-dlp failed".into() } else { msg }));
            }
        });
    }

    fn add_row(self: &Rc<Self>, title: &str, kind: &str) -> Rc<Task> {
        let t = gtk::Label::new(Some(title));
        t.add_css_class("heading");
        t.set_xalign(0.0);
        t.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let k = gtk::Label::new(Some(kind));
        k.add_css_class("dim-label");
        k.add_css_class("caption");
        k.set_xalign(0.0);
        let progress = gtk::ProgressBar::new();
        let status = gtk::Label::new(Some("Starting…"));
        status.add_css_class("dim-label");
        status.add_css_class("caption");
        status.set_xalign(0.0);
        status.set_wrap(true);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 4);
        text.set_hexpand(true);
        text.append(&t);
        text.append(&k);
        text.append(&progress);
        text.append(&status);
        let mk = |icon: &str, tip: &str| {
            let b = gtk::Button::from_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.add_css_class("flat");
            b.add_css_class("circular");
            b.set_valign(gtk::Align::Center);
            b
        };
        let cancel = mk("process-stop-symbolic", "Cancel");
        let open = mk("media-playback-start-symbolic", "Open file");
        let folder = mk("folder-open-symbolic", "Show in folder");
        let remove = mk("window-close-symbolic", "Remove from list");
        for b in [&open, &folder, &remove] {
            b.set_visible(false);
        }
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        row.add_css_class("download-row");
        row.add_css_class("card");
        row.append(&text);
        for b in [&cancel, &open, &folder, &remove] {
            row.append(b);
        }
        let task = Rc::new(Task {
            row,
            progress,
            status,
            cancel,
            open,
            folder,
            remove,
            state: RefCell::new(State::Running),
            process: RefCell::new(None),
            path: RefCell::new(None),
        });
        let w = Rc::downgrade(&task);
        task.cancel.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                if let Some(p) = t.process.borrow().as_ref() {
                    p.force_exit();
                }
                t.finish(State::Cancelled);
            }
        });
        let w = Rc::downgrade(&task);
        task.open.connect_clicked(move |_| {
            if let Some(p) = w.upgrade().and_then(|t| t.path.borrow().clone()) {
                open_path(&p);
            }
        });
        let w = Rc::downgrade(&task);
        task.folder.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                let p = t.path.borrow().clone();
                let dir = p
                    .as_deref()
                    .and_then(|p| std::path::Path::new(p).parent().map(|d| d.to_string_lossy().into_owned()))
                    .unwrap_or_else(|| config::settings().download_dir);
                open_path(&dir);
            }
        });
        let (wt, ws) = (Rc::downgrade(&task), Rc::downgrade(self));
        task.remove.connect_clicked(move |_| {
            if let (Some(t), Some(s)) = (wt.upgrade(), ws.upgrade()) {
                s.list.remove(&t.row);
                s.tasks.borrow_mut().retain(|x| !Rc::ptr_eq(x, &t));
                s.empty.set_visible(s.tasks.borrow().is_empty());
            }
        });
        self.list.prepend(&task.row);
        self.tasks.borrow_mut().push(task.clone());
        self.empty.set_visible(false);
        task
    }
}

impl Task {
    fn handle_line(&self, line: &str) {
        if let Some(rest) = line.strip_prefix("GTPROG ") {
            let parts: Vec<&str> = rest.split('|').map(str::trim).collect();
            let pct = parts.first().and_then(|p| p.trim_end_matches('%').trim().parse::<f64>().ok());
            if let Some(p) = pct {
                self.progress.set_fraction((p / 100.0).clamp(0.0, 1.0));
            }
            let mut s = format!("{} • {} • ETA {}", parts.first().unwrap_or(&""), parts.get(1).unwrap_or(&""), parts.get(2).unwrap_or(&""));
            if let (Some(i), Some(n)) = (parts.get(3), parts.get(4))
                && *i != "NA"
                && *n != "NA"
            {
                s = format!("Video {i} of {n} • {s}");
            }
            self.status.set_text(&s);
        } else if let Some(p) = line.strip_prefix("GTPATH ") {
            self.path.replace(Some(p.trim().to_string()));
        } else if line.starts_with("[ExtractAudio]") || line.starts_with("[Merger]") || line.starts_with("[SponsorBlock]") || line.starts_with("[ModifyChapters]") {
            self.status.set_text("Post-processing…");
        }
    }

    fn finish(&self, state: State) {
        self.cancel.set_visible(false);
        self.remove.set_visible(true);
        match &state {
            State::Done => {
                self.progress.set_fraction(1.0);
                let p = self.path.borrow().clone();
                self.status.set_text(&p.unwrap_or_else(|| "Finished".into()));
                self.open.set_visible(self.path.borrow().is_some());
                self.folder.set_visible(true);
            }
            State::Failed(msg) => {
                self.status.set_text(&format!("Failed: {msg}"));
                self.row.add_css_class("failed");
            }
            State::Cancelled => self.status.set_text("Cancelled"),
            State::Running => {}
        }
        self.state.replace(state);
        self.process.replace(None);
    }
}

pub fn open_path(path: &str) {
    let file = gio::File::for_path(path);
    let launcher = gtk::FileLauncher::new(Some(&file));
    let win = super::ctx().window.clone();
    launcher.launch(Some(&win), None::<&gio::Cancellable>, |r| {
        if let Err(e) = r {
            super::toast(&format!("Could not open: {e}"));
        }
    });
}

fn dialog(title: &str, body: &str, allow_video: bool, done: impl Fn(Options) + 'static) {
    let s = config::settings();
    let d = adw::AlertDialog::new(Some(title), Some(body));
    d.add_response("cancel", "Cancel");
    d.add_response("download", "Download");
    d.set_response_appearance("download", adw::ResponseAppearance::Suggested);
    d.set_default_response(Some("download"));
    d.set_close_response("cancel");

    let group = adw::PreferencesGroup::new();
    let kind = adw::ComboRow::builder().title("Format").model(&gtk::StringList::new(&["Video", "Audio only"])).build();
    let qualities = ["2160", "1440", "1080", "720", "480", "360"];
    let quality = adw::ComboRow::builder()
        .title("Maximum quality")
        .model(&gtk::StringList::new(&qualities.map(|q| format!("{q}p")).iter().map(String::as_str).collect::<Vec<_>>()))
        .build();
    quality.set_selected(qualities.iter().position(|q| q.parse::<u32>().ok() == Some(s.max_quality)).unwrap_or(2) as u32);
    let formats = ["mp3", "m4a", "opus", "flac"];
    let afmt = adw::ComboRow::builder().title("Audio format").model(&gtk::StringList::new(&formats)).build();
    afmt.set_selected(formats.iter().position(|f| *f == s.audio_format).unwrap_or(0) as u32);
    let sb = adw::SwitchRow::builder()
        .title("Cut out sponsor segments")
        .subtitle("Uses SponsorBlock; categories set to “Skip” in Settings")
        .active(s.download_sb_remove)
        .build();
    if !allow_video {
        kind.set_selected(1);
    }
    if s.audio_only {
        kind.set_selected(1);
    }
    let sync = {
        let (q, a) = (quality.clone(), afmt.clone());
        move |k: &adw::ComboRow| {
            let audio = k.selected() == 1;
            q.set_visible(!audio);
            a.set_visible(audio);
        }
    };
    sync(&kind);
    kind.connect_selected_notify(sync);
    group.add(&kind);
    group.add(&quality);
    group.add(&afmt);
    group.add(&sb);
    d.set_extra_child(Some(&group));

    d.connect_response(None, move |_, resp| {
        if resp != "download" {
            return;
        }
        let opts = Options {
            audio_only: kind.selected() == 1,
            quality: qualities[quality.selected() as usize].parse().unwrap_or(1080),
            audio_format: formats[afmt.selected() as usize].to_string(),
            sb_remove: sb.is_active(),
        };
        config::update(|c| {
            c.audio_format = opts.audio_format.clone();
            c.download_sb_remove = opts.sb_remove;
        });
        done(opts);
    });
    d.present(Some(&super::ctx().window));
}

pub fn show_dialog(v: &VideoItem) {
    let v = v.clone();
    let title = v.title.clone();
    dialog("Download video", &title, true, move |opts| {
        super::ctx().downloads.start(&v.title, &v.url(), opts, false);
    });
}

pub fn download_playlist(p: &PlaylistItem) {
    let p = p.clone();
    let body = if p.title.is_empty() { "Every video in this playlist".to_string() } else { p.title.clone() };
    dialog("Download playlist", &body, true, move |opts| {
        let title = if p.title.is_empty() { "Playlist".to_string() } else { p.title.clone() };
        super::ctx()
            .downloads
            .start(&title, &format!("https://www.youtube.com/playlist?list={}", p.id), opts, true);
    });
}
