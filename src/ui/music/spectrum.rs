//! The spectrum visualizer behind the Music player bar.
//!
//! The levels come from mpv itself: an audio filter on a side branch (the
//! sound you hear passes through untouched) downmixes to mono at 11 kHz,
//! splits it into log-spaced bands with an FFT filter bank and measures each
//! band's mean magnitude for every 23 ms window. `ametadata` prints those
//! values, with the window's timestamp, into a FIFO that a reader thread
//! parses. The widget then shows the window that matches `audio-pts` (what
//! is being heard right now), so the bars stay in sync whatever the size of
//! the audio buffers.
//!
//! The whole branch costs about 2% of one CPU core. Before it is used, a
//! short ffmpeg run checks that the local libavfilter has every filter it
//! needs; mpv would otherwise play without audio if the graph didn't parse.

use crate::runtime;
use gtk::{gdk, glib, graphene, gsk, prelude::*, subclass::prelude::*};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};

pub const BANDS: usize = 16;
const RATE: u32 = 11025;
const LOW_HZ: f64 = 45.0;
const HIGH_HZ: f64 = 4600.0;
/// The FFT filter bank delays the levels by about this much.
const FILTER_DELAY: f64 = 0.03;

type Frame = (f64, [f32; BANDS]);

static FRAMES: Mutex<VecDeque<Frame>> = Mutex::new(VecDeque::new());
static FIFO: OnceLock<Option<PathBuf>> = OnceLock::new();
/// 0: not checked yet, 1: checking, 2: usable, 3: unavailable.
static STATUS: AtomicU8 = AtomicU8::new(0);

fn graph(out: &str) -> String {
    let ratio = (HIGH_HZ / LOW_HZ).powf(1.0 / (BANDS - 1) as f64);
    let pan: Vec<String> = (0..BANDS).map(|i| format!("c{i}=c0")).collect();
    format!(
        "asplit[ao][a];\
         [a]aformat=channel_layouts=mono,aresample={RATE},pan={BANDS} channels|{pan},\
         firequalizer=gain='if(between(f,{LOW_HZ}*pow({ratio:.6},ch-0.5),{LOW_HZ}*pow({ratio:.6},ch+0.5)),0,-80)':multi=1:delay=0.02,\
         aspectralstats=win_size=256:overlap=0:measure=mean,\
         ametadata=mode=print:direct=1:file='{out}',anullsink;\
         [ao]anull",
        pan = pan.join("|"),
    )
}

/// The value for mpv's `af` property, once the filter is known to work.
pub fn audio_filter() -> Option<String> {
    if STATUS.load(Ordering::Acquire) != 2 {
        return None;
    }
    let fifo = FIFO.get()?.as_ref()?;
    let g = graph(&fifo.to_string_lossy());
    Some(format!("@gtubeviz:lavfi=graph=%{}%{g}", g.len()))
}

fn fifo_path() -> Option<PathBuf> {
    FIFO.get_or_init(|| {
        let path = glib::user_runtime_dir().join(format!("gtube-spectrum-{}", std::process::id()));
        // Anything in the path that ffmpeg's filter syntax would read as
        // syntax rules this out (the runtime dir never has such characters).
        if path.to_string_lossy().contains(['\'', '\\', ':', ',', ';', '[', ']']) {
            return None;
        }
        let _ = std::fs::remove_file(&path);
        let file = open_fifo(&path)?;
        std::thread::Builder::new().name("spectrum".into()).spawn(move || read_levels(file)).ok()?;
        Some(path)
    })
    .clone()
}

/// Creates the named pipe the levels arrive through.
#[cfg(unix)]
fn open_fifo(path: &std::path::Path) -> Option<std::fs::File> {
    let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    if unsafe { libc::mkfifo(c.as_ptr(), 0o600) } != 0 {
        crate::log::warn(&format!("spectrum: could not create {}: {}", path.display(), std::io::Error::last_os_error()));
        return None;
    }
    // Opened read *and* write, so the reader never sees end-of-file
    // and ffmpeg's open() for writing never waits for a reader.
    match std::fs::OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => Some(f),
        Err(e) => {
            crate::log::warn(&format!("spectrum: could not open {}: {e}", path.display()));
            None
        }
    }
}

/// Named pipes are a Unix thing: elsewhere there's no visualizer.
#[cfg(not(unix))]
fn open_fifo(_: &std::path::Path) -> Option<std::fs::File> {
    None
}

/// Parses `ametadata` output: a "frame:N pts:P pts_time:T" line, then one
/// "lavfi.aspectralstats.<band>.mean=<value>" line per band.
fn read_levels(file: std::fs::File) {
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    let mut cur: Option<Frame> = None;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                std::thread::sleep(std::time::Duration::from_millis(50));
                continue;
            }
            Ok(_) => {}
            Err(_) => return,
        }
        let l = line.trim_end();
        if let Some(t) = l.split("pts_time:").nth(1) {
            cur = t.trim().parse::<f64>().ok().map(|t| (t, [0.0; BANDS]));
            continue;
        }
        let Some(rest) = l.strip_prefix("lavfi.aspectralstats.") else { continue };
        let Some((band, value)) = rest.split_once(".mean=") else { continue };
        let (Ok(band), Ok(value), Some(f)) = (band.parse::<usize>(), value.parse::<f32>(), cur.as_mut()) else { continue };
        if (1..=BANDS).contains(&band) {
            f.1[band - 1] = value;
        }
        if band == BANDS
            && let Some(f) = cur.take()
        {
            let mut q = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
            // Time went backwards: a seek, or the next track.
            if q.back().is_some_and(|b| f.0 < b.0 - 0.05) {
                q.clear();
            }
            q.push_back(f);
            while q.len() > 600 {
                q.pop_front();
            }
        }
    }
}

/// The band magnitudes of the window being heard at `pts`.
fn levels_at(pts: f64) -> Option<[f32; BANDS]> {
    let pts = pts - FILTER_DELAY;
    let mut q = FRAMES.lock().unwrap_or_else(|e| e.into_inner());
    while q.len() > 1 && q[1].0 <= pts {
        q.pop_front();
    }
    let f = q.front()?;
    (f.0 <= pts + 0.03 && pts - f.0 < 0.25).then_some(f.1)
}

/// Checks (once) that this system's libavfilter can run the graph, then
/// calls `ready`.
pub fn init(ready: impl FnOnce() + 'static) {
    if STATUS.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    if fifo_path().is_none() {
        STATUS.store(3, Ordering::Release);
        return;
    }
    runtime::spawn_local(async move {
        let res = runtime::run(async {
            let out = tokio::process::Command::new("ffmpeg")
                .args(["-nostdin", "-hide_banner", "-v", "error", "-f", "lavfi", "-i", "anullsrc=r=48000:cl=stereo", "-t", "0.3", "-af"])
                .arg(graph("/dev/null"))
                .args(["-f", "null", "-"])
                .stdin(std::process::Stdio::null())
                .kill_on_drop(true)
                .output()
                .await?;
            if out.status.success() {
                Ok(())
            } else {
                Err(anyhow::anyhow!("{}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("ffmpeg failed")))
            }
        })
        .await;
        match res {
            Ok(()) => {
                STATUS.store(2, Ordering::Release);
                ready();
            }
            Err(e) => {
                crate::log::warn(&format!("spectrum visualizer unavailable: {e}"));
                STATUS.store(3, Ordering::Release);
            }
        }
    });
}

pub fn cleanup() {
    if let Some(Some(p)) = FIFO.get() {
        let _ = std::fs::remove_file(p);
    }
}

// ---------------------------------------------------------------------------
// Widget

/// Per-band corrections (dB), measured on a few songs: the lowest bands
/// are narrower than the filter bank's resolution and read low.
const TILT: [f32; BANDS] = [6.0, 3.5, 2.0, 1.0, 0.5, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.5, 2.5];
/// The range shown, below the loudest band of the last few seconds.
const RANGE_DB: f32 = 36.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Spectrum {
        pub levels: RefCell<[f32; BANDS]>,
        /// Loudest recent band (dB), falling slowly: an automatic gain.
        pub reference: Cell<f32>,
        pub playing: Cell<bool>,
        pub tick: RefCell<Option<gtk::TickCallbackId>>,
        pub last_frame: Cell<i64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Spectrum {
        const NAME: &'static str = "GtSpectrum";
        type Type = super::Spectrum;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Spectrum {}

    impl WidgetImpl for Spectrum {
        fn measure(&self, _o: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            (0, 0, -1, -1)
        }

        fn unmap(&self) {
            self.parent_unmap();
            if let Some(t) = self.tick.take() {
                t.remove();
            }
            *self.levels.borrow_mut() = [0.0; BANDS];
        }

        fn map(&self) {
            self.parent_map();
            self.obj().update_ticking();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let levels = *self.levels.borrow();
            if levels.iter().all(|v| *v < 0.004) {
                return;
            }
            let (w, h) = (obj.width() as f32, obj.height() as f32);
            let (bar_w, gap) = (4.0f32, 3.0f32);
            let step = bar_w + gap;
            let half = ((w / 2.0) / step).floor() as usize;
            if half < 2 {
                return;
            }
            let cx = w / 2.0;
            let c = obj.color();
            let tint = |a: f32| gdk::RGBA::new(c.red(), c.green(), c.blue(), c.alpha() * a);
            // Plain gradient rectangles: drawn by the GPU, unlike paths. All
            // bars share one vertical ramp; they get fainter towards the
            // edges, behind the track info and the buttons.
            let (bottom, top) = (graphene::Point::new(0.0, h), graphene::Point::new(0.0, 0.0));
            // Bass in the middle, treble towards both edges.
            for i in 0..half {
                let p = i as f32 / (half - 1) as f32;
                let v = sample(&levels, p * (BANDS - 1) as f32);
                let bh = (v * h * 0.78).round();
                if bh < 1.0 {
                    continue;
                }
                let fade = 1.0 - 0.65 * p * p;
                let stops = [gsk::ColorStop::new(0.0, tint(0.34 * fade)), gsk::ColorStop::new(1.0, tint(0.04 * fade))];
                let off = gap / 2.0 + i as f32 * step;
                for x in [cx + off, cx - off - bar_w] {
                    snapshot.append_linear_gradient(&graphene::Rect::new(x, h - bh, bar_w, bh), &bottom, &top, &stops);
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct Spectrum(ObjectSubclass<imp::Spectrum>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Catmull-Rom interpolation between band levels.
fn sample(levels: &[f32; BANDS], x: f32) -> f32 {
    let i = (x.floor() as isize).clamp(0, BANDS as isize - 1);
    let t = x - i as f32;
    let at = |k: isize| levels[k.clamp(0, BANDS as isize - 1) as usize];
    let (p0, p1, p2, p3) = (at(i - 1), at(i), at(i + 1), at(i + 2));
    let v = 0.5 * (2.0 * p1 + (p2 - p0) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t * t + (3.0 * p1 - p0 - 3.0 * p2 + p3) * t * t * t);
    v.clamp(0.0, 1.0)
}

impl Spectrum {
    pub fn new() -> Self {
        let o: Self = glib::Object::new();
        o.add_css_class("music-visualizer");
        o.set_can_target(false);
        o.set_can_focus(false);
        o.imp().reference.set(-200.0);
        o
    }

    /// Whether music is playing (bars move) or not (bars fall and stop).
    pub fn set_playing(&self, playing: bool) {
        if self.imp().playing.replace(playing) != playing {
            self.update_ticking();
        }
    }

    fn enabled() -> bool {
        crate::config::settings().music_visualizer
            && STATUS.load(Ordering::Acquire) == 2
            && gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations())
    }

    pub fn update_ticking(&self) {
        let imp = self.imp();
        if imp.tick.borrow().is_some() || !self.is_mapped() {
            return;
        }
        let idle = imp.levels.borrow().iter().all(|v| *v <= 0.0);
        if !(imp.playing.get() && Self::enabled()) && idle {
            return;
        }
        imp.last_frame.set(0);
        let id = self.add_tick_callback(|w, clock| {
            let more = w.step(clock.frame_time());
            if more {
                glib::ControlFlow::Continue
            } else {
                w.imp().tick.take();
                glib::ControlFlow::Break
            }
        });
        imp.tick.replace(Some(id));
    }

    /// Advances the animation; false once everything has come to rest.
    fn step(&self, now: i64) -> bool {
        let imp = self.imp();
        let last = imp.last_frame.replace(now);
        let dt = if last == 0 { 1.0 / 60.0 } else { ((now - last) as f32 / 1e6).clamp(0.0, 0.1) };
        let live = imp.playing.get() && Self::enabled();
        let target = if live {
            crate::ui::ctx().player.audio_pts().and_then(levels_at).map(|m| self.normalize(&m, dt))
        } else {
            None
        };
        let mut levels = imp.levels.borrow_mut();
        let mut moving = false;
        for (k, v) in levels.iter_mut().enumerate() {
            let t = target.map_or(0.0, |t| t[k]);
            if t > *v {
                *v += (t - *v) * (dt * 28.0).min(1.0);
            } else {
                // Falls at a constant speed, like a meter.
                *v = (*v - dt * 1.4).max(t);
            }
            moving |= *v > 0.0;
        }
        drop(levels);
        self.queue_draw();
        live || moving
    }

    fn normalize(&self, m: &[f32; BANDS], dt: f32) -> [f32; BANDS] {
        let imp = self.imp();
        let mut db = [0.0f32; BANDS];
        for k in 0..BANDS {
            db[k] = 20.0 * m[k].max(1e-9).log10() + TILT[k];
        }
        let loudest = db.iter().copied().fold(f32::MIN, f32::max);
        let mut reference = imp.reference.get() - 5.0 * dt;
        if loudest > reference {
            reference = loudest;
        }
        imp.reference.set(reference);
        let mut out = [0.0f32; BANDS];
        // Near-silence shows nothing.
        if reference < -115.0 {
            return out;
        }
        for k in 0..BANDS {
            let v = ((db[k] - (reference - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
            out[k] = v * v;
        }
        out
    }
}

impl Default for Spectrum {
    fn default() -> Self {
        Self::new()
    }
}
