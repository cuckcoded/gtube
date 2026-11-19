#[derive(Clone, Debug, Default)]
pub struct VideoItem {
    pub id: String,
    pub title: String,
    pub channel_name: Option<String>,
    pub channel_id: Option<String>,
    pub channel_avatar: Option<String>,
    pub thumbnail: Option<String>,
    pub duration: Option<String>,
    pub views: Option<String>,
    pub published: Option<String>,
    pub is_live: bool,
    pub is_short: bool,
    /// Watch progress in 0..=1, when YouTube reports it (logged in).
    pub progress: Option<f64>,
    /// Its entry in a playlist the user can edit (to remove or move it).
    pub set_video_id: Option<String>,
}

impl VideoItem {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }

    pub fn thumbnail_url(&self) -> String {
        self.thumbnail
            .clone()
            .unwrap_or_else(|| format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", self.id))
    }

    pub fn meta_line(&self) -> String {
        [self.views.as_deref(), self.published.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" • ")
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChannelItem {
    pub id: String,
    pub title: String,
    pub avatar: Option<String>,
    pub handle: Option<String>,
    pub subscribers: Option<String>,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaylistItem {
    pub id: String,
    pub title: String,
    pub thumbnail: Option<String>,
    pub video_count: Option<String>,
    pub channel_name: Option<String>,
    pub channel_id: Option<String>,
    /// The video it starts with (mixes are generated around one).
    pub video_id: Option<String>,
}

impl PlaylistItem {
    /// Mixes ("RD…") are endless playlists YouTube generates per viewer;
    /// they have no playlist page and are played from the watch page.
    pub fn is_mix(&self) -> bool {
        is_mix(&self.id)
    }
}

pub fn is_mix(list_id: &str) -> bool {
    list_id.starts_with("RD")
}

/// The playlist panel of a watch page that plays a playlist or a mix.
#[derive(Clone, Debug, Default)]
pub struct PlaylistPanel {
    pub id: String,
    pub title: String,
    pub owner: Option<String>,
    pub items: Vec<VideoItem>,
    /// Index (in `items`) of the video the page is for.
    pub current: usize,
    /// Mixes never end: more items come with each request.
    pub infinite: bool,
    pub total: Option<usize>,
}

#[derive(Clone, Debug)]
pub enum Item {
    Video(VideoItem),
    Channel(ChannelItem),
    Playlist(PlaylistItem),
}

#[derive(Clone, Debug)]
pub struct Chip {
    pub title: String,
    pub token: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    pub items: Vec<Item>,
    pub continuation: Option<String>,
    pub chips: Vec<Chip>,
}

#[derive(Clone, Debug, Default)]
pub struct VideoDetails {
    pub id: String,
    pub title: String,
    pub views: Option<String>,
    pub date: Option<String>,
    pub relative_date: Option<String>,
    pub description: String,
    /// (byte start, byte length, target) for links inside `description`.
    pub description_links: Vec<(usize, usize, String)>,
    pub channel: ChannelItem,
    pub likes: Option<String>,
    /// "LIKE", "DISLIKE" or "INDIFFERENT" (only meaningful when signed in).
    pub like_status: Option<String>,
    pub subscribed: Option<bool>,
    pub related: Feed,
    pub comments_token: Option<String>,
    pub comments_count: Option<String>,
    pub comment_sorts: Vec<(String, String)>,
    pub is_live: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Comment {
    pub author: String,
    pub author_channel_id: Option<String>,
    pub avatar: Option<String>,
    pub text: String,
    pub published: String,
    pub likes: Option<String>,
    pub reply_count: Option<String>,
    pub replies_token: Option<String>,
    pub is_creator: bool,
    pub is_verified: bool,
    pub pinned: Option<String>,
    pub hearted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct CommentsPage {
    pub comments: Vec<Comment>,
    pub continuation: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ChannelPage {
    pub info: ChannelItem,
    pub banner: Option<String>,
    pub video_count: Option<String>,
    pub tabs: Vec<(String, String)>,
    pub subscribed: Option<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct PlaylistPage {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub owner: Option<String>,
    pub owner_id: Option<String>,
    pub stats: Vec<String>,
    pub thumbnail: Option<String>,
    pub feed: Feed,
    /// The user's own playlist (or Watch later).
    pub editable: bool,
    /// PUBLIC, UNLISTED or PRIVATE, when shown.
    pub privacy: Option<String>,
}

/// One of the user's playlists, offered by "Save to playlist".
#[derive(Clone, Debug)]
pub struct SaveTarget {
    pub id: String,
    pub title: String,
    pub saved: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Account {
    pub name: String,
    pub handle: Option<String>,
    pub avatar: Option<String>,
    pub channel_id: Option<String>,
}

#[derive(Clone, Debug)]
pub enum Resolved {
    Video { id: String, playlist: Option<String>, start: Option<f64> },
    Channel(String),
    Playlist(String),
    Search(String),
}
