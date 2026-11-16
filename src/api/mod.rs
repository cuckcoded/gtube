//! InnerTube (YouTube's internal web API) client.
//!
//! Requests are made as the regular desktop web client. When the user is
//! signed in, the browser session cookies are sent along with the
//! `SAPISIDHASH` authorization header the website itself computes, so feeds,
//! history and playlists match what youtube.com shows.

pub mod cookies;
pub mod extras;
pub mod lyrics;
pub mod models;
pub mod music;
pub mod parse;
pub mod stats;

use anyhow::{Context, Result, anyhow, bail};
use cookies::CookieJar;
use models::*;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use std::sync::{Arc, OnceLock, RwLock};

pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";
const ORIGIN: &str = "https://www.youtube.com";
const FALLBACK_CLIENT_VERSION: &str = "2.20261112.10.00";
const MUSIC_ORIGIN: &str = "https://music.youtube.com";
const MUSIC_FALLBACK_CLIENT_VERSION: &str = "1.20261110.04.00";

/// Which web client a request impersonates: youtube.com or YouTube Music.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Web,
    Music,
}

impl Flavor {
    fn origin(self) -> &'static str {
        match self {
            Flavor::Web => ORIGIN,
            Flavor::Music => MUSIC_ORIGIN,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Session {
    pub cookies: CookieJar,
    pub client_version: String,
    pub visitor_data: Option<String>,
    pub user_session_id: Option<String>,
    pub delegated_session_id: Option<String>,
    pub session_index: String,
    pub logged_in: bool,
    pub hl: String,
    pub gl: String,
    pub music_client_version: String,
    /// music.youtube.com's config was fetched (or that was attempted).
    pub music_bootstrapped: bool,
    /// The player script's signature timestamp (youtube.com's `STS`).
    pub sts: Option<u64>,
}

pub struct Client {
    http: reqwest::Client,
    session: RwLock<Session>,
}

static CLIENT: OnceLock<Arc<Client>> = OnceLock::new();

pub fn init(cookies: CookieJar, hl: String, gl: String) {
    let _ = CLIENT.set(Arc::new(Client::new(cookies, hl, gl)));
}

pub fn client() -> Arc<Client> {
    CLIENT.get().expect("api::init not called").clone()
}

pub enum LikeAction {
    Like,
    Dislike,
    Remove,
}

fn ytcfg_value(html: &str, key: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(r#""{key}"\s*:\s*("(?:[^"\\]|\\.)*"|true|false|-?\d+)"#)).ok()?;
    let raw = re.captures(html)?.get(1)?.as_str();
    if raw.starts_with('"') {
        serde_json::from_str::<String>(raw).ok()
    } else {
        Some(raw.to_string())
    }
}

impl Client {
    fn new(mut cookies: CookieJar, hl: String, gl: String) -> Self {
        // Pre-answer the EU cookie consent wall ("reject all"), like yt-dlp does.
        if cookies.get("www.youtube.com", "SOCS").is_none() && cookies.get("www.youtube.com", "CONSENT").is_none() {
            cookies.set(".youtube.com", "SOCS", "CAI");
        }
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .gzip(true)
            .brotli(true)
            // youtube.com pages send ~20 KB of response headers (more when
            // signed in); hyper's 16 KB HTTP/2 default rejects them with
            // "stream error detected: unspecific protocol error".
            .http2_max_header_list_size(256 * 1024)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("http client");
        let logged_in = cookies.has_login();
        Self {
            http,
            session: RwLock::new(Session {
                cookies,
                client_version: FALLBACK_CLIENT_VERSION.into(),
                visitor_data: None,
                user_session_id: None,
                delegated_session_id: None,
                session_index: "0".into(),
                logged_in,
                hl,
                gl,
                music_client_version: MUSIC_FALLBACK_CLIENT_VERSION.into(),
                music_bootstrapped: false,
                sts: None,
            }),
        }
    }

    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn session(&self) -> Session {
        self.session.read().unwrap().clone()
    }

    pub fn is_logged_in(&self) -> bool {
        self.session.read().unwrap().logged_in
    }

    pub fn set_cookies(&self, mut jar: CookieJar) {
        if jar.get("www.youtube.com", "SOCS").is_none() {
            jar.set(".youtube.com", "SOCS", "CAI");
        }
        let mut s = self.session.write().unwrap();
        s.logged_in = jar.has_login();
        s.cookies = jar;
        s.user_session_id = None;
        s.delegated_session_id = None;
        s.session_index = "0".into();
    }

    pub fn set_locale(&self, hl: String, gl: String) {
        let mut s = self.session.write().unwrap();
        s.hl = hl;
        s.gl = gl;
    }

    /// Loads youtube.com once to learn the current client version, visitor
    /// id and account session ids, exactly like the website bootstraps itself.
    pub async fn bootstrap(&self) -> Result<()> {
        let (cookie, hl) = {
            let s = self.session.read().unwrap();
            (s.cookies.header_for("www.youtube.com"), s.hl.clone())
        };
        let html = self
            .http
            .get(format!("{ORIGIN}/?hl={hl}"))
            .header("Cookie", cookie)
            .header("Accept-Language", format!("{hl},en;q=0.8"))
            .send()
            .await?
            .text()
            .await?;
        let mut s = self.session.write().unwrap();
        if let Some(v) = ytcfg_value(&html, "INNERTUBE_CLIENT_VERSION") {
            s.client_version = v;
        }
        s.visitor_data = ytcfg_value(&html, "VISITOR_DATA");
        if let Some(sts) = ytcfg_value(&html, "STS").and_then(|v| v.parse().ok()) {
            s.sts = Some(sts);
        }
        if let Some(idx) = ytcfg_value(&html, "SESSION_INDEX").filter(|v| !v.is_empty()) {
            s.session_index = idx;
        }
        let datasync = ytcfg_value(&html, "DATASYNC_ID");
        let (channel_sync, user_sync) = match datasync.as_deref() {
            Some(d) => {
                let (a, b) = d.split_once("||").unwrap_or((d, ""));
                (Some(a.to_string()).filter(|x| !x.is_empty()), Some(b.to_string()).filter(|x| !x.is_empty()))
            }
            None => (None, None),
        };
        s.user_session_id = ytcfg_value(&html, "USER_SESSION_ID").or(user_sync.clone()).or(channel_sync.clone());
        s.delegated_session_id = ytcfg_value(&html, "DELEGATED_SESSION_ID")
            .filter(|v| !v.is_empty())
            .or(if user_sync.is_some() { channel_sync } else { None });
        if let Some(li) = ytcfg_value(&html, "LOGGED_IN") {
            s.logged_in = li == "true" && s.cookies.has_login();
        }
        if s.cookies.has_login() && !s.logged_in {
            crate::log::warn("bootstrap: session cookies are present but YouTube treats this session as signed out (expired or revoked?)");
        } else {
            crate::log::info(&format!(
                "bootstrap: signed in: {}, client {}, delegated account: {}",
                s.logged_in,
                s.client_version,
                s.delegated_session_id.is_some()
            ));
        }
        Ok(())
    }

    /// Picks up YouTube Music's current client version (once per session).
    async fn music_bootstrap(&self) {
        let (cookie, hl) = {
            let mut s = self.session.write().unwrap();
            if s.music_bootstrapped {
                return;
            }
            s.music_bootstrapped = true;
            (s.cookies.header_for("music.youtube.com"), s.hl.clone())
        };
        let res = async {
            self.http
                .get(format!("{MUSIC_ORIGIN}/?hl={hl}"))
                .header("Cookie", cookie)
                .header("Accept-Language", format!("{hl},en;q=0.8"))
                .send()
                .await?
                .text()
                .await
        }
        .await;
        match res {
            Ok(html) => {
                let mut s = self.session.write().unwrap();
                if let Some(v) = ytcfg_value(&html, "INNERTUBE_CLIENT_VERSION") {
                    s.music_client_version = v;
                }
                if s.visitor_data.is_none() {
                    s.visitor_data = ytcfg_value(&html, "VISITOR_DATA");
                }
                crate::log::info(&format!("music bootstrap: client {}", s.music_client_version));
            }
            Err(e) => crate::log::warn(&format!("music bootstrap failed: {}", crate::log::chain(&e))),
        }
    }

    fn sid_authorization(s: &Session, origin: &str) -> Option<String> {
        let ts = cookies::now_secs().to_string();
        let mut parts = Vec::new();
        for (scheme, name) in [
            ("SAPISIDHASH", "SAPISID"),
            ("SAPISID1PHASH", "__Secure-1PAPISID"),
            ("SAPISID3PHASH", "__Secure-3PAPISID"),
        ] {
            let Some(sid) = s.cookies.get("www.youtube.com", name) else { continue };
            let mut input = Vec::new();
            if let Some(u) = &s.user_session_id {
                input.push(u.clone());
            }
            input.extend([ts.clone(), sid.to_string(), origin.to_string()]);
            let hash = hex::encode(Sha1::digest(input.join(" ").as_bytes()));
            let suffix = if s.user_session_id.is_some() { "_u" } else { "" };
            parts.push(format!("{scheme} {ts}_{hash}{suffix}"));
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    async fn post(&self, endpoint: &str, body: Value) -> Result<Value> {
        self.post_as(Flavor::Web, endpoint, body).await
    }

    pub(crate) async fn post_as(&self, flavor: Flavor, endpoint: &str, mut body: Value) -> Result<Value> {
        if flavor == Flavor::Music {
            self.music_bootstrap().await;
        }
        let s = self.session();
        let origin = flavor.origin();
        let (client_name, client_id, client_version) = match flavor {
            Flavor::Web => ("WEB", "1", s.client_version.clone()),
            Flavor::Music => ("WEB_REMIX", "67", s.music_client_version.clone()),
        };
        let mut ctx = json!({
            "client": {
                "clientName": client_name,
                "clientVersion": client_version,
                "hl": s.hl,
                "gl": s.gl,
                "userAgent": USER_AGENT,
                "platform": "DESKTOP",
                "clientFormFactor": "UNKNOWN_FORM_FACTOR",
            },
            "user": { "lockedSafetyMode": false },
            "request": { "useSsl": true },
        });
        if let Some(v) = &s.visitor_data {
            ctx["client"]["visitorData"] = json!(v);
        }
        if let Some(d) = &s.delegated_session_id {
            ctx["user"]["onBehalfOfUser"] = json!(d);
        }
        body["context"] = ctx;

        let mut req = self
            .http
            .post(format!("{origin}/youtubei/v1/{endpoint}?prettyPrint=false"))
            .header("Content-Type", "application/json")
            .header("Origin", origin)
            .header("X-Origin", origin)
            .header("Referer", format!("{origin}/"))
            .header("X-Youtube-Client-Name", client_id)
            .header("X-Youtube-Client-Version", &client_version)
            .header("Cookie", s.cookies.header_for("www.youtube.com"));
        if let Some(v) = &s.visitor_data {
            req = req.header("X-Goog-Visitor-Id", v);
        }
        if s.logged_in {
            req = req.header("X-Goog-AuthUser", &s.session_index).header("X-Youtube-Bootstrap-Logged-In", "true");
            if let Some(auth) = Self::sid_authorization(&s, origin) {
                req = req.header("Authorization", auth);
            }
            if let Some(d) = &s.delegated_session_id {
                req = req.header("X-Goog-PageId", d);
            }
        }
        let resp = match req.json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                crate::log::warn(&format!("InnerTube {endpoint}: {}", crate::log::chain(&e)));
                return Err(e).with_context(|| format!("request to {endpoint} failed"));
            }
        };
        let status = resp.status();
        let v: Value = match resp.json().await {
            Ok(v) => v,
            Err(e) => {
                crate::log::warn(&format!("InnerTube {endpoint}: {status}, unreadable response: {}", crate::log::chain(&e)));
                return Err(e).context("invalid response from YouTube");
            }
        };
        if !status.is_success() {
            let msg = v.pointer("/error/message").and_then(Value::as_str).unwrap_or("unknown error");
            crate::log::warn(&format!("InnerTube {flavor:?} {endpoint}: {status} {msg} (signed in: {})", s.logged_in));
            if status.as_u16() == 401 || status.as_u16() == 403 {
                bail!("YouTube rejected the request ({status}): {msg}. Try signing in again.");
            }
            bail!("YouTube returned {status}: {msg}");
        }
        // Anonymous continuations only work with the visitor id the first
        // page was served for.
        if s.visitor_data.is_none()
            && let Some(vd) = v.pointer("/responseContext/visitorData").and_then(Value::as_str)
        {
            self.session.write().unwrap().visitor_data.get_or_insert_with(|| vd.to_string());
        }
        Ok(v)
    }

    pub async fn browse(&self, browse_id: &str, params: Option<&str>) -> Result<Value> {
        let mut body = json!({ "browseId": browse_id });
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        self.post("browse", body).await
    }

    pub async fn browse_continuation(&self, token: &str) -> Result<Value> {
        self.post("browse", json!({ "continuation": token })).await
    }

    pub async fn next(&self, video_id: &str) -> Result<Value> {
        self.post("next", json!({ "videoId": video_id, "racyCheckOk": true, "contentCheckOk": true })).await
    }

    /// The watch page of `video_id` played from playlist or mix `list`; it
    /// carries the list's panel. Without a video, the list's first one.
    pub async fn next_in_list(&self, video_id: Option<&str>, list: &str, index: Option<usize>) -> Result<Value> {
        let mut body = json!({ "playlistId": list, "racyCheckOk": true, "contentCheckOk": true });
        if let Some(v) = video_id {
            body["videoId"] = json!(v);
        }
        if let Some(i) = index {
            body["playlistIndex"] = json!(i);
        }
        self.post("next", body).await
    }

    pub async fn next_continuation(&self, token: &str) -> Result<Value> {
        self.post("next", json!({ "continuation": token })).await
    }

    pub async fn search(&self, query: &str, params: Option<&str>) -> Result<Value> {
        let mut body = json!({ "query": query });
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        self.post("search", body).await
    }

    pub async fn search_continuation(&self, token: &str) -> Result<Value> {
        self.post("search", json!({ "continuation": token })).await
    }

    pub async fn resolve_url(&self, url: &str) -> Result<Resolved> {
        let v = self.post("navigation/resolve_url", json!({ "url": url })).await?;
        let ep = v.get("endpoint").ok_or_else(|| anyhow!("could not resolve {url}"))?;
        if let Some(id) = ep.pointer("/watchEndpoint/videoId").and_then(Value::as_str) {
            return Ok(Resolved::Video { id: id.into(), playlist: None, start: None });
        }
        if let Some(id) = ep.pointer("/browseEndpoint/browseId").and_then(Value::as_str) {
            if let Some(pl) = id.strip_prefix("VL") {
                return Ok(Resolved::Playlist(pl.into()));
            }
            return Ok(Resolved::Channel(id.into()));
        }
        bail!("could not resolve {url}")
    }

    pub async fn account(&self) -> Result<Option<Account>> {
        if !self.is_logged_in() {
            return Ok(None);
        }
        let v = self.post("account/account_menu", json!({})).await?;
        Ok(parse::parse_account(&v))
    }

    pub async fn like(&self, video_id: &str, action: LikeAction) -> Result<()> {
        let ep = match action {
            LikeAction::Like => "like/like",
            LikeAction::Dislike => "like/dislike",
            LikeAction::Remove => "like/removelike",
        };
        self.post(ep, json!({ "target": { "videoId": video_id } })).await.map(|_| ())
    }

    pub async fn subscribe(&self, channel_id: &str, subscribe: bool) -> Result<()> {
        let (ep, params) = if subscribe {
            ("subscription/subscribe", "EgIIAhgA")
        } else {
            ("subscription/unsubscribe", "CgIIAhgA")
        };
        self.post(ep, json!({ "channelIds": [channel_id], "params": params })).await.map(|_| ())
    }

    pub async fn suggestions(&self, query: &str) -> Result<Vec<String>> {
        let s = self.session();
        let url = format!(
            "https://suggestqueries-clients6.youtube.com/complete/search?client=youtube&ds=yt&hl={}&gl={}&q={}",
            s.hl,
            s.gl.to_lowercase(),
            urlencoding::encode(query)
        );
        let body = self.http.get(url).send().await?.text().await?;
        let start = body.find('(').ok_or_else(|| anyhow!("bad suggestions"))?;
        let end = body.rfind(')').ok_or_else(|| anyhow!("bad suggestions"))?;
        let v: Value = serde_json::from_str(&body[start + 1..end])?;
        Ok(v.get(1)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.get(0)?.as_str().map(str::to_string)).collect())
            .unwrap_or_default())
    }
}
