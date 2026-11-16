//! Return YouTube Dislike and SponsorBlock.

use anyhow::Result;
use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize)]
pub struct Votes {
    pub likes: Option<i64>,
    pub dislikes: Option<i64>,
}

pub async fn dislikes(http: &reqwest::Client, video_id: &str) -> Result<Votes> {
    Ok(http
        .get(format!("https://returnyoutubedislikeapi.com/votes?videoId={video_id}"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

pub const SB_CATEGORIES: &[(&str, &str, &str)] = &[
    ("sponsor", "Sponsor", "#00d400"),
    ("selfpromo", "Unpaid/Self promotion", "#ffff00"),
    ("interaction", "Interaction reminder", "#cc00ff"),
    ("intro", "Intermission/Intro", "#00ffff"),
    ("outro", "Endcards/Credits", "#0202ed"),
    ("preview", "Preview/Recap", "#008fd6"),
    ("hook", "Hook/Greetings", "#395699"),
    ("music_offtopic", "Non-music section", "#ff9900"),
    ("filler", "Filler tangent", "#7300ff"),
    ("poi_highlight", "Highlight", "#ff1684"),
    ("exclusive_access", "Exclusive access", "#008a5c"),
];

pub fn sb_label(cat: &str) -> &'static str {
    SB_CATEGORIES.iter().find(|c| c.0 == cat).map(|c| c.1).unwrap_or("Segment")
}

pub fn sb_color(cat: &str) -> &'static str {
    SB_CATEGORIES.iter().find(|c| c.0 == cat).map(|c| c.2).unwrap_or("#888888")
}

#[derive(Clone, Debug)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub category: String,
    pub action: String,
}

#[derive(Deserialize)]
struct SbVideo {
    #[serde(rename = "videoID")]
    video_id: String,
    segments: Vec<SbSegment>,
}

#[derive(Deserialize)]
struct SbSegment {
    segment: [f64; 2],
    category: String,
    #[serde(rename = "actionType")]
    action_type: String,
}

/// Uses the k-anonymity endpoint: only the first 4 hex characters of the
/// video id's SHA-256 are sent, so SponsorBlock never learns what you watch.
/// SponsorBlock's server. `GTUBE_SPONSORBLOCK_API` points GTube at
/// another one (a local test server, or a mirror).
pub fn sb_api() -> String {
    std::env::var("GTUBE_SPONSORBLOCK_API")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "https://sponsor.ajay.app".into())
        .trim_end_matches('/')
        .to_string()
}

pub async fn segments(http: &reqwest::Client, video_id: &str) -> Result<Vec<Segment>> {
    let hash = hex::encode(Sha256::digest(video_id.as_bytes()));
    let cats: Vec<&str> = SB_CATEGORIES.iter().map(|c| c.0).collect();
    let url = format!(
        "{}/api/skipSegments/{}?categories={}&actionTypes={}",
        sb_api(),
        &hash[..4],
        urlencoding::encode(&serde_json::to_string(&cats)?),
        urlencoding::encode(r#"["skip","mute","full","poi"]"#)
    );
    let resp = http.get(url).send().await?;
    if resp.status().as_u16() == 404 {
        return Ok(Vec::new());
    }
    let videos: Vec<SbVideo> = resp.error_for_status()?.json().await?;
    let mut segs: Vec<Segment> = videos
        .into_iter()
        .filter(|v| v.video_id == video_id)
        .flat_map(|v| v.segments)
        .map(|s| Segment { start: s.segment[0], end: s.segment[1], category: s.category, action: s.action_type })
        .collect();
    segs.sort_by(|a, b| a.start.total_cmp(&b.start));
    Ok(segs)
}

/// Submits a segment the user marked. `user_id` is the private, random ID
/// that SponsorBlock credits submissions to.
pub async fn submit_segment(http: &reqwest::Client, video_id: &str, user_id: &str, duration: f64, seg: &Segment) -> Result<()> {
    submit_segment_to(http, &sb_api(), video_id, user_id, duration, seg).await
}

async fn submit_segment_to(http: &reqwest::Client, api: &str, video_id: &str, user_id: &str, duration: f64, seg: &Segment) -> Result<()> {
    let body = serde_json::json!({
        "videoID": video_id,
        "userID": user_id,
        "userAgent": format!("GTube/{}", env!("CARGO_PKG_VERSION")),
        "service": "YouTube",
        "videoDuration": duration,
        "segments": [{ "segment": [seg.start, seg.end], "category": seg.category, "actionType": seg.action }],
    });
    let resp = http.post(format!("{api}/api/skipSegments")).json(&body).send().await?;
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let text = resp.text().await.unwrap_or_default();
    let text = text.trim();
    let why = match status.as_u16() {
        400 => "SponsorBlock found the segment invalid",
        403 => "SponsorBlock's moderation rejected the segment",
        409 => "This segment has already been submitted",
        429 => "Too many submissions, try again later",
        _ => "SponsorBlock couldn't take the segment",
    };
    if text.is_empty() || text.starts_with('<') {
        anyhow::bail!("{why} ({status})")
    }
    anyhow::bail!("{why}: {}", text.chars().take(300).collect::<String>())
}

pub fn format_count(n: i64) -> String {
    let f = n as f64;
    if n >= 1_000_000_000 {
        format!("{:.1}B", f / 1e9).replace(".0B", "B")
    } else if n >= 1_000_000 {
        format!("{:.1}M", f / 1e6).replace(".0M", "M")
    } else if n >= 10_000 {
        format!("{:.0}K", f / 1e3)
    } else if n >= 1_000 {
        format!("{:.1}K", f / 1e3).replace(".0K", "K")
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    /// A one-request HTTP server on localhost: answers `status` and hands
    /// back the request it got.
    fn serve_once(status: &'static str, body: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            // Headers, then as much body as Content-Length says.
            loop {
                let n = s.read(&mut buf).unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req).to_string();
                if let Some(h) = text.find("\r\n\r\n") {
                    let len = text
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                        .unwrap_or(0);
                    if req.len() >= h + 4 + len {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let resp = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            s.write_all(resp.as_bytes()).unwrap();
            String::from_utf8_lossy(&req).to_string()
        });
        (base, handle)
    }

    fn seg() -> Segment {
        Segment { start: 12.5, end: 40.0, category: "sponsor".into(), action: "skip".into() }
    }

    #[tokio::test]
    async fn submits_a_segment() {
        let (base, server) = serve_once("200 OK", "[]");
        let http = reqwest::Client::new();
        submit_segment_to(&http, &base, "abcdefghijk", "u".repeat(48).as_str(), 300.0, &seg()).await.unwrap();
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /api/skipSegments "), "{req}");
        let body: serde_json::Value = serde_json::from_str(req.split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(body["videoID"], "abcdefghijk");
        assert_eq!(body["userID"].as_str().unwrap().len(), 48);
        assert_eq!(body["videoDuration"], 300.0);
        assert_eq!(body["service"], "YouTube");
        assert_eq!(body["segments"][0]["segment"], serde_json::json!([12.5, 40.0]));
        assert_eq!(body["segments"][0]["category"], "sponsor");
        assert_eq!(body["segments"][0]["actionType"], "skip");
        assert!(body["userAgent"].as_str().unwrap().starts_with("GTube/"));
    }

    #[tokio::test]
    async fn explains_rejections() {
        let (base, server) = serve_once("409 Conflict", "");
        let err = submit_segment_to(&reqwest::Client::new(), &base, "abcdefghijk", "u", 300.0, &seg()).await.unwrap_err();
        server.join().unwrap();
        assert!(err.to_string().contains("already been submitted"), "{err}");
        let (base, server) = serve_once("403 Forbidden", "Submission rejected due to a tip from a moderator.");
        let err = submit_segment_to(&reqwest::Client::new(), &base, "abcdefghijk", "u", 300.0, &seg()).await.unwrap_err();
        server.join().unwrap();
        assert!(err.to_string().contains("moderat"), "{err}");
        assert!(err.to_string().contains("tip from a moderator"), "{err}");
    }
}
