//! YouTube Music: requests made as music.youtube.com's web client
//! (`WEB_REMIX`) and parsers that turn its renderers into a small set of
//! models — pages made of shelves of tracks and cards, the play queue and
//! lyrics.
//!
//! Every YouTube Music page (home, explore, search, artists, albums,
//! playlists, library, moods, charts, "related") is built from the same
//! handful of renderers, so one generic page parser covers all of them.

use super::models::VideoItem;
use super::{Client, Flavor};
use anyhow::Result;
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Models

/// A piece of text that may link to a page (artist, album, profile).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub text: String,
    pub id: Option<String>,
}

/// What activating something does.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Browse { id: String, params: Option<String> },
    Watch { video_id: Option<String>, playlist_id: Option<String>, params: Option<String>, index: Option<u64> },
    Search { query: String, params: Option<String> },
}

impl Target {
    pub fn browse(id: &str) -> Self {
        Target::Browse { id: id.to_string(), params: None }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum VideoKind {
    /// An "Art Track": the song, with the album cover as its picture.
    #[default]
    Song,
    /// A music video (official or uploaded).
    Video,
    Episode,
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artists: Vec<Link>,
    pub album: Option<Link>,
    pub duration: Option<String>,
    pub thumbnail: Option<String>,
    pub kind: VideoKind,
    pub explicit: bool,
    /// "1.2B plays", "36M views", a year…
    pub info: Option<String>,
    /// Track number on album pages.
    pub index: Option<String>,
    /// The list this row plays from (album, playlist, radio).
    pub playlist_id: Option<String>,
    pub params: Option<String>,
    pub list_index: Option<u64>,
    /// The other version of this track (song ↔ music video), when YouTube
    /// Music offers one.
    pub counterpart: Option<Box<Track>>,
}

impl Track {
    pub fn artist_names(&self) -> String {
        self.artists.iter().map(|a| a.text.as_str()).collect::<Vec<_>>().join(", ")
    }

    pub fn first_artist_id(&self) -> Option<&str> {
        self.artists.iter().find_map(|a| a.id.as_deref())
    }

    pub fn url(&self) -> String {
        format!("https://music.youtube.com/watch?v={}", self.video_id)
    }

    pub fn thumbnail_url(&self, px: u32) -> String {
        match &self.thumbnail {
            Some(t) => image_url(t, px),
            None => format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.video_id),
        }
    }

    /// The form the shared player and the downloader work with.
    pub fn video_item(&self) -> VideoItem {
        VideoItem {
            id: self.video_id.clone(),
            title: self.title.clone(),
            channel_name: Some(self.artist_names()).filter(|s| !s.is_empty()),
            channel_id: self.first_artist_id().map(str::to_string),
            thumbnail: self.thumbnail.as_deref().map(|t| image_url(t, 544)),
            duration: self.duration.clone(),
            ..Default::default()
        }
    }

    /// The watch target that plays this track in its own list (album,
    /// playlist) or, failing that, as a radio.
    pub fn play_target(&self) -> Target {
        if self.playlist_id.is_none() {
            return self.radio_target();
        }
        Target::Watch {
            video_id: Some(self.video_id.clone()),
            playlist_id: self.playlist_id.clone(),
            params: self.params.clone(),
            index: self.list_index,
        }
    }

    pub fn radio_target(&self) -> Target {
        Target::Watch {
            video_id: Some(self.video_id.clone()),
            playlist_id: Some(format!("RDAMVM{}", self.video_id)),
            params: Some("wAEB".into()),
            index: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shape {
    #[default]
    Square,
    /// Artists and profiles.
    Round,
    /// 16:9 music videos.
    Wide,
}

/// An album, single, playlist, artist, video or mood tile.
#[derive(Clone, Debug, Default)]
pub struct Card {
    pub title: String,
    pub subtitle: Vec<Link>,
    pub thumbnail: Option<String>,
    pub target: Option<Target>,
    /// Plays the item directly (the play button over the thumbnail).
    pub play: Option<Target>,
    pub shape: Shape,
    pub explicit: bool,
}

impl Card {
    pub fn subtitle_text(&self) -> String {
        self.subtitle.iter().map(|l| l.text.as_str()).collect()
    }
}

/// A colored "mood or genre" button.
#[derive(Clone, Debug)]
pub struct Mood {
    pub title: String,
    pub target: Target,
    /// 0xAARRGGBB.
    pub color: Option<u32>,
}

#[derive(Clone, Debug)]
pub enum Entry {
    Track(Track),
    Card(Card),
    Mood(Mood),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShelfStyle {
    /// Rows (songs, search results, album tracks).
    #[default]
    List,
    /// A horizontally scrolling row of tiles (or columns of songs).
    Carousel,
    /// Wrapping tiles.
    Grid,
    /// Text ("About", lyrics).
    Text,
}

#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub title: Option<String>,
    pub strapline: Option<String>,
    pub title_target: Option<Target>,
    /// "More", "See all", "Play all"…
    pub more: Option<(String, Target)>,
    pub style: ShelfStyle,
    pub entries: Vec<Entry>,
    pub text: Option<String>,
    pub footer: Option<String>,
    pub continuation: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HeaderKind {
    #[default]
    Simple,
    Artist,
    /// Album or playlist.
    Collection,
}

#[derive(Clone, Debug, Default)]
pub struct Header {
    pub kind: HeaderKind,
    pub title: String,
    /// "Album • 2013"
    pub subtitle: Option<String>,
    /// "13 songs • 1 hour, 14 minutes"
    pub second_subtitle: Option<String>,
    /// Album artist / playlist owner.
    pub owner: Vec<Link>,
    pub description: Option<String>,
    pub thumbnail: Option<String>,
    /// (label, target): "Play" for collections, "Shuffle" for artists.
    pub play: Option<(String, Target)>,
    pub shuffle: Option<Target>,
    pub radio: Option<(String, Target)>,
    pub channel_id: Option<String>,
    pub subscribed: Option<bool>,
    pub subscribers: Option<String>,
    pub monthly: Option<String>,
    /// Album/playlist "save to library" state and the id to save.
    pub saved: Option<bool>,
    pub playlist_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Chip {
    pub title: String,
    pub target: Target,
    pub selected: bool,
}

/// The large "top result" card of a search.
#[derive(Clone, Debug, Default)]
pub struct TopResult {
    pub card: Card,
    pub tracks: Vec<Track>,
    pub shuffle: Option<Target>,
    pub radio: Option<Target>,
}

#[derive(Clone, Debug, Default)]
pub struct Page {
    pub title: Option<String>,
    pub header: Option<Header>,
    pub chips: Vec<Chip>,
    pub top: Option<TopResult>,
    pub shelves: Vec<Shelf>,
    /// Loads more shelves (home).
    pub continuation: Option<String>,
}

/// The result of a continuation request: more shelves (section lists) or
/// more entries for the shelf that asked.
#[derive(Clone, Debug, Default)]
pub struct More {
    pub shelves: Vec<Shelf>,
    pub entries: Vec<Entry>,
    pub continuation: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Queue {
    /// "Playing from": the album, playlist or mix name.
    pub title: Option<String>,
    pub tracks: Vec<Track>,
    pub selected: Option<usize>,
    pub continuation: Option<String>,
    pub playlist_id: Option<String>,
    /// Where to continue once the list runs out ("autoplay").
    pub automix: Option<Target>,
    pub lyrics_id: Option<String>,
    pub related_id: Option<String>,
    pub like_status: Option<String>,
}

pub use super::lyrics::Lyrics;

#[derive(Clone, Debug)]
pub enum Suggestion {
    Query(String),
    Entry(Entry),
}

// ---------------------------------------------------------------------------
// Requests

impl Client {
    pub async fn music_browse(&self, id: &str, params: Option<&str>) -> Result<Value> {
        let mut body = json!({ "browseId": id });
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        self.post_as(Flavor::Music, "browse", body).await
    }

    /// `endpoint` is "browse" or "search", whichever served the first page.
    pub async fn music_continuation(&self, endpoint: &str, token: &str) -> Result<Value> {
        self.post_as(Flavor::Music, endpoint, json!({ "continuation": token })).await
    }

    pub async fn music_search(&self, query: &str, params: Option<&str>) -> Result<Value> {
        let mut body = json!({ "query": query });
        if let Some(p) = params {
            body["params"] = json!(p);
        }
        self.post_as(Flavor::Music, "search", body).await
    }

    /// The play queue ("Up next") for a video and/or playlist, plus the
    /// lyrics and related tabs of the selected track.
    pub async fn music_next(&self, target: &Target) -> Result<Value> {
        let mut body = json!({
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
            "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
        });
        if let Target::Watch { video_id, playlist_id, params, index } = target {
            if let Some(v) = video_id {
                body["videoId"] = json!(v);
            }
            if let Some(p) = playlist_id {
                body["playlistId"] = json!(p);
            }
            if let Some(p) = params {
                body["params"] = json!(p);
            }
            if let Some(i) = index {
                body["index"] = json!(i);
            }
        }
        self.post_as(Flavor::Music, "next", body).await
    }

    pub async fn music_next_continuation(&self, playlist_id: Option<&str>, token: &str) -> Result<Value> {
        let mut body = json!({
            "continuation": token,
            "isAudioOnly": true,
            "enablePersistentPlaylistPanel": true,
        });
        if let Some(p) = playlist_id {
            body["playlistId"] = json!(p);
        }
        self.post_as(Flavor::Music, "next", body).await
    }

    pub async fn music_suggestions(&self, input: &str) -> Result<Vec<Suggestion>> {
        let v = self.post_as(Flavor::Music, "music/get_search_suggestions", json!({ "input": input })).await?;
        Ok(parse_suggestions(&v))
    }

    /// Adds an album or playlist to the library, or removes it.
    pub async fn music_save_playlist(&self, playlist_id: &str, save: bool) -> Result<()> {
        let ep = if save { "like/like" } else { "like/removelike" };
        self.post_as(Flavor::Music, ep, json!({ "target": { "playlistId": playlist_id } })).await.map(|_| ())
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers

fn text(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(s) = v.as_str() {
        return Some(s.to_string());
    }
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    if let Some(s) = v.get("content").and_then(Value::as_str) {
        return Some(s.to_string());
    }
    let runs = v.get("runs")?.as_array()?;
    let s: String = runs.iter().filter_map(|r| r.get("text").and_then(Value::as_str)).collect();
    Some(s)
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn browse_id(ep: Option<&Value>) -> Option<String> {
    ep?.pointer("/browseEndpoint/browseId").and_then(Value::as_str).map(str::to_string)
}

fn runs(v: Option<&Value>) -> Vec<Link> {
    let Some(arr) = v.and_then(|v| v.get("runs")).and_then(Value::as_array) else {
        return text(v).map(|t| vec![Link { text: t, id: None }]).unwrap_or_default();
    };
    arr.iter()
        .map(|r| Link {
            text: r.get("text").and_then(Value::as_str).unwrap_or_default().to_string(),
            id: browse_id(r.get("navigationEndpoint")),
        })
        .collect()
}

fn str_at<'a>(v: &'a Value, ptr: &str) -> Option<&'a str> {
    v.pointer(ptr).and_then(Value::as_str)
}

/// Turns any navigation/play endpoint into a [`Target`].
pub fn target(ep: &Value) -> Option<Target> {
    let ep = ep.get("innertubeCommand").unwrap_or(ep);
    if let Some(b) = ep.get("browseEndpoint") {
        return Some(Target::Browse {
            id: b.get("browseId")?.as_str()?.to_string(),
            params: b.get("params").and_then(Value::as_str).map(str::to_string),
        });
    }
    if let Some(w) = ep.get("watchEndpoint") {
        return Some(Target::Watch {
            video_id: w.get("videoId").and_then(Value::as_str).map(str::to_string),
            playlist_id: w.get("playlistId").and_then(Value::as_str).map(str::to_string),
            params: w.get("params").and_then(Value::as_str).map(str::to_string),
            index: w.get("index").and_then(Value::as_u64),
        });
    }
    if let Some(w) = ep.get("watchPlaylistEndpoint") {
        return Some(Target::Watch {
            video_id: None,
            playlist_id: Some(w.get("playlistId")?.as_str()?.to_string()),
            params: w.get("params").and_then(Value::as_str).map(str::to_string),
            index: None,
        });
    }
    if let Some(s) = ep.get("searchEndpoint") {
        return Some(Target::Search {
            query: s.get("query")?.as_str()?.to_string(),
            params: s.get("params").and_then(Value::as_str).map(str::to_string),
        });
    }
    None
}

fn page_type(ep: Option<&Value>) -> &str {
    ep.and_then(|e| e.pointer("/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

fn video_kind(ep: Option<&Value>) -> VideoKind {
    let t = ep
        .and_then(|e| e.pointer("/watchEndpoint/watchEndpointMusicSupportedConfigs/watchEndpointMusicConfig/musicVideoType"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match t {
        "MUSIC_VIDEO_TYPE_ATV" | "" => VideoKind::Song,
        "MUSIC_VIDEO_TYPE_PODCAST_EPISODE" => VideoKind::Episode,
        _ => VideoKind::Video,
    }
}

/// The largest image in the first `thumbnails` list found under `v`.
fn best_thumb(v: &Value) -> Option<String> {
    fn find(v: &Value, depth: u32) -> Option<&Vec<Value>> {
        if depth > 6 {
            return None;
        }
        match v {
            Value::Object(m) => {
                if let Some(a) = m.get("thumbnails").and_then(Value::as_array).filter(|a| !a.is_empty()) {
                    return Some(a);
                }
                if let Some(a) = m.get("sources").and_then(Value::as_array).filter(|a| !a.is_empty()) {
                    return Some(a);
                }
                m.values().find_map(|x| find(x, depth + 1))
            }
            Value::Array(a) => a.iter().find_map(|x| find(x, depth + 1)),
            _ => None,
        }
    }
    let list = find(v, 0)?;
    let best = list.iter().max_by_key(|t| t.get("width").and_then(Value::as_u64).unwrap_or(0))?;
    let url = best.get("url")?.as_str()?;
    Some(if url.starts_with("//") { format!("https:{url}") } else { url.to_string() })
}

/// Asks Google's image servers for a square image of `px` pixels (artwork
/// and avatars come with a size suffix such as `=w120-h120-l90-rj`).
pub fn image_url(url: &str, px: u32) -> String {
    static WH: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"=w(\d+)-h(\d+)").unwrap());
    static S: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"=s(\d+)").unwrap());
    if !(url.contains("googleusercontent.com") || url.contains("ggpht.com")) {
        return url.to_string();
    }
    if let Some(c) = WH.captures(url)
        && c[1] == c[2]
    {
        return WH.replace(url, format!("=w{px}-h{px}").as_str()).into_owned();
    }
    if S.is_match(url) {
        return S.replace(url, format!("=s{px}").as_str()).into_owned();
    }
    url.to_string()
}

fn is_duration(s: &str) -> bool {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"^\d{1,2}(:\d{2}){1,2}$").unwrap());
    RE.is_match(s.trim())
}

/// The type word that starts rows of mixed search results ("Song • Artist
/// • 54M plays"), in the languages YouTube Music is most used in.
fn type_label(s: &str) -> Option<VideoKind> {
    match s {
        "Song" | "Brano" | "Canción" | "Titre" | "Música" | "Titel" | "Utwór" | "Nummer" | "Låt" | "Песня" | "Şarkı" | "曲" | "노래" => Some(VideoKind::Song),
        "Video" | "Vídeo" | "Vidéo" | "Wideo" | "Видео" | "動画" | "동영상" => Some(VideoKind::Video),
        "Episode" | "Episodio" | "Épisode" | "Episódio" | "Folge" | "Odcinek" | "Aflevering" | "Эпизод" | "Bölüm" | "エピソード" | "에피소드" => Some(VideoKind::Episode),
        _ => None,
    }
}

fn is_separator(s: &str) -> bool {
    matches!(s.trim(), "•" | "&" | "," | "·" | "")
}

fn explicit(it: &Value) -> bool {
    let has = |key: &str| {
        it.get(key).and_then(Value::as_array).is_some_and(|a| {
            a.iter().any(|b| str_at(b, "/musicInlineBadgeRenderer/icon/iconType") == Some("MUSIC_EXPLICIT_BADGE"))
        })
    };
    has("badges") || has("subtitleBadges")
}

fn is_round(it: &Value, ep: Option<&Value>) -> bool {
    let crop = it
        .pointer("/thumbnail/musicThumbnailRenderer/thumbnailCrop")
        .or_else(|| it.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnailCrop"))
        .and_then(Value::as_str);
    crop == Some("MUSIC_THUMBNAIL_CROP_CIRCLE") || matches!(page_type(ep), "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL")
}

/// Splits runs into " • "-separated groups.
fn groups(links: &[Link]) -> Vec<Vec<Link>> {
    let mut out = vec![Vec::new()];
    for l in links {
        if l.text.trim() == "•" {
            out.push(Vec::new());
        } else if let Some(g) = out.last_mut() {
            g.push(l.clone());
        }
    }
    out.retain(|g| !g.is_empty());
    out
}

/// Sorts byline runs ("Artist & Artist • Album • 2013 • 5:38") into the
/// track's fields.
fn fill_byline(t: &mut Track, cols: &[Vec<Link>]) {
    let mut plain_artists: Option<Vec<Link>> = None;
    let mut info = Vec::new();
    for (ci, col) in cols.iter().enumerate() {
        let gs = groups(col);
        let later_has_artist = |from: usize| gs[from..].iter().flatten().any(|l| l.id.as_deref().is_some_and(|i| i.starts_with("UC")));
        for (gi, g) in gs.iter().enumerate() {
            let joined: String = g.iter().map(|l| l.text.as_str()).collect::<String>().trim().to_string();
            if joined.is_empty() {
                continue;
            }
            let ids: Vec<&str> = g.iter().filter_map(|l| l.id.as_deref()).collect();
            if ids.iter().any(|i| i.starts_with("MPREb")) {
                t.album = g.iter().find(|l| l.id.is_some()).cloned();
            } else if ids.iter().any(|i| i.starts_with("UC")) {
                t.artists = g.iter().filter(|l| !is_separator(&l.text)).cloned().collect();
            } else if is_duration(&joined) {
                t.duration = Some(joined);
            } else if ci == 0 && gi == 0 && gs.len() > 1 && (later_has_artist(1) || type_label(&joined).is_some()) {
                // A type label ("Song", "Video", "Episode") in mixed results.
                if let Some(k) = type_label(&joined) {
                    t.kind = k;
                } else if joined == "Video" {
                    t.kind = VideoKind::Video;
                }
            } else if joined.chars().any(|c| c.is_ascii_digit()) {
                info.push(joined);
            } else if plain_artists.is_none() && ci == 0 {
                plain_artists = Some(g.iter().filter(|l| !is_separator(&l.text)).cloned().collect());
            } else if t.album.is_none() && ci > 0 {
                t.album = Some(Link { text: joined, id: None });
            }
        }
    }
    if t.artists.is_empty()
        && let Some(p) = plain_artists
    {
        t.artists = p;
    }
    if !info.is_empty() {
        t.info = Some(info.join(" • "));
    }
}

fn flex_columns(it: &Value) -> Vec<&Value> {
    it.get("flexColumns")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text")).collect())
        .unwrap_or_default()
}

fn parse_list_item(it: &Value) -> Option<Entry> {
    let cols = flex_columns(it);
    let title_col = *cols.first()?;
    let title = nonempty(text(Some(title_col)))?;
    let title_ep = title_col.pointer("/runs/0/navigationEndpoint");
    let nav = it.get("navigationEndpoint");
    let play_ep = it.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint");
    let thumbnail = it.get("thumbnail").and_then(best_thumb);

    if let Some(nav) = nav.filter(|n| n.get("browseEndpoint").is_some()) {
        let mut subtitle: Vec<Link> = Vec::new();
        for c in cols.iter().skip(1) {
            let r = runs(Some(c));
            if r.iter().all(|l| l.text.trim().is_empty()) {
                continue;
            }
            if !subtitle.is_empty() {
                subtitle.push(Link { text: " • ".into(), id: None });
            }
            subtitle.extend(r);
        }
        return Some(Entry::Card(Card {
            title,
            subtitle,
            thumbnail,
            target: target(nav),
            play: play_ep.and_then(target),
            shape: if is_round(it, Some(nav)) { Shape::Round } else { Shape::Square },
            explicit: explicit(it),
        }));
    }

    // The play button's endpoint carries the list (album, radio) to play in.
    let watch = [play_ep, title_ep].into_iter().flatten().find(|e| e.get("watchEndpoint").is_some());
    let video_id = str_at(it, "/playlistItemData/videoId")
        .or_else(|| watch.and_then(|w| str_at(w, "/watchEndpoint/videoId")))?
        .to_string();
    let mut t = Track {
        video_id,
        title,
        thumbnail,
        kind: video_kind(watch),
        explicit: explicit(it),
        index: nonempty(text(it.get("index"))),
        playlist_id: watch.and_then(|w| str_at(w, "/watchEndpoint/playlistId")).map(str::to_string),
        params: watch.and_then(|w| str_at(w, "/watchEndpoint/params")).map(str::to_string),
        list_index: watch.and_then(|w| w.pointer("/watchEndpoint/index")).and_then(Value::as_u64),
        ..Default::default()
    };
    let byline: Vec<Vec<Link>> = cols.iter().skip(1).map(|c| runs(Some(c))).collect();
    fill_byline(&mut t, &byline);
    if let Some(fixed) = it.get("fixedColumns").and_then(Value::as_array) {
        for f in fixed {
            if let Some(s) = nonempty(text(f.pointer("/musicResponsiveListItemFixedColumnRenderer/text")))
                && is_duration(&s)
            {
                t.duration = Some(s);
            }
        }
    }
    Some(Entry::Track(t))
}

fn parse_two_row(it: &Value) -> Option<Entry> {
    let title = nonempty(text(it.get("title")))?;
    let nav = it.get("navigationEndpoint").or_else(|| it.pointer("/title/runs/0/navigationEndpoint"));
    let play = it.pointer("/thumbnailOverlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint");
    let aspect = it.get("aspectRatio").and_then(Value::as_str).unwrap_or("");
    let shape = if aspect.contains("16_9") {
        Shape::Wide
    } else if is_round(it, nav) {
        Shape::Round
    } else {
        Shape::Square
    };
    Some(Entry::Card(Card {
        title,
        subtitle: runs(it.get("subtitle")),
        thumbnail: it.get("thumbnailRenderer").and_then(best_thumb),
        target: nav.and_then(target),
        play: play.and_then(target),
        shape,
        explicit: explicit(it),
    }))
}

fn parse_mood(it: &Value) -> Option<Entry> {
    Some(Entry::Mood(Mood {
        title: nonempty(text(it.get("buttonText")))?,
        target: target(it.get("clickCommand")?)?,
        color: it.pointer("/solid/leftStripeColor").and_then(Value::as_u64).map(|c| c as u32),
    }))
}

/// A queue item ("Up next").
fn parse_panel_video(it: &Value) -> Option<Track> {
    if it.get("unplayableText").is_some() {
        return None;
    }
    let video_id = it.get("videoId").and_then(Value::as_str)?.to_string();
    let ep = it.get("navigationEndpoint");
    let mut t = Track {
        video_id,
        title: nonempty(text(it.get("title")))?,
        duration: nonempty(text(it.get("lengthText"))),
        thumbnail: it.get("thumbnail").and_then(best_thumb),
        kind: video_kind(ep),
        explicit: explicit(it),
        playlist_id: ep.and_then(|e| str_at(e, "/watchEndpoint/playlistId")).map(str::to_string),
        params: ep.and_then(|e| str_at(e, "/watchEndpoint/params")).map(str::to_string),
        list_index: ep.and_then(|e| e.pointer("/watchEndpoint/index")).and_then(Value::as_u64),
        ..Default::default()
    };
    let byline = runs(it.get("longBylineText").or_else(|| it.get("shortBylineText")));
    fill_byline(&mut t, &[byline]);
    Some(t)
}

fn parse_entry(k: &str, it: &Value) -> Option<Entry> {
    match k {
        "musicResponsiveListItemRenderer" => parse_list_item(it),
        "musicTwoRowItemRenderer" => parse_two_row(it),
        "musicNavigationButtonRenderer" => parse_mood(it),
        "playlistPanelVideoRenderer" => parse_panel_video(it).map(Entry::Track),
        _ => None,
    }
}

fn first_kv(v: &Value) -> Option<(&str, &Value)> {
    v.as_object()?.iter().next().map(|(k, v)| (k.as_str(), v))
}

fn continuation_token(v: &Value) -> Option<String> {
    for p in ["/continuations/0/nextContinuationData/continuation", "/continuations/0/nextRadioContinuationData/continuation"] {
        if let Some(t) = str_at(v, p) {
            return Some(t.to_string());
        }
    }
    for key in ["contents", "items", "continuationItems"] {
        if let Some(last) = v.get(key).and_then(Value::as_array).and_then(|a| a.last())
            && let Some(t) = str_at(last, "/continuationItemRenderer/continuationEndpoint/continuationCommand/token")
        {
            return Some(t.to_string());
        }
    }
    None
}

fn entries_of(list: Option<&Value>) -> Vec<Entry> {
    list.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|i| first_kv(i).and_then(|(k, v)| parse_entry(k, v))).collect())
        .unwrap_or_default()
}

fn parse_top_result(s: &Value) -> Option<TopResult> {
    let title_ep = s.pointer("/title/runs/0/navigationEndpoint").or_else(|| s.get("onTap"));
    let card = Card {
        title: nonempty(text(s.get("title")))?,
        subtitle: runs(s.get("subtitle")),
        thumbnail: s.get("thumbnail").and_then(best_thumb),
        target: title_ep.and_then(target),
        play: None,
        shape: if is_round(s, title_ep) { Shape::Round } else { Shape::Square },
        explicit: false,
    };
    let tracks = entries_of(s.get("contents"))
        .into_iter()
        .filter_map(|e| if let Entry::Track(t) = e { Some(t) } else { None })
        .collect();
    let mut top = TopResult { card, tracks, shuffle: None, radio: None };
    for b in s.get("buttons").and_then(Value::as_array).into_iter().flatten() {
        let Some(br) = b.get("buttonRenderer") else { continue };
        let t = br.get("command").or_else(|| br.get("navigationEndpoint")).and_then(target);
        match str_at(br, "/icon/iconType") {
            Some("MUSIC_SHUFFLE") => top.shuffle = t,
            Some("MIX") => top.radio = t,
            _ => {
                if top.card.play.is_none() {
                    top.card.play = t;
                }
            }
        }
    }
    Some(top)
}

fn parse_shelf(k: &str, s: &Value) -> Option<Shelf> {
    let mut shelf = Shelf::default();
    match k {
        "musicCarouselShelfRenderer" | "musicImmersiveCarouselShelfRenderer" => {
            shelf.style = ShelfStyle::Carousel;
            if let Some((_, h)) = s.get("header").and_then(first_kv) {
                shelf.title = nonempty(text(h.get("title")));
                shelf.title_target = h.pointer("/title/runs/0/navigationEndpoint").and_then(target);
                shelf.strapline = nonempty(text(h.get("strapline")));
                if let Some(b) = h.pointer("/moreContentButton/buttonRenderer")
                    && let (Some(label), Some(t)) = (nonempty(text(b.get("text"))), b.get("navigationEndpoint").and_then(target))
                {
                    shelf.more = Some((label, t));
                }
            }
            shelf.entries = entries_of(s.get("contents"));
        }
        "musicShelfRenderer" | "musicPlaylistShelfRenderer" => {
            shelf.title = nonempty(text(s.get("title")));
            shelf.title_target = s.pointer("/title/runs/0/navigationEndpoint").and_then(target);
            if let (Some(label), Some(t)) = (nonempty(text(s.get("bottomText"))), s.get("bottomEndpoint").and_then(target)) {
                shelf.more = Some((label, t));
            }
            shelf.entries = entries_of(s.get("contents"));
            shelf.continuation = continuation_token(s);
        }
        "gridRenderer" => {
            shelf.style = ShelfStyle::Grid;
            shelf.title = nonempty(text(s.pointer("/header/gridHeaderRenderer/title")));
            shelf.entries = entries_of(s.get("items"));
            shelf.continuation = continuation_token(s);
        }
        "musicDescriptionShelfRenderer" => {
            shelf.style = ShelfStyle::Text;
            shelf.title = nonempty(text(s.get("header")));
            shelf.text = nonempty(text(s.get("description")));
            shelf.footer = nonempty(text(s.get("footer")));
            shelf.text.as_ref()?;
        }
        _ => return None,
    }
    if shelf.style != ShelfStyle::Text && shelf.entries.is_empty() && shelf.continuation.is_none() {
        return None;
    }
    Some(shelf)
}

fn parse_chips(sl: &Value) -> Vec<Chip> {
    sl.pointer("/header/chipCloudRenderer/chips")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let c = c.get("chipCloudChipRenderer")?;
                    Some(Chip {
                        title: nonempty(text(c.get("text")))?,
                        target: target(c.get("navigationEndpoint")?)?,
                        selected: c.get("isSelected").and_then(Value::as_bool).unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Adds the sections of a section list to `page`.
fn add_sections(page: &mut Page, sections: &[Value]) {
    let mut loose = Shelf::default();
    let flush = |page: &mut Page, loose: &mut Shelf| {
        if !loose.entries.is_empty() {
            page.shelves.push(std::mem::take(loose));
        }
    };
    for sec in sections {
        let Some((k, s)) = first_kv(sec) else { continue };
        match k {
            "itemSectionRenderer" => {
                for c in s.get("contents").and_then(Value::as_array).into_iter().flatten() {
                    let Some((ck, cv)) = first_kv(c) else { continue };
                    if let Some(e) = parse_entry(ck, cv) {
                        loose.entries.push(e);
                    } else if let Some(sh) = parse_shelf(ck, cv) {
                        flush(page, &mut loose);
                        page.shelves.push(sh);
                    }
                }
            }
            "musicCardShelfRenderer" => {
                flush(page, &mut loose);
                if page.top.is_none() {
                    page.top = parse_top_result(s);
                }
            }
            "musicResponsiveHeaderRenderer" | "musicEditablePlaylistDetailHeaderRenderer" | "musicDetailHeaderRenderer" => {
                if page.header.is_none() {
                    page.header = parse_header(k, s);
                }
            }
            _ => {
                if let Some(sh) = parse_shelf(k, s) {
                    flush(page, &mut loose);
                    page.shelves.push(sh);
                }
            }
        }
    }
    flush(page, &mut loose);
}

fn add_section_list(page: &mut Page, sl: &Value) {
    if page.chips.is_empty() {
        page.chips = parse_chips(sl);
    }
    if let Some(a) = sl.get("contents").and_then(Value::as_array) {
        add_sections(page, a);
    }
    if let Some(t) = continuation_token(sl) {
        page.continuation = Some(t);
    }
}

fn button_target(b: &Value) -> Option<(String, Target)> {
    let br = b.get("buttonRenderer")?;
    let t = br.get("navigationEndpoint").or_else(|| br.get("command")).and_then(target)?;
    Some((nonempty(text(br.get("text"))).unwrap_or_default(), t))
}

fn parse_header(k: &str, h: &Value) -> Option<Header> {
    match k {
        "musicEditablePlaylistDetailHeaderRenderer" => {
            let (ik, iv) = first_kv(h.get("header")?)?;
            let mut hd = parse_header(ik, iv)?;
            if let Some(pid) = str_at(h, "/playlistId") {
                hd.playlist_id = Some(pid.to_string());
            }
            Some(hd)
        }
        "musicImmersiveHeaderRenderer" | "musicVisualHeaderRenderer" => {
            let sub = h.pointer("/subscriptionButton/subscribeButtonRenderer");
            Some(Header {
                kind: HeaderKind::Artist,
                title: nonempty(text(h.get("title")))?,
                description: nonempty(text(h.get("description"))),
                thumbnail: h.get("thumbnail").and_then(best_thumb).or_else(|| h.get("foregroundThumbnail").and_then(best_thumb)),
                play: h.get("playButton").and_then(button_target),
                radio: h.get("startRadioButton").and_then(button_target),
                channel_id: sub.and_then(|s| str_at(s, "/channelId")).map(str::to_string),
                subscribed: sub.and_then(|s| s.get("subscribed")).and_then(Value::as_bool),
                subscribers: sub.and_then(|s| nonempty(text(s.get("longSubscriberCountText")).or_else(|| text(s.get("subscriberCountText"))))),
                monthly: nonempty(text(h.get("monthlyListenerCount"))),
                ..Default::default()
            })
        }
        "musicResponsiveHeaderRenderer" | "musicDetailHeaderRenderer" => {
            let mut hd = Header {
                kind: HeaderKind::Collection,
                title: nonempty(text(h.get("title")))?,
                subtitle: nonempty(text(h.get("subtitle"))),
                second_subtitle: nonempty(text(h.get("secondSubtitle"))),
                owner: runs(h.get("straplineTextOne")).into_iter().filter(|l| !l.text.trim().is_empty()).collect(),
                description: nonempty(text(h.pointer("/description/musicDescriptionShelfRenderer/description")))
                    .or_else(|| nonempty(text(h.get("description")))),
                thumbnail: h.get("thumbnail").and_then(best_thumb),
                ..Default::default()
            };
            if hd.owner.is_empty()
                && let Some(av) = h.pointer("/facepile/avatarStackViewModel")
                && let Some(name) = nonempty(text(av.get("text")))
            {
                let id = av
                    .pointer("/rendererContext/commandContext/onTap/innertubeCommand/browseEndpoint/browseId")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                hd.owner = vec![Link { text: name, id }];
            }
            let mut menu_items = Vec::new();
            for b in h.get("buttons").and_then(Value::as_array).into_iter().flatten() {
                if let Some(p) = b.pointer("/musicPlayButtonRenderer/playNavigationEndpoint").and_then(target) {
                    hd.play = Some(("Play".into(), p));
                }
                if let Some(tb) = b.get("toggleButtonRenderer") {
                    hd.saved = tb.get("isToggled").and_then(Value::as_bool);
                    if let Some(pid) = str_at(tb, "/defaultServiceEndpoint/likeEndpoint/target/playlistId") {
                        hd.playlist_id = Some(pid.to_string());
                    }
                }
                if let Some(items) = b.pointer("/menuRenderer/items").and_then(Value::as_array) {
                    menu_items.extend(items.iter());
                }
            }
            for it in h.pointer("/menu/menuRenderer/items").and_then(Value::as_array).into_iter().flatten() {
                menu_items.push(it);
            }
            for it in menu_items {
                let Some(m) = it.get("menuNavigationItemRenderer") else { continue };
                let t = m.get("navigationEndpoint").and_then(target);
                match str_at(m, "/icon/iconType") {
                    Some("MUSIC_SHUFFLE") => hd.shuffle = t,
                    Some("MIX") => hd.radio = t.map(|t| (nonempty(text(m.get("text"))).unwrap_or_else(|| "Radio".into()), t)),
                    _ => {}
                }
            }
            if hd.playlist_id.is_none() {
                // The album's/playlist's own list id, for saving it.
                hd.playlist_id = [hd.shuffle.as_ref(), hd.play.as_ref().map(|p| &p.1)].into_iter().flatten().find_map(|t| match t {
                    Target::Watch { playlist_id: Some(p), .. } if !p.starts_with("RDAM") => Some(p.clone()),
                    _ => None,
                });
            }
            Some(hd)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Public parsers

/// Any browse or search response.
pub fn parse_page(v: &Value) -> Page {
    let mut page = Page::default();
    if let Some((k, h)) = v.get("header").and_then(first_kv) {
        if k == "musicHeaderRenderer" {
            page.title = nonempty(text(h.get("title")));
        } else {
            page.header = parse_header(k, h);
        }
    }
    let c = v.get("contents");
    let tab_list = |root: &str| c.and_then(|c| c.pointer(&format!("/{root}/tabs/0/tabRenderer/content/sectionListRenderer")));
    if let Some(sl) = tab_list("singleColumnBrowseResultsRenderer") {
        add_section_list(&mut page, sl);
    }
    if let Some(sl) = tab_list("tabbedSearchResultsRenderer") {
        add_section_list(&mut page, sl);
    }
    if let Some(sl) = tab_list("twoColumnBrowseResultsRenderer") {
        add_section_list(&mut page, sl);
    }
    if let Some(sl) = c.and_then(|c| c.pointer("/twoColumnBrowseResultsRenderer/secondaryContents/sectionListRenderer")) {
        add_section_list(&mut page, sl);
    }
    if let Some(sl) = c.and_then(|c| c.get("sectionListRenderer")) {
        add_section_list(&mut page, sl);
    }
    if let Some(sl) = v.pointer("/continuationContents/sectionListContinuation") {
        add_section_list(&mut page, sl);
    }
    if page.title.is_none() {
        page.title = page.header.as_ref().map(|h| h.title.clone());
    }
    page
}

/// A continuation response (more shelves, or more rows for one shelf).
pub fn parse_more(v: &Value) -> More {
    let mut more = More::default();
    if let Some(cc) = v.get("continuationContents").and_then(Value::as_object) {
        for (k, c) in cc {
            match k.as_str() {
                "sectionListContinuation" => {
                    let mut p = Page::default();
                    add_section_list(&mut p, c);
                    more.shelves = p.shelves;
                    more.continuation = p.continuation;
                }
                "musicShelfContinuation" | "musicPlaylistShelfContinuation" => {
                    more.entries = entries_of(c.get("contents"));
                    more.continuation = continuation_token(c);
                }
                "gridContinuation" => {
                    more.entries = entries_of(c.get("items"));
                    more.continuation = continuation_token(c);
                }
                _ => {}
            }
        }
    }
    for a in v.get("onResponseReceivedActions").and_then(Value::as_array).into_iter().flatten() {
        if let Some(items) = a.pointer("/appendContinuationItemsAction/continuationItems") {
            more.entries.extend(entries_of(Some(items)));
            if let Some(t) = continuation_token(a.get("appendContinuationItemsAction").unwrap_or(&Value::Null)) {
                more.continuation = Some(t);
            }
        }
    }
    more
}

fn panel_items(contents: Option<&Value>) -> (Vec<Track>, Option<usize>, Option<Target>) {
    let mut tracks = Vec::new();
    let mut selected = None;
    let mut automix = None;
    for item in contents.and_then(Value::as_array).into_iter().flatten() {
        let Some((k, it)) = first_kv(item) else { continue };
        match k {
            "playlistPanelVideoRenderer" => {
                if let Some(t) = parse_panel_video(it) {
                    if it.get("selected").and_then(Value::as_bool) == Some(true) {
                        selected = Some(tracks.len());
                    }
                    tracks.push(t);
                }
            }
            "playlistPanelVideoWrapperRenderer" => {
                let Some(primary) = it.pointer("/primaryRenderer/playlistPanelVideoRenderer") else { continue };
                let Some(mut t) = parse_panel_video(primary) else { continue };
                if let Some(cp) = it.pointer("/counterpart/0/counterpartRenderer/playlistPanelVideoRenderer").and_then(parse_panel_video) {
                    t.counterpart = Some(Box::new(cp));
                }
                if primary.get("selected").and_then(Value::as_bool) == Some(true) {
                    selected = Some(tracks.len());
                }
                tracks.push(t);
            }
            "automixPreviewVideoRenderer" => {
                automix = it.pointer("/content/automixPlaylistVideoRenderer/navigationEndpoint").and_then(target);
            }
            _ => {}
        }
    }
    (tracks, selected, automix)
}

/// A `next` response: the queue and the selected track's lyrics/related tabs.
pub fn parse_queue(v: &Value) -> Queue {
    let mut q = Queue::default();
    let tabs = v
        .pointer("/contents/singleColumnMusicWatchNextResultsRenderer/tabbedRenderer/watchNextTabbedResultsRenderer/tabs")
        .and_then(Value::as_array);
    for tab in tabs.into_iter().flatten() {
        let Some(tr) = tab.get("tabRenderer") else { continue };
        if let Some(panel) = tr.pointer("/content/musicQueueRenderer/content/playlistPanelRenderer") {
            let (tracks, selected, automix) = panel_items(panel.get("contents"));
            q.tracks = tracks;
            q.selected = selected;
            q.automix = automix;
            q.continuation = continuation_token(panel);
            q.title = nonempty(text(panel.get("title")));
            q.playlist_id = panel.get("playlistId").and_then(Value::as_str).map(str::to_string);
        }
        if tr.get("unselectable").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        if let Some(id) = browse_id(tr.get("endpoint")) {
            if id.starts_with("MPLYt") {
                q.lyrics_id = Some(id);
            } else if id.starts_with("MPTRt") {
                q.related_id = Some(id);
            }
        }
    }
    q.like_status = v
        .pointer("/playerOverlays/playerOverlayRenderer/actions/0/likeButtonRenderer/likeStatus")
        .and_then(Value::as_str)
        .map(str::to_string);
    q
}

/// More queue items (radio continuation).
pub fn parse_queue_more(v: &Value) -> (Vec<Track>, Option<String>, Option<Target>) {
    let Some(c) = v.pointer("/continuationContents/playlistPanelContinuation") else {
        return (Vec::new(), None, None);
    };
    let (tracks, _, automix) = panel_items(c.get("contents"));
    (tracks, continuation_token(c), automix)
}

pub fn parse_lyrics(v: &Value) -> Option<Lyrics> {
    let mut found = None;
    fn walk(v: &Value, out: &mut Option<Lyrics>) {
        if out.is_some() {
            return;
        }
        match v {
            Value::Object(m) => {
                if let Some(d) = m.get("musicDescriptionShelfRenderer")
                    && let Some(body) = nonempty(text(d.get("description")))
                {
                    *out = Some(Lyrics::plain(body, nonempty(text(d.get("footer")))));
                    return;
                }
                for x in m.values() {
                    walk(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(v.get("contents")?, &mut found);
    found
}

pub fn parse_suggestions(v: &Value) -> Vec<Suggestion> {
    let mut out = Vec::new();
    for sec in v.get("contents").and_then(Value::as_array).into_iter().flatten() {
        for c in sec.pointer("/searchSuggestionsSectionRenderer/contents").and_then(Value::as_array).into_iter().flatten() {
            let Some((k, it)) = first_kv(c) else { continue };
            if k == "searchSuggestionRenderer" {
                let q = str_at(it, "/navigationEndpoint/searchEndpoint/query").map(str::to_string).or_else(|| text(it.get("suggestion")));
                if let Some(q) = nonempty(q) {
                    out.push(Suggestion::Query(q));
                }
            } else if let Some(e) = parse_entry(k, it) {
                out.push(Suggestion::Entry(e));
            }
        }
    }
    out
}

/// A music.youtube.com link → what it opens.
pub fn parse_music_url(input: &str) -> Option<Target> {
    let input = input.trim();
    let with_scheme = if input.starts_with("music.youtube.com") { format!("https://{input}") } else { input.to_string() };
    let url = url::Url::parse(&with_scheme).ok()?;
    if url.host_str()? != "music.youtube.com" {
        return None;
    }
    let q = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
    let segs: Vec<&str> = url.path_segments().map(|s| s.filter(|x| !x.is_empty()).collect()).unwrap_or_default();
    match segs.as_slice() {
        ["watch"] => Some(Target::Watch { video_id: q("v"), playlist_id: q("list"), params: None, index: None }),
        ["playlist"] => q("list").map(|l| Target::browse(&format!("VL{l}"))),
        ["browse", id] | ["channel", id] => Some(Target::browse(id)),
        ["search"] => q("q").map(|query| Target::Search { query, params: None }),
        [] => Some(Target::browse("FEmusic_home")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(text: &str, id: Option<&str>) -> Link {
        Link { text: text.into(), id: id.map(str::to_string) }
    }

    #[test]
    fn byline_with_type_label_artists_album_and_duration() {
        let mut t = Track::default();
        let col = vec![
            l("Song", None),
            l(" • ", None),
            l("Daft Punk", Some("UCa")),
            l(" & ", None),
            l("Julian Casablancas", Some("UCb")),
            l(" • ", None),
            l("Random Access Memories", Some("MPREb_x")),
            l(" • ", None),
            l("5:38", None),
        ];
        fill_byline(&mut t, &[col, vec![l("1.2B plays", None)]]);
        assert_eq!(t.artist_names(), "Daft Punk, Julian Casablancas");
        assert_eq!(t.album.as_ref().map(|a| a.text.as_str()), Some("Random Access Memories"));
        assert_eq!(t.duration.as_deref(), Some("5:38"));
        assert_eq!(t.info.as_deref(), Some("1.2B plays"));
    }

    #[test]
    fn byline_with_type_label_and_unlinked_artist() {
        let mut t = Track::default();
        fill_byline(&mut t, &[vec![l("Song", None), l(" • ", None), l("Daft Punk", None), l(" • ", None), l("54M plays", None)]]);
        assert_eq!(t.artist_names(), "Daft Punk");
        assert_eq!(t.info.as_deref(), Some("54M plays"));
        let mut v = Track::default();
        fill_byline(&mut v, &[vec![l("Video", None), l(" • ", None), l("Daft Punk", None), l(" • ", None), l("3:27", None)]]);
        assert_eq!((v.kind, v.artist_names().as_str(), v.duration.as_deref()), (VideoKind::Video, "Daft Punk", Some("3:27")));
    }

    #[test]
    fn byline_with_unlinked_artist() {
        let mut t = Track::default();
        fill_byline(&mut t, &[vec![l("Dante Velar", None), l(" • ", None), l("2:41", None)]]);
        assert_eq!(t.artist_names(), "Dante Velar");
        assert_eq!(t.duration.as_deref(), Some("2:41"));
    }

    #[test]
    fn image_sizes() {
        assert_eq!(image_url("https://lh3.googleusercontent.com/abc=w120-h120-l90-rj", 544), "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj");
        assert_eq!(image_url("https://yt3.ggpht.com/x=s48-c-k", 240), "https://yt3.ggpht.com/x=s240-c-k");
        assert_eq!(image_url("https://lh3.googleusercontent.com/b=w1440-h600-p", 544), "https://lh3.googleusercontent.com/b=w1440-h600-p");
        assert_eq!(image_url("https://i.ytimg.com/vi/x/hqdefault.jpg", 544), "https://i.ytimg.com/vi/x/hqdefault.jpg");
    }

    #[test]
    fn music_urls() {
        assert_eq!(parse_music_url("https://music.youtube.com/watch?v=abc&list=RDx"), Some(Target::Watch {
            video_id: Some("abc".into()),
            playlist_id: Some("RDx".into()),
            params: None,
            index: None
        }));
        assert_eq!(parse_music_url("music.youtube.com/playlist?list=PL1"), Some(Target::browse("VLPL1")));
        assert_eq!(parse_music_url("https://music.youtube.com/browse/MPREb_1"), Some(Target::browse("MPREb_1")));
        assert_eq!(parse_music_url("https://www.youtube.com/watch?v=abc"), None);
    }

    /// Parser checks against saved YouTube Music responses:
    /// `GTUBE_MUSIC_FIXTURES=/dir cargo test music_fixtures -- --nocapture`
    #[test]
    fn music_fixtures() {
        let Ok(dir) = std::env::var("GTUBE_MUSIC_FIXTURES") else { return };
        let load = |name: &str| -> Option<Value> {
            let t = std::fs::read_to_string(format!("{dir}/{name}")).ok()?;
            serde_json::from_str(&t).ok()
        };
        let count = |p: &Page| -> (usize, usize, usize) {
            let mut n = (0, 0, 0);
            for s in &p.shelves {
                for e in &s.entries {
                    match e {
                        Entry::Track(_) => n.0 += 1,
                        Entry::Card(_) => n.1 += 1,
                        Entry::Mood(_) => n.2 += 1,
                    }
                }
            }
            n
        };
        if let Some(v) = load("home.json") {
            let p = parse_page(&v);
            println!("home: {} shelves {:?}, chips {}, cont {}", p.shelves.len(), count(&p), p.chips.len(), p.continuation.is_some());
            assert!(!p.shelves.is_empty() && p.continuation.is_some() && !p.chips.is_empty());
            let quick = &p.shelves[0];
            let Entry::Track(t) = &quick.entries[0] else { panic!("quick picks should be tracks") };
            println!("  first: {} — {} [{:?}] {:?} {:?}", t.title, t.artist_names(), t.album, t.info, t.playlist_id);
            assert!(!t.artists.is_empty() && t.album.is_some());
        }
        if let Some(v) = load("home_cont.json") {
            let m = parse_more(&v);
            println!("home continuation: {} shelves", m.shelves.len());
            assert!(!m.shelves.is_empty());
        }
        for name in ["explore.json", "moods.json", "charts.json", "newrel.json", "mood_cat.json", "related.json"] {
            if let Some(v) = load(name) {
                let p = parse_page(&v);
                println!("{name}: title {:?}, {} shelves {:?}", p.title, p.shelves.len(), count(&p));
                assert!(!p.shelves.is_empty(), "{name}");
            }
        }
        if let Some(v) = load("search_all.json") {
            let p = parse_page(&v);
            let top = p.top.as_ref().expect("top result");
            println!("search all: top {:?} ({} tracks, shuffle {}), {} shelves {:?}, chips {:?}", top.card.title, top.tracks.len(), top.shuffle.is_some(), p.shelves.len(), count(&p), p.chips.iter().map(|c| &c.title).collect::<Vec<_>>());
            assert!(!p.chips.is_empty() && !p.shelves.is_empty());
        }
        for name in ["search_songs.json", "search_albums.json", "search_artists.json", "search_videos.json", "search_cplaylists.json"] {
            if let Some(v) = load(name) {
                let p = parse_page(&v);
                let s = &p.shelves[0];
                println!("{name}: {:?} {} entries, cont {}", s.title, s.entries.len(), s.continuation.is_some());
                match &s.entries[0] {
                    Entry::Track(t) => println!("  {} — {} | {:?} | {:?} | {:?} | {:?}", t.title, t.artist_names(), t.album.as_ref().map(|a| &a.text), t.duration, t.info, t.kind),
                    Entry::Card(c) => println!("  {} — {} | {:?} {:?}", c.title, c.subtitle_text(), c.target, c.shape),
                    Entry::Mood(m) => println!("  mood {}", m.title),
                }
            }
        }
        if let Some(v) = load("artist.json") {
            let p = parse_page(&v);
            let h = p.header.as_ref().expect("artist header");
            println!("artist: {} {:?} {:?} sub {:?}/{:?} play {:?} radio {:?}; shelves {:?}", h.title, h.kind, h.monthly, h.channel_id, h.subscribers, h.play.as_ref().map(|p| &p.0), h.radio.as_ref().map(|p| &p.0), p.shelves.iter().map(|s| (s.title.clone(), s.style, s.entries.len())).collect::<Vec<_>>());
            assert_eq!(h.kind, HeaderKind::Artist);
            assert!(h.channel_id.is_some() && h.play.is_some());
        }
        for name in ["album.json", "playlist.json", "playlist_community.json"] {
            if let Some(v) = load(name) {
                let p = parse_page(&v);
                let h = p.header.as_ref().expect("collection header");
                let tracks = p.shelves.first().map(|s| s.entries.len()).unwrap_or(0);
                println!("{name}: {} | {:?} | {:?} | owner {:?} | play {} shuffle {} radio {} | list {:?} | {} tracks, {} shelves", h.title, h.subtitle, h.second_subtitle, h.owner.iter().map(|o| &o.text).collect::<Vec<_>>(), h.play.is_some(), h.shuffle.is_some(), h.radio.is_some(), h.playlist_id, tracks, p.shelves.len());
                assert!(h.play.is_some() && tracks > 0);
                if let Some(Entry::Track(t)) = p.shelves[0].entries.first() {
                    println!("  #{:?} {} — {} {:?} list {:?}", t.index, t.title, t.artist_names(), t.duration, t.playlist_id);
                }
            }
        }
        for name in ["next_radio.json", "next_album.json", "next_video.json"] {
            if let Some(v) = load(name) {
                let q = parse_queue(&v);
                println!("{name}: {} tracks, selected {:?}, cont {}, automix {}, lyrics {:?}, related {:?}", q.tracks.len(), q.selected, q.continuation.is_some(), q.automix.is_some(), q.lyrics_id, q.related_id);
                let t = &q.tracks[0];
                println!("  {} — {} | {:?} | {:?} | {:?}", t.title, t.artist_names(), t.album.as_ref().map(|a| &a.text), t.duration, t.info);
                assert!(!q.tracks.is_empty() && q.lyrics_id.is_some());
            }
        }
        if let Some(v) = load("next_radio_cont.json") {
            let (t, c, _) = parse_queue_more(&v);
            println!("radio continuation: {} tracks, more {}", t.len(), c.is_some());
            assert!(!t.is_empty() && c.is_some());
        }
        if let Some(v) = load("lyrics.json") {
            let l = parse_lyrics(&v).expect("lyrics");
            println!("lyrics: {} lines, source {:?}", l.text.lines().count(), l.source);
        }
        if let Some(v) = load("suggest.json") {
            let s = parse_suggestions(&v);
            println!("suggestions: {} ({} entries)", s.len(), s.iter().filter(|x| matches!(x, Suggestion::Entry(_))).count());
            assert!(!s.is_empty());
        }
    }
}
