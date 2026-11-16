//! Telling YouTube what was watched, the way its web player does, so videos
//! land in the watch history with how far they got and YouTube can offer to
//! pick them up there (here, on youtube.com or on any other device).
//!
//! The InnerTube `player` response carries the reporting URLs
//! (`playbackTracking`) and, for a video watched part of the way, where to
//! continue (`playerConfig.playbackStartConfig.startSeconds`). A view then
//! sends one `api/stats/playback` request when playback starts and
//! `api/stats/watchtime` requests with the stretches of the video that
//! played: after 10, 20 and 30 seconds, every 40 seconds after that, on
//! pause, and a final one when the video is left.

use super::{Client, Flavor};
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

/// What the `player` endpoint says about a video for reporting and resuming.
#[derive(Clone, Debug, Default)]
pub struct Tracking {
    pub playback_url: Option<String>,
    pub watchtime_url: Option<String>,
    /// Where YouTube would continue the video (seconds).
    pub resume: Option<f64>,
    pub length: Option<f64>,
}

pub fn parse_tracking(v: &Value) -> Tracking {
    let url = |key: &str| {
        v.pointer(&format!("/playbackTracking/{key}/baseUrl"))
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("https://"))
            .map(str::to_string)
    };
    let num = |p: &str| {
        v.pointer(p).and_then(|x| x.as_f64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
    };
    Tracking {
        playback_url: url("videostatsPlaybackUrl"),
        watchtime_url: url("videostatsWatchtimeUrl"),
        resume: num("/playerConfig/playbackStartConfig/startSeconds").filter(|s| *s > 0.0),
        length: num("/videoDetails/lengthSeconds").filter(|l| *l > 0.0),
    }
}

impl Client {
    /// Asks the `player` endpoint (as youtube.com does when a video opens)
    /// for the video's reporting URLs and resume position. Streams still come
    /// from yt-dlp; this response's own are never used.
    pub async fn tracking(&self, video_id: &str) -> Result<Tracking> {
        let sts = self.signature_timestamp().await;
        let mut ctx = json!({ "html5Preference": "HTML5_PREF_WANTS" });
        if let Some(sts) = sts {
            ctx["signatureTimestamp"] = json!(sts);
        }
        let body = json!({
            "videoId": video_id,
            "racyCheckOk": true,
            "contentCheckOk": true,
            "playbackContext": { "contentPlaybackContext": ctx },
        });
        let v = self.post_as(Flavor::Web, "player", body).await?;
        let t = parse_tracking(&v);
        if t.watchtime_url.is_none() {
            let status = v.pointer("/playabilityStatus/status").and_then(Value::as_str).unwrap_or("?");
            let reason = v.pointer("/playabilityStatus/reason").and_then(Value::as_str).unwrap_or("");
            return Err(anyhow!("no playback tracking for {video_id} ({status} {reason})"));
        }
        Ok(t)
    }

    /// The player script version the `player` endpoint wants to hear about
    /// (youtube.com's page carries it as `STS`; without it the response is
    /// "The page needs to be reloaded").
    async fn signature_timestamp(&self) -> Option<u64> {
        if let Some(s) = self.session.read().unwrap().sts {
            return Some(s);
        }
        static TRIED: Mutex<bool> = Mutex::new(false);
        {
            let mut tried = TRIED.lock().unwrap();
            if *tried {
                return None;
            }
            *tried = true;
        }
        // Not in the page we bootstrapped from: read it from the player script.
        let get = |url: String| async move { self.http.get(url).send().await.ok()?.text().await.ok() };
        let api = get("https://www.youtube.com/iframe_api".into()).await?;
        let id = regex::Regex::new(r"player\\?/([0-9a-fA-F]{8})\\?/").ok()?.captures(&api)?.get(1)?.as_str().to_string();
        let js = get(format!("https://www.youtube.com/s/player/{id}/player_ias.vflset/en_US/base.js")).await?;
        let sts: u64 = regex::Regex::new(r"(?:signatureTimestamp|sts)\s*:\s*(\d{5})").ok()?.captures(&js)?.get(1)?.as_str().parse().ok()?;
        crate::log::info(&format!("player script {id}: signature timestamp {sts}"));
        self.session.write().unwrap().sts = Some(sts);
        Some(sts)
    }

    /// Sends one report.
    pub async fn ping(&self, url: &str) -> Result<()> {
        if std::env::var_os("GTUBE_STATS_DRY_RUN").is_some() {
            // The log drops query strings, so list what matters.
            let u = url::Url::parse(url)?;
            let q: HashMap<String, String> = u.query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
            let fields: Vec<String> = ["docid", "cmt", "st", "et", "state", "rt", "len", "rate", "final"]
                .iter()
                .filter_map(|k| q.get(*k).map(|v| format!("{k}={v}")))
                .collect();
            let kind = u.path().rsplit('/').next().unwrap_or("");
            crate::log::info(&format!("stats (dry run) {kind}: {}", fields.join(" ")));
            return Ok(());
        }
        let s = self.session();
        let mut req = self
            .http
            .get(url)
            .header("Referer", "https://www.youtube.com/")
            .header("Origin", "https://www.youtube.com")
            .header("Cookie", s.cookies.header_for("s.youtube.com"));
        if let Some(v) = &s.visitor_data {
            req = req.header("X-Goog-Visitor-Id", v);
        }
        let status = req.send().await?.status();
        if !status.is_success() {
            return Err(anyhow!("{status}"));
        }
        Ok(())
    }
}

/// Seconds as the web player writes them: at most 3 decimals, no trailing zeros.
fn secs(t: f64) -> String {
    let s = format!("{:.3}", t.max(0.0));
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A random client playback nonce, the id of one view.
fn nonce() -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_";
    let mut buf = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf));
    if random.is_err() {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (seed >> ((i % 8) * 8)) as u8 ^ (i as u8).wrapping_mul(37);
        }
    }
    buf.iter().map(|b| ALPHABET[(b & 63) as usize] as char).collect()
}

/// A stretch of the video that played without a jump.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Segment {
    start: f64,
    end: f64,
    rate: f64,
}

/// Seconds between watch-time reports while playing (the last one repeats).
const SCHEDULE: [f64; 4] = [10.0, 10.0, 10.0, 40.0];

/// One view of one video: collects what played and says which reports are due.
/// Times are in seconds on any steady clock (`now`).
pub struct Report {
    pub video: String,
    cpn: String,
    tracking: Option<Tracking>,
    live: bool,
    client_version: String,
    hl: String,
    gl: String,
    /// When playback first started.
    started: Option<f64>,
    playback_sent: bool,
    /// Played since the last report.
    done: Vec<Segment>,
    open: Option<Segment>,
    /// `now` at the last time update.
    last_now: f64,
    position: f64,
    length: f64,
    next_at: Option<f64>,
    step: usize,
    finished: bool,
}

impl Report {
    pub fn new(video: &str, live: bool, client_version: &str, hl: &str, gl: &str) -> Self {
        Report {
            video: video.to_string(),
            cpn: nonce(),
            tracking: None,
            live,
            client_version: client_version.to_string(),
            hl: hl.to_string(),
            gl: gl.to_string(),
            started: None,
            playback_sent: false,
            done: Vec::new(),
            open: None,
            last_now: 0.0,
            position: 0.0,
            length: 0.0,
            next_at: None,
            step: 0,
            finished: false,
        }
    }

    /// The reporting URLs arrived; returns reports that were already due.
    pub fn set_tracking(&mut self, t: Tracking, now: f64) -> Vec<String> {
        if self.length <= 0.0 {
            self.length = t.length.unwrap_or(0.0);
        }
        self.tracking = Some(t);
        let mut out = Vec::new();
        if self.started.is_some() {
            out.extend(self.playback());
            if self.next_at.is_some_and(|at| now >= at) {
                out.extend(self.watchtime("playing", false, now));
                self.schedule(now);
            }
        }
        out
    }

    pub fn set_length(&mut self, len: f64) {
        if len > 0.0 {
            self.length = len;
        }
    }

    /// Playback is at `t` (playing at `rate`); returns the reports now due.
    pub fn time(&mut self, t: f64, rate: f64, now: f64) -> Vec<String> {
        if self.finished || !t.is_finite() {
            return Vec::new();
        }
        let rate = if rate > 0.0 { rate } else { 1.0 };
        // Another speed: a new stretch, so each one has a single rate.
        if let Some(s) = self.open
            && (s.rate - rate).abs() > 0.01
        {
            self.close();
            self.open = Some(Segment { start: s.end, end: s.end, rate });
        }
        let expected = (now - self.last_now).max(0.0) * rate;
        match &mut self.open {
            // Moving on normally: the stretch grows.
            Some(s) if t >= s.end - 0.25 && t <= s.end + expected + 2.0 => s.end = s.end.max(t),
            // A jump (seek, skipped segment): the stretch ends, a new one begins.
            Some(_) => {
                self.close();
                self.open = Some(Segment { start: t, end: t, rate });
            }
            None => self.open = Some(Segment { start: t, end: t, rate }),
        }
        self.last_now = now;
        self.position = t;
        let mut out = Vec::new();
        if self.started.is_none() {
            self.started = Some(now);
        }
        // Starting, or playing again after a pause: reports follow the schedule.
        if self.next_at.is_none() {
            self.schedule(now);
        }
        out.extend(self.playback());
        if self.tracking.is_some() && self.next_at.is_some_and(|at| now >= at) {
            out.extend(self.watchtime("playing", false, now));
            self.schedule(now);
        }
        out
    }

    /// Playback paused (or stopped for good with `last`).
    pub fn pause(&mut self, now: f64) -> Vec<String> {
        self.close();
        if self.started.is_none() || self.done.is_empty() {
            return Vec::new();
        }
        let out = self.watchtime("paused", false, now);
        // Reports continue on the usual schedule once playing again.
        self.next_at = None;
        out
    }

    /// The view ends (another video, closing the player, quitting).
    pub fn finish(&mut self, now: f64) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.close();
        let out = if self.started.is_some() { self.watchtime("paused", true, now) } else { Vec::new() };
        self.finished = true;
        out
    }

    fn close(&mut self) {
        if let Some(s) = self.open.take()
            && s.end > s.start
        {
            self.done.push(s);
        }
    }

    fn schedule(&mut self, now: f64) {
        let gap = SCHEDULE[self.step.min(SCHEDULE.len() - 1)];
        self.step += 1;
        self.next_at = Some(now + gap);
    }

    fn rt(&self, now: f64) -> f64 {
        self.started.map(|s| now - s).unwrap_or(0.0)
    }

    fn common(&self, now: f64) -> Vec<(&'static str, String)> {
        let rt = secs(self.rt(now));
        let mut q = vec![
            ("ver", "2".to_string()),
            ("cpn", self.cpn.clone()),
            ("el", "detailpage".into()),
            ("ns", "yt".into()),
            ("cmt", secs(self.position)),
            ("fs", "0".into()),
            ("rt", rt.clone()),
            ("rtn", rt),
            ("lact", "0".into()),
            ("c", "WEB".into()),
            ("cver", self.client_version.clone()),
            ("cplayer", "UNIPLAYER".into()),
            ("cplatform", "DESKTOP".into()),
            ("hl", self.hl.clone()),
            ("cr", self.gl.clone()),
        ];
        if self.length > 0.0 {
            q.push(("len", secs(self.length)));
        }
        q
    }

    /// The one "a view started" report (adds the video to the history).
    fn playback(&mut self) -> Option<String> {
        if self.playback_sent {
            return None;
        }
        let base = self.tracking.as_ref()?.playback_url.clone()?;
        self.playback_sent = true;
        Some(with_params(&base, &self.common(self.started.unwrap_or(0.0))))
    }

    /// A watch-time report with what played since the last one.
    fn watchtime(&mut self, state: &str, last: bool, now: f64) -> Vec<String> {
        // Live streams count views, but there's no position to keep.
        if self.live {
            self.done.clear();
            return Vec::new();
        }
        let Some(base) = self.tracking.as_ref().and_then(|t| t.watchtime_url.clone()) else { return Vec::new() };
        let mut segs = std::mem::take(&mut self.done);
        if let Some(s) = &mut self.open {
            // Report the stretch that is still playing up to now; it carries on
            // from here in the next report.
            if s.end > s.start {
                segs.push(*s);
            }
            s.start = s.end;
        }
        if segs.is_empty() {
            if !last {
                return Vec::new();
            }
            segs.push(Segment { start: self.position, end: self.position, rate: 1.0 });
        }
        let list = |f: &dyn Fn(&Segment) -> String| segs.iter().map(f).collect::<Vec<_>>().join(",");
        let mut q = self.common(now);
        q.push(("state", state.to_string()));
        q.push(("st", list(&|s| secs(s.start))));
        q.push(("et", list(&|s| secs(s.end))));
        q.push(("volume", list(&|_| "100".into())));
        q.push(("muted", list(&|_| "0".into())));
        if segs.iter().any(|s| (s.rate - 1.0).abs() > 0.01) {
            q.push(("rate", list(&|s| secs(s.rate))));
        }
        if last {
            q.push(("final", "1".into()));
        }
        record(&self.video, self.position, self.length);
        vec![with_params(&base, &q)]
    }
}

/// `base` with `params` set (replacing any it already has).
fn with_params(base: &str, params: &[(&str, String)]) -> String {
    let Ok(mut url) = url::Url::parse(base) else { return base.to_string() };
    let keep: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !params.iter().any(|(p, _)| p == k))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.query_pairs_mut().clear().extend_pairs(keep).extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    url.to_string()
}

/// Positions reported during this run, so a video opened again right away
/// continues even before YouTube's own record catches up. Memory only.
static REPORTED: LazyLock<Mutex<HashMap<String, (f64, f64)>>> = LazyLock::new(Default::default);

fn finished(pos: f64, len: f64) -> bool {
    len - pos < (len * 0.05).clamp(20.0, 90.0)
}

fn record(video: &str, pos: f64, len: f64) {
    if len > 0.0 && pos.is_finite() {
        REPORTED.lock().unwrap().insert(video.to_string(), (pos, len));
    }
}

/// Where to continue `video` from what was reported this run: Some(0) means
/// from the start (it was watched to the end, or barely started).
pub fn reported_position(video: &str) -> Option<f64> {
    let (pos, len) = *REPORTED.lock().unwrap().get(video)?;
    Some(if finished(pos, len) || pos < 15.0 { 0.0 } else { pos })
}

/// How much of `video` was watched according to this run's reports (0..1).
pub fn reported_progress(video: &str) -> Option<f64> {
    let (pos, len) = *REPORTED.lock().unwrap().get(video)?;
    Some(if finished(pos, len) { 1.0 } else { (pos / len).clamp(0.0, 1.0) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracking() -> Tracking {
        Tracking {
            playback_url: Some("https://s.youtube.com/api/stats/playback?cl=1&docid=abc&ei=E&len=600&ns=yt&el=detailpage".into()),
            watchtime_url: Some("https://s.youtube.com/api/stats/watchtime?cl=1&docid=abc&ei=E&len=600&ns=yt&el=detailpage".into()),
            resume: None,
            length: Some(600.0),
        }
    }

    fn query(url: &str) -> HashMap<String, String> {
        url::Url::parse(url).unwrap().query_pairs().map(|(k, v)| (k.into_owned(), v.into_owned())).collect()
    }

    #[test]
    fn reads_the_player_response() {
        let v = json!({
            "playbackTracking": {
                "videostatsPlaybackUrl": {"baseUrl": "https://s.youtube.com/api/stats/playback?docid=x"},
                "videostatsWatchtimeUrl": {"baseUrl": "https://s.youtube.com/api/stats/watchtime?docid=x"},
            },
            "playerConfig": {"playbackStartConfig": {"startSeconds": 312}},
            "videoDetails": {"lengthSeconds": "798"},
        });
        let t = parse_tracking(&v);
        assert_eq!(t.resume, Some(312.0));
        assert_eq!(t.length, Some(798.0));
        assert!(t.watchtime_url.unwrap().contains("watchtime"));
        assert_eq!(parse_tracking(&json!({})).resume, None);
    }

    #[test]
    fn formats_like_the_web_player() {
        assert_eq!(secs(12.0), "12");
        assert_eq!(secs(12.3456), "12.346");
        assert_eq!(secs(0.5), "0.5");
        assert_eq!(secs(-1.0), "0");
        let n = nonce();
        assert_eq!(n.len(), 16);
        assert_ne!(n, nonce());
    }

    #[test]
    fn reports_what_played() {
        let mut r = Report::new("abc", false, "2.2026", "en", "US");
        r.set_length(600.0);
        // Plays before the URLs arrive: the start report waits for them.
        assert!(r.time(100.0, 1.0, 0.0).is_empty());
        assert!(r.time(102.0, 1.0, 2.0).is_empty());
        let out = r.set_tracking(tracking(), 2.5);
        assert_eq!(out.len(), 1);
        let p = query(&out[0]);
        assert!(out[0].starts_with("https://s.youtube.com/api/stats/playback?"));
        assert_eq!(p["docid"], "abc");
        assert_eq!(p["ver"], "2");
        assert_eq!(p["cpn"].len(), 16);

        // 10 seconds in: the first watch-time report.
        assert!(r.time(107.0, 1.0, 7.0).is_empty());
        let out = r.time(110.0, 1.0, 10.0);
        assert_eq!(out.len(), 1);
        let w = query(&out[0]);
        assert_eq!(w["cpn"], p["cpn"]);
        assert_eq!((w["st"].as_str(), w["et"].as_str(), w["cmt"].as_str()), ("100", "110", "110"));
        assert_eq!(w["state"], "playing");
        assert_eq!(w["len"], "600");
        assert!(!w.contains_key("final"));

        // A seek splits the stretch; pausing reports both parts.
        r.time(112.0, 1.0, 12.0);
        r.time(300.0, 1.0, 13.0);
        r.time(305.0, 1.0, 18.0);
        let out = r.pause(18.5);
        let w = query(&out[0]);
        assert_eq!((w["st"].as_str(), w["et"].as_str()), ("110,300", "112,305"));
        assert_eq!(w["state"], "paused");
        assert_eq!(w["volume"], "100,100");
        assert_eq!(reported_position("abc"), Some(305.0));

        // Leaving the video: a final report, then nothing more.
        r.time(305.0, 1.0, 30.0);
        r.time(306.5, 2.0, 31.0);
        let out = r.finish(32.0);
        let w = query(&out[0]);
        assert_eq!(w["final"], "1");
        assert_eq!((w["st"].as_str(), w["et"].as_str()), ("305", "306.5"));
        assert_eq!(w["rate"], "2");
        assert!(r.finish(33.0).is_empty());
        assert!(r.time(310.0, 1.0, 34.0).is_empty());
    }

    #[test]
    fn schedule_follows_the_web_player() {
        let mut r = Report::new("sched", false, "2", "en", "US");
        r.set_tracking(tracking(), 0.0);
        let mut at = Vec::new();
        for s in 0..=150 {
            if r.time(s as f64, 1.0, s as f64).iter().any(|u| u.contains("/watchtime")) {
                at.push(s);
            }
        }
        assert_eq!(at, vec![10, 20, 30, 70, 110, 150]);
    }

    #[test]
    fn watched_to_the_end_starts_over() {
        record("end", 590.0, 600.0);
        assert_eq!(reported_position("end"), Some(0.0));
        assert_eq!(reported_progress("end"), Some(1.0));
        record("mid", 300.0, 600.0);
        assert_eq!(reported_position("mid"), Some(300.0));
        assert_eq!(reported_progress("mid"), Some(0.5));
        assert_eq!(reported_position("never"), None);
    }

    #[test]
    fn live_streams_only_count_the_view() {
        let mut r = Report::new("live", true, "2", "en", "US");
        r.set_tracking(tracking(), 0.0);
        let out = r.time(5.0, 1.0, 0.0);
        assert!(out[0].contains("/playback"));
        r.time(15.0, 1.0, 10.0);
        assert!(r.finish(20.0).is_empty());
    }
}
