//! Song lyrics, timed line by line when possible. YouTube Music's own come
//! first: its Android app gets them with each line's time, which the web
//! client doesn't. When those aren't timed (or there are none), LRCLIB
//! (lrclib.net, an open database of synced lyrics) is asked; untimed text
//! from either is the last resort.

use super::{Client, MUSIC_ORIGIN};
use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::{Value, json};

/// The Android YouTube Music app this asks as (any recent one works).
const ANDROID_MUSIC_VERSION: &str = "7.21.50";
const LRCLIB: &str = "https://lrclib.net/api";

/// One line of timed lyrics.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Seconds into the song.
    pub start: f64,
    pub text: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lyrics {
    /// The whole text, one line per line.
    pub text: String,
    /// The lines with their times; empty when the lyrics aren't timed.
    pub lines: Vec<Line>,
    /// Who provides them ("Source: Musixmatch", "LRCLIB").
    pub source: Option<String>,
    /// LRCLIB knows the song has no words.
    pub instrumental: bool,
}

impl Lyrics {
    pub fn plain(text: String, source: Option<String>) -> Self {
        Self { text, lines: Vec::new(), source, instrumental: false }
    }

    pub fn timed(mut lines: Vec<Line>, source: Option<String>) -> Self {
        lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        let text = lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n");
        Self { text, lines, source, instrumental: false }
    }

    pub fn is_timed(&self) -> bool {
        !self.lines.is_empty()
    }

    /// The line being sung at `t` seconds (None before the first one).
    pub fn line_at(&self, t: f64) -> Option<usize> {
        self.lines.partition_point(|l| l.start <= t).checked_sub(1)
    }
}

/// The song LRCLIB is asked about.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Song {
    pub title: String,
    pub artists: Vec<String>,
    /// Seconds.
    pub duration: Option<f64>,
}

// ---------------------------------------------------------------------------
// YouTube Music

/// The Android app's lyrics (`timedLyricsModel`): timed when the lines carry
/// cue ranges, plain when YouTube only has static lyrics for the song.
pub fn parse_timed(v: &Value) -> Option<Lyrics> {
    fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
        match v {
            Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find(x, key))),
            Value::Array(a) => a.iter().find_map(|x| find(x, key)),
            _ => None,
        }
    }
    let data = find(v, "timedLyricsModel")?.get("lyricsData")?;
    let source = data.get("sourceMessage").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let mut lines = Vec::new();
    let mut timed = true;
    for e in data.get("timedLyricsData")?.as_array()? {
        let text = e.get("lyricLine").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let start = e.pointer("/cueRange/startTimeMilliseconds").and_then(|x| x.as_str().and_then(|s| s.parse::<f64>().ok()).or(x.as_f64()));
        timed &= start.is_some();
        lines.push(Line { start: start.unwrap_or(0.0) / 1000.0, text });
    }
    if lines.iter().all(|l| l.text.is_empty()) {
        return None;
    }
    Some(if timed {
        Lyrics::timed(lines, source)
    } else {
        Lyrics::plain(lines.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"), source)
    })
}

impl Client {
    /// YouTube Music's lyrics for a song's lyrics tab (`MPLY…`), asked for as
    /// the Android app so they come timed when YouTube has the timing. Sent
    /// without the session: lyrics are the same for everyone.
    async fn android_lyrics(&self, browse_id: &str) -> Result<Option<Lyrics>> {
        let (hl, gl) = {
            let s = self.session();
            (s.hl.clone(), s.gl.clone())
        };
        let body = json!({
            "browseId": browse_id,
            "context": { "client": {
                "clientName": "ANDROID_MUSIC",
                "clientVersion": ANDROID_MUSIC_VERSION,
                "androidSdkVersion": 34,
                "hl": hl,
                "gl": gl,
            } },
        });
        let v: Value = self
            .http
            .post(format!("{MUSIC_ORIGIN}/youtubei/v1/browse?prettyPrint=false"))
            .header("User-Agent", format!("com.google.android.apps.youtube.music/{ANDROID_MUSIC_VERSION} (Linux; U; Android 14) gzip"))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
            .context("invalid lyrics response")?;
        Ok(parse_timed(&v).or_else(|| super::music::parse_lyrics(&v)))
    }

    /// The lyrics tab as the web client sees it (never timed).
    async fn web_lyrics(&self, browse_id: &str) -> Result<Option<Lyrics>> {
        Ok(super::music::parse_lyrics(&self.music_browse(browse_id, None).await?))
    }

    pub async fn music_lyrics(&self, browse_id: &str) -> Result<Option<Lyrics>> {
        match self.android_lyrics(browse_id).await {
            Ok(Some(l)) => Ok(Some(l)),
            Ok(None) => self.web_lyrics(browse_id).await,
            Err(e) => {
                crate::log::warn(&format!("lyrics as the Android app: {e:#}"));
                self.web_lyrics(browse_id).await
            }
        }
    }
}

// ---------------------------------------------------------------------------
// LRCLIB

/// `mm:ss.xx` (also `mm:ss`, `mm:ss:xx`, `hh:mm:ss.xx`) in seconds.
fn lrc_time(tag: &str) -> Option<f64> {
    let parts: Vec<&str> = tag.split(':').collect();
    if parts.len() < 2 || parts.len() > 3 || !parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',')) {
        return None;
    }
    let num = |s: &str| s.replace(',', ".").parse::<f64>().ok();
    match parts[..] {
        // mm:ss:xx (hundredths after a colon) or hh:mm:ss.xx
        [a, b, c] if !c.contains('.') && c.len() <= 2 && !b.contains('.') => Some(num(a)? * 60.0 + num(b)? + num(c)? / 100.0),
        [h, m, s] => Some(num(h)? * 3600.0 + num(m)? * 60.0 + num(s)?),
        [m, s] => Some(num(m)? * 60.0 + num(s)?),
        _ => None,
    }
}

/// Drops enhanced LRC's per-word times (`<00:12.34>`).
fn strip_word_times(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        match rest[i + 1..].find('>') {
            Some(j) if lrc_time(&rest[i + 1..i + 1 + j]).is_some() => {
                out.push_str(&rest[..i]);
                rest = &rest[i + j + 2..];
            }
            _ => {
                out.push_str(&rest[..=i]);
                rest = &rest[i + 1..];
            }
        }
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Synced lyrics in the LRC format: `[mm:ss.xx]text` lines, possibly with
/// several times per line, ID tags and an `[offset:±ms]`.
pub fn parse_lrc(s: &str) -> Vec<Line> {
    let mut offset = 0.0;
    let mut lines = Vec::new();
    for raw in s.lines() {
        let mut rest = raw.trim();
        let mut times = Vec::new();
        while let Some(r) = rest.strip_prefix('[') {
            let Some(end) = r.find(']') else { break };
            let tag = &r[..end];
            if let Some(t) = lrc_time(tag) {
                times.push(t);
            } else if let Some(o) = tag.strip_prefix("offset:") {
                // Positive: the lyrics come sooner.
                offset = o.trim().trim_start_matches('+').parse::<f64>().unwrap_or(0.0) / 1000.0;
            }
            rest = r[end + 1..].trim_start();
        }
        let text = strip_word_times(rest);
        lines.extend(times.into_iter().map(|t| Line { start: t, text: text.clone() }));
    }
    for l in &mut lines {
        l.start = (l.start - offset).max(0.0);
    }
    lines.sort_by(|a, b| a.start.total_cmp(&b.start));
    lines
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LrclibRecord {
    pub track_name: String,
    pub artist_name: String,
    pub duration: Option<f64>,
    pub instrumental: bool,
    pub plain_lyrics: Option<String>,
    pub synced_lyrics: Option<String>,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

impl LrclibRecord {
    fn synced(&self) -> Option<Vec<Line>> {
        let lines = parse_lrc(self.synced_lyrics.as_deref()?);
        lines.iter().any(|l| !l.text.is_empty()).then_some(lines)
    }

    fn plain(&self) -> Option<String> {
        self.plain_lyrics.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_string)
    }

    /// Lyrics for the song, when this is the song: the same artist, and
    /// (for timing) the same length.
    fn lyrics_for(&self, song: &Song, timed_only: bool) -> Option<Lyrics> {
        let artist = song.artists.first().map(|a| norm(a)).unwrap_or_default();
        let theirs = norm(&self.artist_name);
        if !artist.is_empty() && !theirs.contains(&artist) && !artist.contains(&theirs) {
            return None;
        }
        let same_length = match (song.duration, self.duration) {
            (Some(a), Some(b)) => (a - b).abs() <= 3.0,
            _ => true,
        };
        let source = Some("LRCLIB".to_string());
        if same_length && let Some(lines) = self.synced() {
            return Some(Lyrics::timed(lines, source));
        }
        if timed_only {
            return None;
        }
        if self.instrumental {
            return Some(Lyrics { instrumental: true, source, ..Default::default() });
        }
        self.plain().map(|t| Lyrics::plain(t, source))
    }
}

/// The best of LRCLIB's search results: timed lyrics of the same length
/// first, else the first plain ones.
pub fn pick_lrclib(records: &[LrclibRecord], song: &Song) -> Option<Lyrics> {
    let title = norm(&song.title);
    let titled = |r: &&LrclibRecord| title.is_empty() || norm(&r.track_name) == title;
    let ordered: Vec<&LrclibRecord> = records.iter().filter(titled).chain(records.iter().filter(|r| !titled(r))).collect();
    ordered.iter().find_map(|r| r.lyrics_for(song, true)).or_else(|| ordered.iter().find_map(|r| r.lyrics_for(song, false)))
}

/// "Song (Official Video)" → "Song": what music video titles add.
pub fn clean_title(t: &str) -> String {
    const NOISE: &[&str] = &[
        "official", "video", "audio", "lyric", "lyrics", "visualizer", "visualiser", "remaster", "remastered", "hd", "4k", "mv",
        "clip", "explicit",
    ];
    let mut out = String::new();
    let mut rest = t;
    while let Some(i) = rest.find(['(', '[']) {
        let close = if rest[i..].starts_with('(') { ')' } else { ']' };
        let Some(j) = rest[i..].find(close).map(|j| i + j) else { break };
        out.push_str(&rest[..i]);
        let inner = rest[i + 1..j].to_lowercase();
        if !inner.split(|c: char| !c.is_alphanumeric()).any(|w| NOISE.contains(&w)) {
            out.push_str(&rest[i..=j]);
        }
        rest = &rest[j + 1..];
    }
    out.push_str(rest);
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Client {
    async fn lrclib_get(&self, path: &str, query: &[(&str, String)]) -> Result<Option<Value>> {
        let url = reqwest::Url::parse_with_params(&format!("{LRCLIB}/{path}"), query)?;
        let resp = self.http.get(url).header("User-Agent", concat!("GTube/", env!("CARGO_PKG_VERSION"))).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(resp.error_for_status()?.json().await.context("invalid LRCLIB response")?))
    }

    /// LRCLIB's lyrics for the song: its exact match (same title, artist
    /// and length) when that is timed, else the best search result.
    pub async fn lrclib(&self, song: &Song) -> Result<Option<Lyrics>> {
        let title = clean_title(&song.title);
        if title.is_empty() || song.artists.is_empty() {
            return Ok(None);
        }
        let song = Song { title, ..song.clone() };
        let mut exact = None;
        if let Some(d) = song.duration {
            // (No album: YouTube Music's album names often differ.)
            let q = [
                ("track_name", song.title.clone()),
                ("artist_name", song.artists.join(", ")),
                ("duration", format!("{}", d.round() as i64)),
            ];
            if let Some(v) = self.lrclib_get("get", &q).await? {
                let r: LrclibRecord = serde_json::from_value(v).unwrap_or_default();
                exact = r.lyrics_for(&song, false);
                if exact.as_ref().is_some_and(|l| l.is_timed() || l.instrumental) {
                    return Ok(exact);
                }
            }
        }
        let q = [("track_name", song.title.clone()), ("artist_name", song.artists[0].clone())];
        let found = match self.lrclib_get("search", &q).await? {
            Some(v) => pick_lrclib(&serde_json::from_value::<Vec<LrclibRecord>>(v).unwrap_or_default(), &song),
            None => None,
        };
        Ok(match (found, exact) {
            (Some(f), _) if f.is_timed() => Some(f),
            (f, e) => e.or(f),
        })
    }
}

/// The best lyrics there are for a song: YouTube Music's when timed, else
/// LRCLIB's when timed, else whichever has the text (YouTube's first).
/// `lyrics_id` is the song's lyrics tab on YouTube Music, if it has one.
pub async fn find(client: &Client, lyrics_id: Option<&str>, song: &Song, use_lrclib: bool) -> Option<Lyrics> {
    let mut plain = None;
    if let Some(id) = lyrics_id {
        match client.music_lyrics(id).await {
            Ok(Some(l)) if l.is_timed() => return Some(l),
            Ok(l) => plain = l,
            Err(e) => crate::log::warn(&format!("lyrics: {e:#}")),
        }
    }
    if use_lrclib {
        match client.lrclib(song).await {
            Ok(Some(l)) if l.is_timed() => return Some(l),
            Ok(Some(l)) if plain.is_none() => plain = Some(l),
            Ok(_) => {}
            Err(e) => crate::log::warn(&format!("LRCLIB: {e:#}")),
        }
    }
    plain
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_lrc() {
        let lrc = "[ar:Someone]\n[offset:+500]\n[00:12.50]First line\n[00:10.00][01:02.25]Chorus\n\n[00:15.00]\n[1:05:00.00]Late\nno time here\n";
        let lines = parse_lrc(lrc);
        let got: Vec<(f64, &str)> = lines.iter().map(|l| (l.start, l.text.as_str())).collect();
        assert_eq!(got, vec![(9.5, "Chorus"), (12.0, "First line"), (14.5, ""), (61.75, "Chorus"), (3899.5, "Late")]);
    }

    #[test]
    fn drops_word_times() {
        let lines = parse_lrc("[00:18.89] <00:18.89>We're <00:19.25>no <00:19.53>strangers <00:20.07>to <00:20.33>love\n[00:22.00]a <b> c");
        assert_eq!(lines[0].text, "We're no strangers to love");
        assert_eq!(lines[1].text, "a <b> c");
        assert!((lrc_time("01:02:03").unwrap() - 62.03).abs() < 1e-9);
        assert_eq!(lrc_time("ar"), None);
    }

    #[test]
    fn finds_the_current_line() {
        let l = Lyrics::timed(vec![Line { start: 5.0, text: "b".into() }, Line { start: 1.0, text: "a".into() }], None);
        assert_eq!(l.text, "a\nb");
        assert_eq!(l.line_at(0.5), None);
        assert_eq!(l.line_at(1.0), Some(0));
        assert_eq!(l.line_at(4.9), Some(0));
        assert_eq!(l.line_at(100.0), Some(1));
    }

    #[test]
    fn reads_youtube_timed_lyrics() {
        let v = json!({"contents": {"elementRenderer": {"newElement": {"type": {"componentType": {"model": {"timedLyricsModel": {"lyricsData": {
            "timedLyricsData": [
                {"lyricLine": "Two", "cueRange": {"startTimeMilliseconds": "20450", "endTimeMilliseconds": "24930"}},
                {"lyricLine": "One", "cueRange": {"startTimeMilliseconds": "1000", "endTimeMilliseconds": "2000"}},
                {"lyricLine": "♪", "cueRange": {"startTimeMilliseconds": "30000", "endTimeMilliseconds": "40000"}}
            ],
            "sourceMessage": "Source: Musixmatch"
        }}}}}}}}});
        let l = parse_timed(&v).unwrap();
        assert!(l.is_timed());
        assert_eq!(l.lines[0], Line { start: 1.0, text: "One".into() });
        assert_eq!(l.lines[1].start, 20.45);
        assert_eq!(l.source.as_deref(), Some("Source: Musixmatch"));
        // Static lyrics: lines without cue ranges.
        let v = json!({"timedLyricsModel": {"lyricsData": {"timedLyricsData": [{"lyricLine": "a"}, {"lyricLine": "b"}]}}});
        let l = parse_timed(&v).unwrap();
        assert!(!l.is_timed());
        assert_eq!(l.text, "a\nb");
    }

    fn record(artist: &str, title: &str, duration: f64, synced: Option<&str>, plain: Option<&str>) -> LrclibRecord {
        LrclibRecord {
            track_name: title.into(),
            artist_name: artist.into(),
            duration: Some(duration),
            instrumental: false,
            plain_lyrics: plain.map(str::to_string),
            synced_lyrics: synced.map(str::to_string),
        }
    }

    #[test]
    fn picks_matching_lrclib_lyrics() {
        let song = Song { title: "Song".into(), artists: vec!["The Band".into()], duration: Some(200.0) };
        let records = vec![
            record("Other", "Song", 200.0, Some("[00:01.00]wrong artist"), None),
            record("The Band", "Song (Live)", 260.0, Some("[00:01.00]too long"), Some("live text")),
            record("The Band feat. X", "Song", 201.5, Some("[00:02.00]right"), Some("right")),
        ];
        let l = pick_lrclib(&records, &song).unwrap();
        assert!(l.is_timed());
        assert_eq!(l.lines[0].text, "right");
        assert_eq!(l.source.as_deref(), Some("LRCLIB"));
        // Nothing of the same length is timed: plain text will do.
        let l = pick_lrclib(&records[..2], &song).unwrap();
        assert!(!l.is_timed());
        assert_eq!(l.text, "live text");
        assert!(pick_lrclib(&records[..1], &song).is_none());
    }

    #[test]
    fn cleans_video_titles() {
        assert_eq!(clean_title("Never Gonna Give You Up (Official Video) (4K Remaster)"), "Never Gonna Give You Up");
        assert_eq!(clean_title("Song [Official Music Video]"), "Song");
        assert_eq!(clean_title("Song (feat. Someone) [Lyric Video]"), "Song (feat. Someone)");
        assert_eq!(clean_title("Song (Acoustic)"), "Song (Acoustic)");
        assert_eq!(clean_title("Song (2022 Remaster)"), "Song");
        assert_eq!(clean_title("Odd (title"), "Odd (title");
    }
}
