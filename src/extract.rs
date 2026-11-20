//! Stream extraction with yt-dlp.
//!
//! GTube runs yt-dlp itself rather than through mpv's ytdl_hook so it can
//! honour `available_at`: when YouTube's player response contains a
//! pre-roll ad, the stream URLs return 403 until that ad could have been
//! skipped. Waiting exactly that long (instead of letting ffmpeg retry with
//! exponential backoff) starts playback as early as YouTube allows.

use anyhow::{Result, anyhow};
use serde_json::Value;
use std::path::PathBuf;
use std::process::Stdio;

#[derive(Clone, Debug)]
pub struct Streams {
    /// What mpv should load: a plain URL, or an `edl://` joining separate
    /// video and audio streams into one file.
    pub url: String,
    pub user_agent: Option<String>,
    /// Unix time (seconds) before which the stream URLs are refused.
    pub available_at: Option<f64>,
    pub captions: Captions,
    /// The video heights there are formats for, tallest first.
    pub heights: Vec<u32>,
    /// The height of the video picked.
    pub height: Option<u32>,
}

/// A video's subtitles, as yt-dlp lists them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Captions {
    /// Uploaded tracks (by name), then auto-generated ones.
    pub tracks: Vec<CaptionTrack>,
    /// Languages YouTube can translate the subtitles into: (code, name),
    /// by name.
    pub translations: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CaptionTrack {
    /// Language code, e.g. "en" or "pt-BR".
    pub lang: String,
    /// e.g. "English" or "English (auto-generated)".
    pub name: String,
    /// Made by YouTube's speech recognition.
    pub auto: bool,
    /// The track's timedtext URL, without a format or translation.
    pub url: String,
}

impl CaptionTrack {
    /// This track as WebVTT, translated into `tlang` if given.
    pub fn vtt_url(&self, tlang: Option<&str>) -> String {
        let mut url = format!("{}&fmt=vtt", self.url);
        if let Some(t) = tlang {
            url.push_str("&tlang=");
            url.push_str(&urlencoding::encode(t));
        }
        url
    }
}

pub struct Request {
    pub ytdlp: String,
    pub url: String,
    pub format: String,
    pub cookies: Option<PathBuf>,
}

/// yt-dlp from PATH (which ends with the copy bundled in GTube's packages).
pub fn find_ytdlp() -> Option<String> {
    let name = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|p| p.join(name))
            .find(|p| p.is_file())
            .map(|p| p.to_string_lossy().into_owned())
    })
}

/// The yt-dlp to run: the one found, or its name (for a "not found" error).
pub fn ytdlp() -> String {
    find_ytdlp().unwrap_or_else(|| "yt-dlp".into())
}

pub fn format_string(audio_only: bool, q: u32) -> String {
    if audio_only {
        "bestaudio/best".into()
    } else {
        format!("bestvideo[height<=?{q}]+bestaudio/best[height<=?{q}]/best")
    }
}

pub async fn extract(req: Request) -> Result<Streams> {
    let mut cmd = tokio::process::Command::new(&req.ytdlp);
    cmd.args(["-J", "--no-playlist", "--no-progress", "-f", &req.format]);
    if let Some(c) = &req.cookies {
        cmd.arg("--cookies").arg(c);
    }
    cmd.arg("--").arg(&req.url);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = cmd.output().await.map_err(|e| anyhow!("could not run yt-dlp: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .rev()
            .find(|l| l.starts_with("ERROR"))
            .or_else(|| err.lines().rev().find(|l| !l.trim().is_empty()))
            .unwrap_or("yt-dlp failed");
        return Err(anyhow!("{}", line.trim()));
    }
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| anyhow!("unexpected yt-dlp output: {e}"))?;
    parse(&v)
}

/// EDL's length-prefixed quoting, so URLs may contain any character.
fn edl_quote(s: &str) -> String {
    format!("%{}%{}", s.len(), s)
}

pub fn parse(v: &Value) -> Result<Streams> {
    let formats: Vec<&Value> = match v.get("requested_formats").and_then(Value::as_array) {
        Some(a) if !a.is_empty() => a.iter().collect(),
        _ => vec![v],
    };
    let urls = formats
        .iter()
        .map(|f| f.get("url").and_then(Value::as_str).map(str::to_string))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| anyhow!("yt-dlp returned no stream URL"))?;
    let url = if urls.len() == 1 {
        urls[0].clone()
    } else {
        // Same layout mpv's ytdl_hook uses for split audio/video.
        let parts: Vec<String> = urls.iter().map(|u| format!("!no_clip;!no_chapters;{}", edl_quote(u))).collect();
        format!("edl://{}", parts.join(";!new_stream;"))
    };
    let user_agent = formats
        .iter()
        .chain(std::iter::once(&v))
        .find_map(|f| f.pointer("/http_headers/User-Agent").and_then(Value::as_str))
        .map(str::to_string);
    let available_at = formats
        .iter()
        .chain(std::iter::once(&v))
        .filter_map(|f| f.get("available_at").and_then(Value::as_f64))
        .reduce(f64::max);
    let height = |f: &Value| {
        // Storyboards (thumbnail sheets) have a height too.
        let video = f.get("vcodec").and_then(Value::as_str).is_none_or(|c| c != "none") && f.get("ext").and_then(Value::as_str) != Some("mhtml");
        f.get("height").and_then(Value::as_u64).filter(|_| video).map(|h| h as u32)
    };
    let mut heights: Vec<u32> = v.get("formats").and_then(Value::as_array).into_iter().flatten().filter_map(height).collect();
    heights.sort_unstable_by(|a, b| b.cmp(a));
    heights.dedup();
    let height = formats.iter().find_map(|f| height(f));
    Ok(Streams { url, user_agent, available_at, captions: parse_captions(v), heights, height })
}

/// A subtitle format's URL, if it is one of YouTube's timedtext URLs (yt-dlp
/// also lists live chat and HLS subtitle renditions).
fn timedtext_url(f: &Value) -> Option<reqwest::Url> {
    let u = reqwest::Url::parse(f.get("url")?.as_str()?).ok()?;
    (u.path() == "/api/timedtext").then_some(u)
}

fn query(u: &reqwest::Url, key: &str) -> Option<String> {
    u.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned())
}

/// `u` without its format and translation, so either can be added.
fn base_url(u: &reqwest::Url) -> String {
    let pairs: Vec<(String, String)> =
        u.query_pairs().filter(|(k, _)| k != "fmt" && k != "tlang").map(|(k, v)| (k.into_owned(), v.into_owned())).collect();
    let mut u = u.clone();
    u.query_pairs_mut().clear().extend_pairs(pairs);
    u.to_string()
}

pub fn parse_captions(v: &Value) -> Captions {
    let mut c = Captions::default();
    let first = |formats: &Value| {
        formats.as_array()?.iter().find_map(|f| {
            let name = f.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).map(str::to_string);
            timedtext_url(f).map(|u| (u, name))
        })
    };
    let mut auto = Vec::new();
    for (key, formats) in v.get("subtitles").and_then(Value::as_object).into_iter().flatten() {
        let Some((u, name)) = first(formats) else { continue };
        let url = base_url(&u);
        if !c.tracks.iter().any(|t| t.url == url) {
            let lang = query(&u, "lang").unwrap_or_else(|| key.clone());
            c.tracks.push(CaptionTrack { name: name.unwrap_or_else(|| key.clone()), lang, auto: false, url });
        }
    }
    // Automatic captions: the speech recognition track ("en-orig"; yt-dlp
    // lists it under the plain code as well), and every language YouTube
    // offers to translate into.
    for (key, formats) in v.get("automatic_captions").and_then(Value::as_object).into_iter().flatten() {
        let Some((u, name)) = first(formats) else { continue };
        if let Some(t) = query(&u, "tlang") {
            if !c.translations.iter().any(|(code, _)| *code == t) {
                c.translations.push((t, name.unwrap_or_else(|| key.clone())));
            }
        } else if query(&u, "kind").as_deref() == Some("asr") {
            let lang = query(&u, "lang").unwrap_or_else(|| key.trim_end_matches("-orig").to_string());
            if !auto.iter().any(|t: &CaptionTrack| t.lang == lang) {
                let name = name.unwrap_or_else(|| lang.clone());
                let name = name.strip_suffix(" (Original)").unwrap_or(&name);
                auto.push(CaptionTrack { name: format!("{name} (auto-generated)"), lang, auto: true, url: base_url(&u) });
            }
        }
    }
    // Videos dubbed by YouTube also have auto-generated subtitles for each
    // dub; only those in the language that is spoken go with the audio
    // played (the rest are among the translations).
    let primary = |code: &str| code.split(['-', '_']).next().unwrap_or(code).to_lowercase();
    if let Some(spoken) = v.get("language").and_then(Value::as_str).map(primary)
        && auto.iter().any(|t| primary(&t.lang) == spoken)
    {
        auto.retain(|t| primary(&t.lang) == spoken);
    }
    let by_name = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
    c.tracks.sort_by(|a, b| by_name(&a.name, &b.name));
    auto.sort_by(|a, b| by_name(&a.name, &b.name));
    c.tracks.extend(auto);
    c.translations.sort_by(|a, b| by_name(&a.1, &b.1));
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_streams_become_edl() {
        let v = serde_json::json!({
            "requested_formats": [
                {"url": "https://v.example/a,b", "available_at": 100, "http_headers": {"User-Agent": "UA, x"}},
                {"url": "https://a.example/c", "available_at": 104.5},
            ]
        });
        let s = parse(&v).unwrap();
        assert_eq!(
            s.url,
            "edl://!no_clip;!no_chapters;%21%https://v.example/a,b;!new_stream;!no_clip;!no_chapters;%19%https://a.example/c"
        );
        assert_eq!(s.user_agent.as_deref(), Some("UA, x"));
        assert_eq!(s.available_at, Some(104.5));
    }

    #[test]
    fn reads_captions() {
        let tt = |q: &str| format!("https://www.youtube.com/api/timedtext?v=x&ei=y&{q}&signature=z");
        let v = serde_json::json!({
            "url": "https://x/y",
            "language": "en-US",
            "subtitles": {
                "en": [{"ext": "json3", "name": "English", "url": tt("lang=en&fmt=json3")}, {"ext": "vtt", "name": "English", "url": tt("lang=en&fmt=vtt")}],
                "de-DE": [{"ext": "vtt", "name": "German (Germany)", "url": tt("lang=de-DE&fmt=vtt")}],
                "live_chat": [{"ext": "json", "url": "https://www.youtube.com/watch?v=x&bpctr=1"}],
            },
            "automatic_captions": {
                "en-orig": [{"ext": "vtt", "name": "English (Original)", "url": tt("lang=en&kind=asr&fmt=vtt")}],
                "ar-orig": [{"ext": "vtt", "name": "Arabic (Original)", "url": tt("lang=ar&kind=asr&fmt=vtt")}],
                "en": [{"ext": "vtt", "url": "https://manifest.googlevideo.com/api/manifest/hls_timedtext_playlist/x"},
                       {"ext": "vtt", "name": "English", "url": tt("lang=en&kind=asr&fmt=vtt")}],
                "it": [{"ext": "vtt", "name": "Italian", "url": tt("lang=en&tlang=it&kind=asr&fmt=vtt")}],
                "af": [{"ext": "vtt", "name": "Afrikaans", "url": tt("lang=en&tlang=af&kind=asr&fmt=vtt")}],
            },
        });
        let c = parse(&v).unwrap().captions;
        let names: Vec<_> = c.tracks.iter().map(|t| (t.name.as_str(), t.lang.as_str(), t.auto)).collect();
        assert_eq!(names, [("English", "en", false), ("German (Germany)", "de-DE", false), ("English (auto-generated)", "en", true)]);
        assert_eq!(c.translations, [("af".to_string(), "Afrikaans".to_string()), ("it".to_string(), "Italian".to_string())]);
        assert_eq!(c.tracks[0].url, tt("lang=en"));
        assert_eq!(c.tracks[2].vtt_url(Some("pt-BR")), tt("lang=en&kind=asr") + "&fmt=vtt&tlang=pt-BR");
        assert_eq!(parse(&serde_json::json!({"url": "u"})).unwrap().captions, Captions::default());
    }

    #[test]
    fn lists_heights() {
        let v = serde_json::json!({
            "formats": [
                {"vcodec": "none", "height": null}, {"vcodec": "avc1", "height": 360}, {"vcodec": "vp9", "height": 1080},
                {"vcodec": "avc1", "height": 1080}, {"vcodec": "none", "acodec": "none", "ext": "mhtml", "height": 90}, {"vcodec": "none", "acodec": "opus"},
            ],
            "requested_formats": [{"url": "v", "vcodec": "vp9", "height": 1080}, {"url": "a", "vcodec": "none"}],
        });
        let s = parse(&v).unwrap();
        assert_eq!(s.heights, [1080, 360]);
        assert_eq!(s.height, Some(1080));
    }

    #[test]
    fn single_format() {
        let v = serde_json::json!({"url": "https://x/y.m3u8"});
        let s = parse(&v).unwrap();
        assert_eq!(s.url, "https://x/y.m3u8");
        assert_eq!(s.available_at, None);
    }
}
