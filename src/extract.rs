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
    Ok(Streams { url, user_agent, available_at })
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
    fn single_format() {
        let v = serde_json::json!({"url": "https://x/y.m3u8"});
        let s = parse(&v).unwrap();
        assert_eq!(s.url, "https://x/y.m3u8");
        assert_eq!(s.available_at, None);
    }
}
