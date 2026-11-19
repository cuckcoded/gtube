//! Turns InnerTube JSON into our models.
//!
//! YouTube's response layout shifts constantly, so instead of following exact
//! paths we walk the tree and pick out the renderers we understand, wherever
//! they live. Ads and Shorts are dropped during the walk.

use super::models::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

pub static ADS_BLOCKED: AtomicUsize = AtomicUsize::new(0);
pub static SHORTS_HIDDEN: AtomicUsize = AtomicUsize::new(0);

/// Renderers that only ever carry ads or paid promotions.
const AD_KEYS: &[&str] = &[
    "adSlotRenderer",
    "promotedSparklesWebRenderer",
    "promotedSparklesTextSearchRenderer",
    "promotedVideoRenderer",
    "compactPromotedVideoRenderer",
    "compactPromotedItemRenderer",
    "searchPyvRenderer",
    "displayAdRenderer",
    "inFeedAdLayoutRenderer",
    "bannerPromoRenderer",
    "statementBannerRenderer",
    "brandVideoShelfRenderer",
    "brandVideoSingletonRenderer",
    "movieOfferModuleRenderer",
    "mealbarPromoRenderer",
    "adsEngagementPanelContentRenderer",
    "playerLegacyDesktopWatchAdsRenderer",
    "merchandiseShelfRenderer",
    "ticketShelfRenderer",
    "primetimePromoRenderer",
    "adBreakServiceRenderer",
    "playerAds",
    "adPlacements",
    "adSlots",
];

/// Subtrees that never contain feed items (or contain items we don't want).
const SKIP_KEYS: &[&str] = &[
    "frameworkUpdates",
    "responseContext",
    "topbar",
    "microformat",
    "trackingParams",
    "menu",
    "menuButton",
    "loggingDirectives",
    "engagementPanels",
    "playerOverlays",
    "accessibility",
    "commandMetadata",
    "backstagePostThreadRenderer",
    "postRenderer",
    "commentsEntryPointHeaderRenderer",
    "channelVideoPlayerRenderer",
    "unifiedSharePanelRenderer",
    "playlistPanelRenderer",
];

const SHORTS_KEYS: &[&str] = &["reelItemRenderer", "shortsLockupViewModel"];

#[derive(Clone, Copy)]
pub struct Opts {
    pub hide_shorts: bool,
}

// ---------------------------------------------------------------------------
// helpers

pub fn text(v: &Value) -> Option<String> {
    let s = match v {
        Value::String(s) => Some(s.clone()),
        Value::Object(m) => {
            if let Some(s) = m.get("simpleText").and_then(Value::as_str) {
                Some(s.to_string())
            } else if let Some(runs) = m.get("runs").and_then(Value::as_array) {
                Some(runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect())
            } else if let Some(s) = m.get("content").and_then(Value::as_str) {
                Some(s.to_string())
            } else {
                m.get("text").and_then(text)
            }
        }
        _ => None,
    }?;
    let s = s.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn text_at(v: &Value, ptr: &str) -> Option<String> {
    v.pointer(ptr).and_then(text)
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

pub fn find_first<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => {
            if let Some(x) = m.get(key) {
                return Some(x);
            }
            m.values().find_map(|x| find_first(x, key))
        }
        Value::Array(a) => a.iter().find_map(|x| find_first(x, key)),
        _ => None,
    }
}

pub fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                if k == key {
                    out.push(x);
                } else {
                    find_all(x, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| find_all(x, key, out)),
        _ => {}
    }
}

pub fn fix_url(u: &str) -> String {
    if let Some(rest) = u.strip_prefix("//") {
        format!("https://{rest}")
    } else {
        u.to_string()
    }
}

/// Picks the smallest thumbnail at least `target` px wide (or the largest one).
pub fn best_thumb(v: &Value, target: u64) -> Option<String> {
    let arr = if let Some(a) = v.as_array() {
        a
    } else if let Some(a) = v.get("thumbnails").and_then(Value::as_array) {
        a
    } else if let Some(a) = v.get("sources").and_then(Value::as_array) {
        a
    } else if let Some(t) = v.get("thumbnail") {
        return best_thumb(t, target);
    } else if let Some(t) = v.get("image") {
        return best_thumb(t, target);
    } else {
        return None;
    };
    let mut items: Vec<(u64, &str)> = arr
        .iter()
        .filter_map(|t| Some((t.get("width").and_then(Value::as_u64).unwrap_or(0), t.get("url")?.as_str()?)))
        .collect();
    if items.is_empty() {
        return None;
    }
    items.sort_by_key(|(w, _)| *w);
    let pick = items.iter().find(|(w, _)| *w >= target).or(items.last())?;
    Some(fix_url(pick.1))
}

fn continuation_token(v: &Value) -> Option<String> {
    let cmd = find_first(v, "continuationCommand")?;
    cmd.get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| find_first(cmd, "token").and_then(Value::as_str).map(str::to_string))
}

fn browse_id(v: &Value) -> Option<String> {
    find_first(v, "browseEndpoint")
        .and_then(|b| b.get("browseId"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn channel_from_runs(v: Option<&Value>) -> (Option<String>, Option<String>) {
    let Some(v) = v else { return (None, None) };
    let name = v.pointer("/runs/0/text").and_then(Value::as_str).map(str::to_string).or_else(|| text(v));
    let id = v
        .pointer("/runs/0/navigationEndpoint/browseEndpoint/browseId")
        .and_then(Value::as_str)
        .filter(|s| s.starts_with("UC"))
        .map(str::to_string);
    (name, id)
}

fn is_shorts_endpoint(v: &Value) -> bool {
    if v.get("reelWatchEndpoint").is_some() {
        return true;
    }
    str_at(v, "/commandMetadata/webCommandMetadata/url").is_some_and(|u| u.starts_with("/shorts/"))
}

// ---------------------------------------------------------------------------
// feed walker

struct Walker {
    opts: Opts,
    feed: Feed,
    seen: HashSet<String>,
}

impl Walker {
    fn new(opts: Opts) -> Self {
        Self { opts, feed: Feed::default(), seen: HashSet::new() }
    }

    fn push(&mut self, item: Item) {
        let key = match &item {
            Item::Video(v) => {
                if v.is_short && self.opts.hide_shorts {
                    SHORTS_HIDDEN.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                format!("v:{}", v.id)
            }
            Item::Channel(c) => format!("c:{}", c.id),
            Item::Playlist(p) => format!("p:{}", p.id),
        };
        if self.seen.insert(key) {
            self.feed.items.push(item);
        }
    }

    fn walk(&mut self, v: &Value) {
        match v {
            Value::Array(a) => a.iter().for_each(|x| self.walk(x)),
            Value::Object(m) => {
                for (k, x) in m {
                    self.visit(k, x);
                }
            }
            _ => {}
        }
    }

    fn visit(&mut self, k: &str, x: &Value) {
        if AD_KEYS.contains(&k) {
            ADS_BLOCKED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        if SKIP_KEYS.contains(&k) {
            return;
        }
        if SHORTS_KEYS.contains(&k) {
            if self.opts.hide_shorts {
                SHORTS_HIDDEN.fetch_add(1, Ordering::Relaxed);
            } else if let Some(v) = parse_short(k, x) {
                self.push(Item::Video(v));
            }
            return;
        }
        match k {
            "reelShelfRenderer" if self.opts.hide_shorts => {
                SHORTS_HIDDEN.fetch_add(1, Ordering::Relaxed);
            }
            "videoRenderer" | "gridVideoRenderer" | "compactVideoRenderer" | "playlistVideoRenderer"
            | "videoWithContextRenderer" => {
                if let Some(v) = parse_video_renderer(x) {
                    self.push(Item::Video(v));
                }
            }
            "lockupViewModel" => {
                if let Some(item) = parse_lockup(x) {
                    self.push(item);
                }
            }
            "channelRenderer" | "gridChannelRenderer" => {
                if let Some(c) = parse_channel_renderer(x) {
                    self.push(Item::Channel(c));
                }
            }
            "playlistRenderer" | "gridPlaylistRenderer" | "compactPlaylistRenderer" => {
                if let Some(p) = parse_playlist_renderer(x) {
                    self.push(Item::Playlist(p));
                }
            }
            "continuationItemRenderer" | "continuationItemViewModel" => {
                if let Some(t) = continuation_token(x) {
                    self.feed.continuation = Some(t);
                }
            }
            "nextContinuationData" => {
                if let Some(t) = x.get("continuation").and_then(Value::as_str) {
                    self.feed.continuation = Some(t.to_string());
                }
            }
            "chipCloudChipRenderer" => {
                if let (Some(title), Some(token)) =
                    (x.get("text").and_then(text), x.get("navigationEndpoint").and_then(continuation_token))
                {
                    let selected = x.get("isSelected").and_then(Value::as_bool).unwrap_or(false);
                    self.feed.chips.push(Chip { title, token, selected });
                }
            }
            "chipViewModel" => {
                if let (Some(title), Some(token)) =
                    (x.get("text").and_then(text), x.get("tapCommand").and_then(continuation_token))
                {
                    let selected = x.get("selected").and_then(Value::as_bool).unwrap_or(false);
                    self.feed.chips.push(Chip { title, token, selected });
                }
            }
            _ => self.walk(x),
        }
    }
}

/// Parses any browse/search/continuation response into a feed.
pub fn parse_feed(root: &Value, opts: Opts) -> Feed {
    let mut w = Walker::new(opts);
    for key in [
        "contents",
        "onResponseReceivedActions",
        "onResponseReceivedEndpoints",
        "onResponseReceivedCommands",
        "continuationContents",
    ] {
        if let Some(v) = root.get(key) {
            w.walk(v);
        }
    }
    w.feed
}

fn parse_feed_from(v: &Value, opts: Opts) -> Feed {
    let mut w = Walker::new(opts);
    w.walk(v);
    w.feed
}

// ---------------------------------------------------------------------------
// item renderers

fn parse_video_renderer(v: &Value) -> Option<VideoItem> {
    let id = v.get("videoId")?.as_str()?.to_string();
    if v.get("isPlayable").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let title = v.get("title").and_then(text).or_else(|| v.get("headline").and_then(text))?;
    let byline = v.get("longBylineText").or_else(|| v.get("ownerText")).or_else(|| v.get("shortBylineText"));
    let (channel_name, mut channel_id) = channel_from_runs(byline);
    let avatar_src = v
        .pointer("/channelThumbnailSupportedRenderers/channelThumbnailWithLinkRenderer")
        .or_else(|| v.get("channelThumbnail"));
    let channel_avatar = avatar_src.and_then(|a| best_thumb(a, 68));
    if channel_id.is_none() {
        channel_id = avatar_src.and_then(browse_id).filter(|s| s.starts_with("UC"));
    }

    let overlays = v.get("thumbnailOverlays").and_then(Value::as_array);
    let mut duration = v.get("lengthText").and_then(text);
    let mut is_live = false;
    let mut is_short = false;
    let mut progress = None;
    for o in overlays.into_iter().flatten() {
        if let Some(ts) = o.get("thumbnailOverlayTimeStatusRenderer") {
            match ts.get("style").and_then(Value::as_str) {
                Some("LIVE") => is_live = true,
                Some("SHORTS") => is_short = true,
                _ => {}
            }
            if duration.is_none() {
                duration = ts.get("text").and_then(text);
            }
        }
        if let Some(p) = o.pointer("/thumbnailOverlayResumePlaybackRenderer/percentDurationWatched") {
            progress = p.as_f64().map(|p| p / 100.0);
        }
    }
    if let Some(badges) = v.get("badges").and_then(Value::as_array) {
        for b in badges {
            if str_at(b, "/metadataBadgeRenderer/style") == Some("BADGE_STYLE_TYPE_LIVE_NOW") {
                is_live = true;
            }
        }
    }
    if v.get("navigationEndpoint").is_some_and(is_shorts_endpoint) {
        is_short = true;
    }
    if v.get("upcomingEventData").is_some() && duration.is_none() {
        duration = Some("Upcoming".into());
    }

    let (views, published) = if let Some(info) = v.get("videoInfo").and_then(text) {
        (Some(info), None)
    } else {
        (
            v.get("shortViewCountText").and_then(text).or_else(|| v.get("viewCountText").and_then(text)),
            v.get("publishedTimeText").and_then(text),
        )
    };

    Some(VideoItem {
        id,
        title,
        channel_name,
        channel_id,
        channel_avatar,
        thumbnail: v.get("thumbnail").and_then(|t| best_thumb(t, 480)),
        duration: if is_live { Some("LIVE".into()) } else { duration },
        views,
        published,
        is_live,
        is_short,
        progress,
        set_video_id: v.get("setVideoId").and_then(Value::as_str).map(str::to_string),
    })
}

struct MetaPart {
    text: String,
    browse_id: Option<String>,
    a11y: Option<String>,
    has_icon: bool,
}

fn lockup_rows(meta: &Value) -> Vec<Vec<MetaPart>> {
    let Some(rows) = meta.pointer("/metadata/contentMetadataViewModel/metadataRows").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .map(|row| {
            row.get("metadataParts")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| {
                            let t = p.get("text")?;
                            Some(MetaPart {
                                text: text(t)?,
                                browse_id: browse_id(t),
                                a11y: p.get("accessibilityLabel").and_then(Value::as_str).map(str::to_string),
                                has_icon: p.get("leadingIcon").is_some(),
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .filter(|r: &Vec<MetaPart>| !r.is_empty())
        .collect()
}

fn parse_lockup(v: &Value) -> Option<Item> {
    let content_type = v.get("contentType").and_then(Value::as_str).unwrap_or("");
    let id = v.get("contentId")?.as_str()?.to_string();
    let meta = v.pointer("/metadata/lockupMetadataViewModel")?;
    let title = meta.get("title").and_then(text)?;
    let rows = lockup_rows(meta);
    let image = v
        .pointer("/contentImage/thumbnailViewModel/image")
        .or_else(|| v.pointer("/contentImage/collectionThumbnailViewModel/primaryThumbnail/thumbnailViewModel/image"));
    let thumbnail = image.and_then(|i| best_thumb(i, 480));

    let mut badges = Vec::new();
    if let Some(ci) = v.get("contentImage") {
        find_all(ci, "thumbnailBadgeViewModel", &mut badges);
    }
    let badge_text = badges.iter().find_map(|b| b.get("text").and_then(text));
    let is_live = badges
        .iter()
        .any(|b| b.get("badgeStyle").and_then(Value::as_str).is_some_and(|s| s.contains("LIVE")));
    let mut progress = None;
    if let Some(ci) = v.get("contentImage")
        && let Some(pb) = find_first(ci, "thumbnailOverlayProgressBarViewModel")
    {
        progress = pb.get("startPercent").and_then(Value::as_f64).map(|p| p / 100.0);
    }

    let on_tap = v.pointer("/rendererContext/commandContext/onTap/innertubeCommand");
    let avatar_vm = meta.pointer("/image/decoratedAvatarViewModel");
    let avatar = avatar_vm.and_then(|a| a.pointer("/avatar/avatarViewModel/image")).and_then(|i| best_thumb(i, 68));
    let avatar_channel = avatar_vm.and_then(browse_id).filter(|s| s.starts_with("UC"));

    match content_type {
        "LOCKUP_CONTENT_TYPE_VIDEO" | "LOCKUP_CONTENT_TYPE_EPISODE" | "" => {
            let is_short = on_tap.is_some_and(is_shorts_endpoint);
            // With two or more rows the first one is the channel; a single row
            // (e.g. on a channel's own Videos tab) only holds views and age.
            let (channel_row, stat_rows) = if rows.len() >= 2 { (rows.first(), &rows[1..]) } else { (None, &rows[..]) };
            let channel_name = channel_row.and_then(|r| r.first()).map(|p| p.text.clone());
            let channel_id = channel_row
                .and_then(|r| r.iter().find_map(|p| p.browse_id.clone()))
                .filter(|s| s.starts_with("UC"))
                .or(avatar_channel);
            let stats: Vec<String> = stat_rows
                .iter()
                .flatten()
                .map(|p| if p.has_icon { p.a11y.clone().unwrap_or_else(|| p.text.clone()) } else { p.text.clone() })
                .collect();
            let (views, published) = match stats.len() {
                0 => (None, None),
                1 => (Some(stats[0].clone()), None),
                _ => (Some(stats[0].clone()), Some(stats[1..].join(" • "))),
            };
            Some(Item::Video(VideoItem {
                id,
                title,
                channel_name,
                channel_id,
                channel_avatar: avatar,
                thumbnail,
                duration: badge_text,
                views,
                published,
                is_live,
                is_short,
                progress,
                // In the owner's playlist, its "Remove" command names the entry.
                set_video_id: find_first(v, "setVideoId").and_then(Value::as_str).map(str::to_string),
            }))
        }
        "LOCKUP_CONTENT_TYPE_PLAYLIST" | "LOCKUP_CONTENT_TYPE_PODCAST" | "LOCKUP_CONTENT_TYPE_ALBUM" => {
            let owner = rows.iter().flatten().find(|p| p.browse_id.as_deref().is_some_and(|b| b.starts_with("UC")));
            let video_id = on_tap
                .and_then(|t| find_first(t, "watchEndpoint"))
                .and_then(|w| w.get("videoId"))
                .and_then(Value::as_str)
                .map(str::to_string);
            Some(Item::Playlist(PlaylistItem {
                id,
                title,
                thumbnail,
                video_count: badge_text,
                channel_name: owner.map(|p| p.text.clone()),
                channel_id: owner.and_then(|p| p.browse_id.clone()),
                video_id,
            }))
        }
        _ => None,
    }
}

fn parse_short(kind: &str, v: &Value) -> Option<VideoItem> {
    if kind == "shortsLockupViewModel" {
        let id = find_first(v.get("onTap")?, "reelWatchEndpoint")?.get("videoId")?.as_str()?.to_string();
        return Some(VideoItem {
            id,
            title: text_at(v, "/overlayMetadata/primaryText")?,
            views: text_at(v, "/overlayMetadata/secondaryText"),
            thumbnail: v.get("thumbnail").and_then(|t| best_thumb(t, 480)),
            duration: Some("Short".into()),
            is_short: true,
            ..Default::default()
        });
    }
    Some(VideoItem {
        id: v.get("videoId")?.as_str()?.to_string(),
        title: v.get("headline").and_then(text)?,
        views: v.get("viewCountText").and_then(text),
        thumbnail: v.get("thumbnail").and_then(|t| best_thumb(t, 480)),
        duration: Some("Short".into()),
        is_short: true,
        ..Default::default()
    })
}

fn parse_channel_renderer(v: &Value) -> Option<ChannelItem> {
    let id = v.get("channelId")?.as_str()?.to_string();
    let title = v.get("title").and_then(text)?;
    let sub_text = v.get("subscriberCountText").and_then(text);
    let video_text = v.get("videoCountText").and_then(text);
    // Search results put the @handle in subscriberCountText and the
    // subscriber count in videoCountText.
    let (handle, subscribers) = match sub_text {
        Some(s) if s.starts_with('@') => (Some(s), video_text),
        other => (None, other),
    };
    Some(ChannelItem {
        id,
        title,
        avatar: v.get("thumbnail").and_then(|t| best_thumb(t, 176)),
        handle,
        subscribers,
        description: v.get("descriptionSnippet").and_then(text),
    })
}

fn parse_playlist_renderer(v: &Value) -> Option<PlaylistItem> {
    let id = v.get("playlistId")?.as_str()?.to_string();
    let video_id = v
        .get("navigationEndpoint")
        .and_then(|e| find_first(e, "watchEndpoint"))
        .and_then(|w| w.get("videoId"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let thumb = v
        .pointer("/thumbnails/0")
        .or_else(|| v.get("thumbnail"))
        .or_else(|| v.get("thumbnailRenderer").and_then(|t| find_first(t, "thumbnail")));
    let (channel_name, channel_id) =
        channel_from_runs(v.get("shortBylineText").or_else(|| v.get("longBylineText")));
    let video_count = v
        .get("videoCountText")
        .and_then(text)
        .or_else(|| v.get("videoCountShortText").and_then(text))
        .or_else(|| v.get("videoCount").and_then(Value::as_str).map(|c| format!("{c} videos")));
    Some(PlaylistItem {
        id,
        title: v.get("title").and_then(text)?,
        thumbnail: thumb.and_then(|t| best_thumb(t, 480)),
        video_count,
        channel_name,
        channel_id,
        video_id,
    })
}

// ---------------------------------------------------------------------------
// watch page

/// Converts a UTF-16 offset (as used by YouTube's attributed strings) into a
/// byte offset into `s`.
fn utf16_to_byte(s: &str, idx: usize) -> Option<usize> {
    let mut u = 0;
    for (b, c) in s.char_indices() {
        if u == idx {
            return Some(b);
        }
        u += c.len_utf16();
        if u > idx {
            return None;
        }
    }
    (u == idx).then_some(s.len())
}

/// Where an InnerTube command leads, as a link the UI understands.
fn command_target(cmd: &Value, video_id: &str) -> Option<String> {
    if let Some(w) = cmd.get("watchEndpoint") {
        let vid = w.get("videoId").and_then(Value::as_str)?;
        let t = w.get("startTimeSeconds").and_then(Value::as_u64);
        if vid == video_id {
            return Some(format!("seek:{}", t.unwrap_or(0)));
        }
        return Some(match t {
            Some(t) if t > 0 => format!("https://www.youtube.com/watch?v={vid}&t={t}s"),
            _ => format!("https://www.youtube.com/watch?v={vid}"),
        });
    }
    let url = cmd
        .pointer("/urlEndpoint/url")
        .or_else(|| cmd.pointer("/commandMetadata/webCommandMetadata/url"))
        .and_then(Value::as_str)?;
    let url = if url.starts_with('/') { format!("https://www.youtube.com{url}") } else { url.to_string() };
    let parsed = url::Url::parse(&url).ok()?;
    if parsed.path() == "/redirect"
        && let Some((_, q)) = parsed.query_pairs().find(|(k, _)| k == "q")
    {
        return Some(q.into_owned());
    }
    if let Some(tag) = parsed.path().strip_prefix("/hashtag/") {
        return Some(format!("search:{}", urlencoding::encode(&format!("#{tag}"))));
    }
    Some(url)
}

fn description_links(ad: &Value, content: &str, video_id: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    for run in ad.get("commandRuns").and_then(Value::as_array).into_iter().flatten() {
        let (Some(start), Some(len)) = (run.get("startIndex").and_then(Value::as_u64), run.get("length").and_then(Value::as_u64)) else {
            continue;
        };
        let Some(cmd) = run.pointer("/onTap/innertubeCommand") else { continue };
        let (Some(b0), Some(b1)) = (utf16_to_byte(content, start as usize), utf16_to_byte(content, (start + len) as usize)) else {
            continue;
        };
        if let Some(target) = command_target(cmd, video_id) {
            out.push((b0, b1 - b0, target));
        }
    }
    out.sort_by_key(|l| l.0);
    // Drop overlapping runs.
    let mut end = 0;
    out.retain(|l| {
        let ok = l.0 >= end;
        if ok {
            end = l.0 + l.1;
        }
        ok
    });
    out
}

/// The playlist panel of a `next` response for a playlist or mix.
pub fn parse_playlist_panel(root: &Value) -> Option<PlaylistPanel> {
    let pl = root.pointer("/contents/twoColumnWatchNextResults/playlist/playlist")?;
    let items: Vec<VideoItem> = pl
        .get("contents")?
        .as_array()?
        .iter()
        .filter_map(|it| it.get("playlistPanelVideoRenderer"))
        .filter_map(|r| {
            let (channel_name, channel_id) = channel_from_runs(r.get("shortBylineText").or_else(|| r.get("longBylineText")));
            Some(VideoItem {
                id: r.get("videoId")?.as_str()?.to_string(),
                title: r.get("title").and_then(text).unwrap_or_default(),
                channel_name,
                channel_id,
                thumbnail: r.get("thumbnail").and_then(|t| best_thumb(t, 336)),
                duration: r.get("lengthText").and_then(text),
                ..Default::default()
            })
        })
        .collect();
    let current = str_at(root, "/currentVideoEndpoint/watchEndpoint/videoId")
        .and_then(|id| items.iter().position(|v| v.id == id))
        .or_else(|| pl.get("currentIndex").and_then(Value::as_u64).map(|i| i as usize))
        .unwrap_or(0)
        .min(items.len().saturating_sub(1));
    Some(PlaylistPanel {
        id: pl.get("playlistId")?.as_str()?.to_string(),
        title: pl.get("title").and_then(text).or_else(|| pl.get("titleText").and_then(text)).unwrap_or_default(),
        owner: pl.get("ownerName").and_then(text).or_else(|| pl.get("shortBylineText").and_then(text)),
        items,
        current,
        infinite: pl.get("isInfinite").and_then(Value::as_bool).unwrap_or(false),
        total: pl.get("totalVideos").and_then(Value::as_u64).map(|n| n as usize),
    })
}

pub fn parse_next(root: &Value, opts: Opts) -> VideoDetails {
    let mut d = VideoDetails::default();
    d.id = str_at(root, "/currentVideoEndpoint/watchEndpoint/videoId").unwrap_or_default().to_string();
    let results = root.pointer("/contents/twoColumnWatchNextResults/results/results/contents");
    let primary = results.and_then(|r| find_first(r, "videoPrimaryInfoRenderer"));
    let secondary = results.and_then(|r| find_first(r, "videoSecondaryInfoRenderer"));

    if let Some(p) = primary {
        d.title = p.get("title").and_then(text).unwrap_or_default();
        let vc = p.pointer("/viewCount/videoViewCountRenderer");
        d.views = vc.and_then(|v| v.get("viewCount")).and_then(text);
        d.is_live = vc.and_then(|v| v.get("isLive")).and_then(Value::as_bool).unwrap_or(false);
        d.date = p.get("dateText").and_then(text);
        d.relative_date = p.get("relativeDateText").and_then(text);
        if let Some(lb) = find_first(p, "likeButtonViewModel") {
            let mut buttons = Vec::new();
            find_all(lb, "buttonViewModel", &mut buttons);
            d.likes = buttons
                .iter()
                .find(|b| b.get("iconName").and_then(Value::as_str) == Some("LIKE"))
                .or(buttons.first())
                .and_then(|b| b.get("title"))
                .and_then(text);
            d.like_status = find_first(lb, "likeStatusEntity")
                .and_then(|e| e.get("likeStatus"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
    }

    if let Some(s) = secondary {
        if let Some(o) = s.pointer("/owner/videoOwnerRenderer") {
            d.channel.title = o.get("title").and_then(text).unwrap_or_default();
            d.channel.id = browse_id(o.get("title").unwrap_or(o)).or_else(|| browse_id(o)).unwrap_or_default();
            d.channel.avatar = o.get("thumbnail").and_then(|t| best_thumb(t, 88));
            d.channel.subscribers = o.get("subscriberCountText").and_then(text);
        }
        if let Some(ad) = s.get("attributedDescription")
            && let Some(content) = ad.get("content").and_then(Value::as_str)
        {
            d.description = content.to_string();
            d.description_links = description_links(ad, content, &d.id);
        } else {
            d.description = s.get("description").and_then(text).unwrap_or_default();
        }
        d.subscribed = find_first(s, "subscribeButtonRenderer")
            .and_then(|b| b.get("subscribed"))
            .and_then(Value::as_bool);
    }
    if let Some(fu) = root.get("frameworkUpdates")
        && let Some(sub) = find_first(fu, "subscriptionStateEntity")
        && let Some(b) = sub.get("subscribed").and_then(Value::as_bool)
    {
        d.subscribed = Some(b);
    }

    if let Some(r) = results {
        let mut sections = Vec::new();
        find_all(r, "itemSectionRenderer", &mut sections);
        d.comments_token = sections
            .iter()
            .find(|s| s.get("sectionIdentifier").and_then(Value::as_str) == Some("comment-item-section"))
            .and_then(|s| continuation_token(s));
    }

    if let Some(panels) = root.get("engagementPanels").and_then(Value::as_array) {
        for p in panels {
            let Some(panel) = p.get("engagementPanelSectionListRenderer") else { continue };
            let ident = panel.get("panelIdentifier").and_then(Value::as_str).unwrap_or("");
            if ident != "engagement-panel-comments-section" {
                continue;
            }
            d.comments_count = panel
                .pointer("/header/engagementPanelTitleHeaderRenderer/contextualInfo")
                .and_then(text);
            if let Some(menu) = find_first(panel, "sortFilterSubMenuRenderer")
                && let Some(items) = menu.get("subMenuItems").and_then(Value::as_array)
            {
                for it in items {
                    if let (Some(t), Some(tok)) =
                        (it.get("title").and_then(text), it.get("serviceEndpoint").and_then(continuation_token))
                    {
                        d.comment_sorts.push((t, tok));
                    }
                }
            }
        }
    }

    if let Some(sec) = root.pointer("/contents/twoColumnWatchNextResults/secondaryResults") {
        d.related = parse_feed_from(sec, opts);
        d.related.chips.clear();
    }
    d
}

// ---------------------------------------------------------------------------
// comments

pub fn parse_comments(root: &Value) -> CommentsPage {
    let mut entities: HashMap<String, &Value> = HashMap::new();
    if let Some(muts) = root.pointer("/frameworkUpdates/entityBatchUpdate/mutations").and_then(Value::as_array) {
        for m in muts {
            if let (Some(k), Some(p)) = (m.get("entityKey").and_then(Value::as_str), m.get("payload")) {
                entities.insert(k.to_string(), p);
            }
        }
    }

    let mut page = CommentsPage::default();
    let mut items: Vec<&Value> = Vec::new();
    for key in ["onResponseReceivedEndpoints", "onResponseReceivedActions"] {
        if let Some(eps) = root.get(key).and_then(Value::as_array) {
            for ep in eps {
                let list = ep
                    .pointer("/reloadContinuationItemsCommand/continuationItems")
                    .or_else(|| ep.pointer("/appendContinuationItemsAction/continuationItems"));
                if let Some(list) = list.and_then(Value::as_array) {
                    items.extend(list.iter());
                }
            }
        }
    }

    for it in items {
        if let Some(thread) = it.get("commentThreadRenderer") {
            let replies_token = thread.get("replies").and_then(continuation_token);
            let c = if let Some(vm) = thread.pointer("/commentViewModel/commentViewModel") {
                comment_from_view_model(vm, &entities)
            } else {
                thread.pointer("/comment/commentRenderer").and_then(comment_from_renderer)
            };
            if let Some(mut c) = c {
                c.replies_token = replies_token;
                page.comments.push(c);
            }
        } else if let Some(vm) = it.get("commentViewModel") {
            if let Some(c) = comment_from_view_model(vm, &entities) {
                page.comments.push(c);
            }
        } else if let Some(r) = it.get("commentRenderer") {
            if let Some(c) = comment_from_renderer(r) {
                page.comments.push(c);
            }
        } else if it.get("continuationItemRenderer").is_some() {
            page.continuation = continuation_token(it);
        }
    }
    page
}

fn comment_from_view_model(vm: &Value, entities: &HashMap<String, &Value>) -> Option<Comment> {
    let key = vm.get("commentKey")?.as_str()?;
    let payload = entities.get(key)?.get("commentEntityPayload")?;
    let props = payload.get("properties")?;
    let author = payload.get("author");
    let toolbar = payload.get("toolbar");
    let hearted = vm
        .get("toolbarStateKey")
        .and_then(Value::as_str)
        .and_then(|k| entities.get(k))
        .and_then(|e| e.pointer("/engagementToolbarStateEntityPayload/heartState"))
        .and_then(Value::as_str)
        == Some("TOOLBAR_HEART_STATE_HEARTED");
    let non_empty = |s: Option<&str>| s.filter(|s| !s.is_empty() && *s != "0").map(str::to_string);
    Some(Comment {
        author: author
            .and_then(|a| a.get("displayName"))
            .and_then(Value::as_str)
            .unwrap_or("Unknown")
            .to_string(),
        author_channel_id: author.and_then(|a| a.get("channelId")).and_then(Value::as_str).map(str::to_string),
        avatar: author
            .and_then(|a| a.get("avatarThumbnailUrl"))
            .and_then(Value::as_str)
            .map(fix_url)
            .or_else(|| payload.get("avatar").and_then(|a| best_thumb(a, 88))),
        text: props.pointer("/content/content").and_then(Value::as_str).unwrap_or_default().to_string(),
        published: props.get("publishedTime").and_then(Value::as_str).unwrap_or_default().to_string(),
        likes: non_empty(toolbar.and_then(|t| t.get("likeCountNotliked")).and_then(Value::as_str)),
        reply_count: non_empty(toolbar.and_then(|t| t.get("replyCount")).and_then(Value::as_str)),
        replies_token: None,
        is_creator: author.and_then(|a| a.get("isCreator")).and_then(Value::as_bool).unwrap_or(false),
        is_verified: author.and_then(|a| a.get("isVerified")).and_then(Value::as_bool).unwrap_or(false),
        pinned: vm.get("pinnedText").and_then(text),
        hearted,
    })
}

fn comment_from_renderer(r: &Value) -> Option<Comment> {
    r.get("commentId")?;
    Some(Comment {
        author: r.get("authorText").and_then(text).unwrap_or_else(|| "Unknown".into()),
        author_channel_id: r
            .pointer("/authorEndpoint/browseEndpoint/browseId")
            .and_then(Value::as_str)
            .map(str::to_string),
        avatar: r.get("authorThumbnail").and_then(|t| best_thumb(t, 88)),
        text: r.get("contentText").and_then(text).unwrap_or_default(),
        published: r.get("publishedTimeText").and_then(text).unwrap_or_default(),
        likes: r.get("voteCount").and_then(text),
        reply_count: r.get("replyCount").and_then(|c| c.as_u64().map(|n| n.to_string()).or_else(|| text(c))),
        replies_token: None,
        is_creator: r.get("authorIsChannelOwner").and_then(Value::as_bool).unwrap_or(false),
        is_verified: false,
        pinned: r.get("pinnedCommentBadge").map(|_| "Pinned".to_string()),
        hearted: r.pointer("/actionButtons/commentActionButtonsRenderer/creatorHeart").is_some(),
    })
}

// ---------------------------------------------------------------------------
// channel & playlist pages

pub fn parse_channel(root: &Value) -> ChannelPage {
    let mut page = ChannelPage::default();
    if let Some(m) = root.pointer("/metadata/channelMetadataRenderer") {
        page.info.id = m.get("externalId").and_then(Value::as_str).unwrap_or_default().to_string();
        page.info.title = m.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
        page.info.description = m.get("description").and_then(Value::as_str).map(str::to_string);
        page.info.avatar = m.get("avatar").and_then(|a| best_thumb(a, 176));
        page.info.handle = m
            .get("vanityChannelUrl")
            .and_then(Value::as_str)
            .and_then(|u| u.rsplit('/').next())
            .filter(|h| h.starts_with('@'))
            .map(str::to_string);
    }
    if let Some(header) = root.get("header") {
        if let Some(ph) = find_first(header, "pageHeaderViewModel") {
            if page.info.title.is_empty() {
                page.info.title = text_at(ph, "/title/dynamicTextViewModel/text").unwrap_or_default();
            }
            if let Some(img) = ph.pointer("/image/decoratedAvatarViewModel/avatar/avatarViewModel/image") {
                page.info.avatar = best_thumb(img, 176).or(page.info.avatar.take());
            }
            if let Some(b) = find_first(ph, "imageBannerViewModel") {
                page.banner = b.get("image").and_then(|i| best_thumb(i, 2120));
            }
            let mut others = Vec::new();
            for row in lockup_rows(ph) {
                for p in row {
                    if p.text.starts_with('@') {
                        page.info.handle = Some(p.text);
                    } else {
                        others.push(p.text);
                    }
                }
            }
            let mut others = others.into_iter();
            page.info.subscribers = others.next();
            page.video_count = others.next();
        } else if let Some(c4) = header.get("c4TabbedHeaderRenderer") {
            page.info.subscribers = c4.get("subscriberCountText").and_then(text);
            page.video_count = c4.get("videosCountText").and_then(text);
            page.info.handle = c4.get("channelHandleText").and_then(text).or(page.info.handle.take());
            page.banner = c4.get("banner").and_then(|b| best_thumb(b, 2120));
        }
    }
    if let Some(contents) = root.get("contents") {
        let mut tabs = Vec::new();
        find_all(contents, "tabRenderer", &mut tabs);
        for t in tabs {
            if let (Some(title), Some(params)) =
                (t.get("title").and_then(text), str_at(t, "/endpoint/browseEndpoint/params"))
            {
                page.tabs.push((title, params.to_string()));
            }
        }
    }
    let fu = root.get("frameworkUpdates").and_then(|f| find_first(f, "subscriptionStateEntity"));
    page.subscribed = fu
        .and_then(|s| s.get("subscribed"))
        .and_then(Value::as_bool)
        .or_else(|| {
            root.get("header")
                .and_then(|h| find_first(h, "subscribeButtonRenderer"))
                .and_then(|b| b.get("subscribed"))
                .and_then(Value::as_bool)
        });
    page
}

pub fn parse_playlist(root: &Value, id: &str, opts: Opts) -> PlaylistPage {
    let mut page = PlaylistPage { id: id.to_string(), ..Default::default() };
    if let Some(m) = root.pointer("/metadata/playlistMetadataRenderer") {
        page.title = m.get("title").and_then(Value::as_str).unwrap_or_default().to_string();
        page.description = m.get("description").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
    }
    if let Some(header) = root.get("header") {
        if let Some(ph) = find_first(header, "pageHeaderViewModel") {
            if page.title.is_empty() {
                page.title = text_at(ph, "/title/dynamicTextViewModel/text").unwrap_or_default();
            }
            if let Some(stack) = find_first(ph, "avatarStackViewModel") {
                page.owner = stack.get("text").and_then(text);
                page.owner_id = browse_id(stack).filter(|b| b.starts_with("UC"));
            }
            for row in lockup_rows(ph) {
                for p in row {
                    if Some(&p.text) != page.owner.as_ref() && !p.text.starts_with("by ") {
                        page.stats.push(p.text);
                    }
                }
            }
            if let Some(hero) = find_first(ph, "contentPreviewImageViewModel") {
                page.thumbnail = hero.get("image").and_then(|i| best_thumb(i, 480));
            }
        } else if let Some(h) = header.get("playlistHeaderRenderer") {
            if page.title.is_empty() {
                page.title = h.get("title").and_then(text).unwrap_or_default();
            }
            let (o, oid) = channel_from_runs(h.get("ownerText"));
            page.owner = o;
            page.owner_id = oid;
            if let Some(s) = h.get("numVideosText").and_then(text) {
                page.stats.push(s);
            }
        }
    }
    if page.thumbnail.is_none()
        && let Some(sb) = root.get("sidebar")
        && let Some(t) = find_first(sb, "playlistVideoThumbnailRenderer")
    {
        page.thumbnail = best_thumb(t, 480);
    }
    page.feed = parse_feed(root, opts);
    page.feed.chips.clear();
    page.editable = match find_first(root, "playlistVideoListRenderer") {
        Some(l) => l.get("isEditable").and_then(Value::as_bool).unwrap_or(false),
        // Newer pages: only the owner's entries carry a set id.
        None => page.feed.items.iter().any(|i| matches!(i, Item::Video(v) if v.set_video_id.is_some())),
    };
    page.privacy = root
        .get("header")
        .and_then(|h| find_first(h, "privacy"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            page.stats.iter().find_map(|s| match s.as_str() {
                "Public" | "Unlisted" | "Private" => Some(s.to_uppercase()),
                _ => None,
            })
        });
    page
}

/// The user's playlists from `playlist/get_add_to_playlist`.
pub fn parse_save_targets(root: &Value) -> Vec<SaveTarget> {
    let mut opts = Vec::new();
    find_all(root, "playlistAddToOptionRenderer", &mut opts);
    opts.into_iter()
        .filter_map(|o| {
            Some(SaveTarget {
                id: o.get("playlistId")?.as_str()?.to_string(),
                title: o.get("title").and_then(text)?,
                saved: o.get("containsSelectedVideos").and_then(Value::as_str) == Some("ALL"),
            })
        })
        .collect()
}

pub fn parse_account(root: &Value) -> Option<Account> {
    let h = find_first(root, "activeAccountHeaderRenderer")?;
    let mut ids = Vec::new();
    find_all(root, "browseEndpoint", &mut ids);
    Some(Account {
        name: h.get("accountName").and_then(text)?,
        handle: h.get("channelHandle").and_then(text),
        avatar: h.get("accountPhoto").and_then(|p| best_thumb(p, 88)),
        channel_id: ids
            .iter()
            .filter_map(|b| b.get("browseId").and_then(Value::as_str))
            .find(|b| b.starts_with("UC"))
            .map(str::to_string),
    })
}

/// Parses a YouTube URL locally, without any network request. Handles
/// return `Search` so the caller can resolve them through the API.
pub fn parse_youtube_url(input: &str) -> Option<Resolved> {
    let input = input.trim();
    let with_scheme = if input.starts_with("http://") || input.starts_with("https://") {
        input.to_string()
    } else if input.starts_with("www.") || input.starts_with("youtube.com") || input.starts_with("youtu.be")
        || input.starts_with("m.youtube.com") || input.starts_with("music.youtube.com")
    {
        format!("https://{input}")
    } else {
        return None;
    };
    let url = url::Url::parse(&with_scheme).ok()?;
    let host = url.host_str()?.trim_start_matches("www.").trim_start_matches("m.").to_string();
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let start = q("t").and_then(|t| parse_time_param(&t));
    let segs: Vec<&str> = url.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();

    if host == "youtu.be" {
        return segs.first().map(|id| Resolved::Video { id: id.to_string(), playlist: q("list"), start });
    }
    if !host.ends_with("youtube.com") && !host.ends_with("youtube-nocookie.com") {
        return None;
    }
    match segs.as_slice() {
        ["watch"] => q("v").map(|id| Resolved::Video { id, playlist: q("list"), start }),
        ["shorts" | "live" | "embed" | "v", id, ..] => Some(Resolved::Video { id: id.to_string(), playlist: None, start }),
        ["playlist"] => q("list").map(Resolved::Playlist),
        ["channel", id, ..] => Some(Resolved::Channel(id.to_string())),
        _ => Some(Resolved::Search(with_scheme)),
    }
}

fn parse_time_param(t: &str) -> Option<f64> {
    if let Ok(n) = t.trim_end_matches('s').parse::<f64>() {
        return Some(n);
    }
    let re = regex::Regex::new(r"^(?:(\d+)h)?(?:(\d+)m)?(?:(\d+)s)?$").ok()?;
    let c = re.captures(t)?;
    let g = |i| c.get(i).and_then(|m| m.as_str().parse::<f64>().ok()).unwrap_or(0.0);
    Some(g(1) * 3600.0 + g(2) * 60.0 + g(3))
}

#[cfg(test)]
mod tests {
    //! Run with `GTUBE_FIXTURES=/path/to/json/dir cargo test -- --nocapture`
    //! against saved InnerTube responses (next.json, search.json, …).
    use super::*;

    fn fixture(name: &str) -> Option<Value> {
        let dir = std::env::var("GTUBE_FIXTURES").ok()?;
        let text = std::fs::read_to_string(std::path::Path::new(&dir).join(name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn show(feed: &Feed, n: usize) {
        for it in feed.items.iter().take(n) {
            match it {
                Item::Video(v) => println!(
                    "  V {} | {} | ch={:?}/{:?} | {:?} | {:?} | {:?} | short={} live={} av={}",
                    v.id, v.title, v.channel_name, v.channel_id, v.duration, v.views, v.published, v.is_short, v.is_live, v.channel_avatar.is_some()
                ),
                Item::Channel(c) => println!("  C {} | {} | {:?} {:?}", c.id, c.title, c.handle, c.subscribers),
                Item::Playlist(p) => println!("  P {} | {} | {:?} | {:?}", p.id, p.title, p.video_count, p.channel_name),
            }
        }
        println!("  items={} continuation={} chips={:?}", feed.items.len(), feed.continuation.is_some(), feed.chips.iter().map(|c| &c.title).collect::<Vec<_>>());
    }

    #[test]
    fn playlist_panel() {
        let v: Value = serde_json::from_str(
            r#"{"currentVideoEndpoint":{"watchEndpoint":{"videoId":"b"}},
               "contents":{"twoColumnWatchNextResults":{"playlist":{"playlist":{
                 "playlistId":"RDa","title":"Mix - A","isInfinite":true,"ownerName":{"simpleText":"YouTube"},
                 "contents":[
                   {"playlistPanelVideoRenderer":{"videoId":"a","title":{"simpleText":"A"},"lengthText":{"simpleText":"3:00"},
                     "shortBylineText":{"runs":[{"text":"Artist","navigationEndpoint":{"browseEndpoint":{"browseId":"UCx"}}}]}}},
                   {"playlistPanelVideoRenderer":{"videoId":"b","title":{"simpleText":"B"}}},
                   {"automixPreviewVideoRenderer":{}}]}}}}}"#,
        )
        .unwrap();
        let p = parse_playlist_panel(&v).unwrap();
        assert_eq!((p.id.as_str(), p.title.as_str(), p.infinite, p.current, p.items.len()), ("RDa", "Mix - A", true, 1, 2));
        assert_eq!(p.owner.as_deref(), Some("YouTube"));
        assert_eq!(p.items[0].channel_id.as_deref(), Some("UCx"));
        assert_eq!(p.items[0].duration.as_deref(), Some("3:00"));
    }

    #[test]
    fn fixtures() {
        let o = Opts { hide_shorts: true };
        for name in ["next_mix.json", "next_pl.json"] {
            if let Some(v) = fixture(name) {
                let p = parse_playlist_panel(&v).unwrap();
                println!("{name}: {} | {} | {:?} | inf={} cur={} total={:?} items={}", p.id, p.title, p.owner, p.infinite, p.current, p.total, p.items.len());
                for it in p.items.iter().take(3) {
                    println!("  {} | {} | {:?} {:?} | {:?}", it.id, it.title, it.channel_name, it.channel_id, it.duration);
                }
            }
        }
        if let Some(v) = fixture("search_mix.json") {
            println!("search_mix:");
            show(&parse_feed(&v, o), 8);
        }
        if let Some(v) = fixture("search.json") {
            println!("search:");
            show(&parse_feed(&v, o), 6);
        }
        if let Some(v) = fixture("next.json") {
            let d = parse_next(&v, o);
            println!("next: id={} title={} views={:?} date={:?} likes={:?} ch={}/{} subs={:?} sub={:?}", d.id, d.title, d.views, d.date, d.likes, d.channel.title, d.channel.id, d.channel.subscribers, d.subscribed);
            println!("  desc={:?}", d.description.chars().take(80).collect::<String>());
            println!("  comments token={} count={:?} sorts={:?}", d.comments_token.is_some(), d.comments_count, d.comment_sorts.iter().map(|s| &s.0).collect::<Vec<_>>());
            show(&d.related, 4);
        }
        if let Some(v) = fixture("comments.json") {
            let c = parse_comments(&v);
            println!("comments: n={} cont={}", c.comments.len(), c.continuation.is_some());
            for x in c.comments.iter().take(3) {
                println!("  {} | {} | likes={:?} replies={:?} tok={} hearted={} pinned={:?}", x.author, x.text.chars().take(50).collect::<String>(), x.likes, x.reply_count, x.replies_token.is_some(), x.hearted, x.pinned);
            }
        }
        if let Some(v) = fixture("channel.json") {
            let c = parse_channel(&v);
            println!("channel: {:?} {:?} {:?} {:?} banner={} tabs={:?}", c.info.title, c.info.handle, c.info.subscribers, c.video_count, c.banner.is_some(), c.tabs.iter().map(|t| &t.0).collect::<Vec<_>>());
        }
        for f in ["ch_videos.json", "ch_playlists.json", "ch_live.json"] {
            if let Some(v) = fixture(f) {
                println!("{f}:");
                show(&parse_feed(&v, o), 3);
            }
        }
        if let Some(v) = fixture("playlist.json") {
            let p = parse_playlist(&v, "PL", o);
            println!("playlist: {} owner={:?}/{:?} stats={:?} thumb={}", p.title, p.owner, p.owner_id, p.stats, p.thumbnail.is_some());
            show(&p.feed, 3);
        }
    }

    #[test]
    fn urls() {
        assert!(matches!(parse_youtube_url("https://youtu.be/abc?t=30"), Some(Resolved::Video { start: Some(30.0), .. })));
        assert!(matches!(parse_youtube_url("https://www.youtube.com/shorts/xyz"), Some(Resolved::Video { .. })));
        assert!(matches!(parse_youtube_url("youtube.com/watch?v=dQw4w9WgXcQ&t=1m5s"), Some(Resolved::Video { start: Some(65.0), .. })));
        assert!(matches!(parse_youtube_url("https://www.youtube.com/playlist?list=PL1"), Some(Resolved::Playlist(_))));
        assert!(matches!(parse_youtube_url("https://www.youtube.com/@handle"), Some(Resolved::Search(_))));
        assert!(parse_youtube_url("linux kernel").is_none());
    }

    #[test]
    fn owned_playlists_are_editable() {
        let entry = |id: &str, owned: bool| {
            let mut l = serde_json::json!({"lockupViewModel": {
                "contentType": "LOCKUP_CONTENT_TYPE_VIDEO", "contentId": id,
                "metadata": {"lockupMetadataViewModel": {"title": {"content": id}}}}});
            if owned {
                l["lockupViewModel"]["menu"] = serde_json::json!({"playlistEditEndpoint": {"actions": [
                    {"action": "ACTION_REMOVE_VIDEO", "setVideoId": format!("s{id}")}]}});
            }
            l
        };
        let page = |owned| serde_json::json!({"contents": [entry("a", owned), entry("b", owned)]});
        let opts = Opts { hide_shorts: false };
        let p = parse_playlist(&page(true), "PL1", opts);
        assert!(p.editable);
        assert!(matches!(&p.feed.items[1], Item::Video(v) if v.set_video_id.as_deref() == Some("sb")));
        assert!(!parse_playlist(&page(false), "PL1", opts).editable);
        // The older list says so itself (and has set ids either way).
        let old = serde_json::json!({"contents": {"playlistVideoListRenderer": {"isEditable": false, "contents": [
            {"playlistVideoRenderer": {"videoId": "a", "title": {"simpleText": "A"}, "setVideoId": "sa"}}]}}});
        assert!(!parse_playlist(&old, "PL1", opts).editable);
    }

    #[test]
    fn save_targets() {
        let v = serde_json::json!({"contents": [{"addToPlaylistRenderer": {"playlists": [
            {"playlistAddToOptionRenderer": {"playlistId": "WL", "title": {"simpleText": "Watch later"}, "containsSelectedVideos": "NONE"}},
            {"playlistAddToOptionRenderer": {"playlistId": "PL1", "title": {"simpleText": "Mine"}, "containsSelectedVideos": "ALL"}}]}}]});
        let t = parse_save_targets(&v);
        assert_eq!(t.iter().map(|t| (t.id.as_str(), t.saved)).collect::<Vec<_>>(), [("WL", false), ("PL1", true)]);
    }
}
