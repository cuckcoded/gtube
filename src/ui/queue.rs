//! Playing a list of videos: the queue on the watch page. It is filled from
//! a playlist (all of it, page by page as it plays), a mix (endless: more
//! comes as it plays) or by hand ("Play next" / "Add to queue").

use super::widgets::AspectBin;
use crate::api::{self, models::*, parse};
use crate::{images, runtime};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Runs once the queue has grown.
type Waiter = Box<dyn FnOnce(&Rc<QueuePanel>)>;

/// Pages of a playlist loaded at most to find a video in it, or to shuffle.
const MAX_PAGES: usize = 50;

#[derive(Clone, Default)]
pub struct Queue {
    pub title: String,
    pub owner: Option<String>,
    pub items: Vec<VideoItem>,
    pub index: usize,
    /// The playlist or mix it plays (None: put together by hand).
    pub list_id: Option<String>,
    /// Continuation for the rest of a long playlist.
    pub more: Option<String>,
    /// Number of videos in the playlist, when YouTube says.
    pub total: Option<usize>,
    pub repeat: bool,
    /// The original order, while shuffled.
    pub unshuffled: Option<Vec<VideoItem>>,
    /// A mix that stopped giving new videos.
    mix_done: bool,
}

impl Queue {
    pub fn is_mix(&self) -> bool {
        self.list_id.as_deref().is_some_and(is_mix)
    }

    fn can_grow(&self) -> bool {
        (self.is_mix() && !self.mix_done) || self.more.is_some()
    }

    /// Adds fetched videos. A mix's pages overlap, so it skips repeats.
    fn append(&mut self, items: Vec<VideoItem>) -> usize {
        let before = self.items.len();
        for v in items {
            if !self.is_mix() || !self.items.iter().any(|o| o.id == v.id) {
                if let Some(u) = &mut self.unshuffled {
                    u.push(v.clone());
                }
                self.items.push(v);
            }
        }
        self.items.len() - before
    }

    /// Shuffles the queue. The current video stays (first) unless
    /// `everything`, for starting a playlist shuffled.
    fn shuffle(&mut self, everything: bool) {
        if self.unshuffled.is_none() {
            self.unshuffled = Some(self.items.clone());
        }
        if self.items.is_empty() {
            return;
        }
        let current = (!everything).then(|| self.items.remove(self.index.min(self.items.len() - 1)));
        for i in (1..self.items.len()).rev() {
            let j = glib::random_int_range(0, i as i32 + 1) as usize;
            self.items.swap(i, j);
        }
        if let Some(c) = current {
            self.items.insert(0, c);
        }
        self.index = 0;
    }

    fn unshuffle(&mut self) {
        let Some(original) = self.unshuffled.take() else { return };
        let id = self.items.get(self.index).map(|v| v.id.clone());
        self.index = id.and_then(|id| original.iter().position(|v| v.id == id)).unwrap_or(0);
        self.items = original;
    }

    fn from_panel(p: PlaylistPanel, list: &str) -> Self {
        Queue {
            title: p.title,
            owner: p.owner,
            index: p.current,
            items: p.items,
            list_id: Some(if p.id.is_empty() { list.to_string() } else { p.id }),
            total: p.total,
            // Some generated lists ("RD…" album radios) do end.
            mix_done: !p.infinite,
            ..Default::default()
        }
    }
}

/// "123 videos" (from a playlist page's stats).
fn video_count(stats: &[String]) -> Option<usize> {
    stats.iter().filter(|s| s.contains("video")).find_map(|s| {
        let digits: String = s.chars().take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.').filter(char::is_ascii_digit).collect();
        digits.parse().ok()
    })
}

async fn fetch_panel(video: Option<String>, list: String, index: Option<usize>) -> anyhow::Result<Option<PlaylistPanel>> {
    runtime::run(async move {
        let v = api::client().next_in_list(video.as_deref(), &list, index).await?;
        Ok(parse::parse_playlist_panel(&v))
    })
    .await
}

/// The next page of a playlist.
async fn fetch_page(token: String) -> anyhow::Result<(Vec<VideoItem>, Option<String>)> {
    let o = super::feed::opts();
    runtime::run(async move {
        let v = api::client().browse_continuation(&token).await?;
        let f = parse::parse_feed(&v, o);
        Ok((super::playlist::videos_of(&f.items), f.continuation))
    })
    .await
}

/// Loads the rest of a long playlist (up to MAX_PAGES pages).
async fn load_rest(q: &mut Queue) {
    for _ in 0..MAX_PAGES {
        let Some(token) = q.more.take() else { return };
        match fetch_page(token.clone()).await {
            Ok((items, more)) => {
                q.append(items);
                q.more = more;
            }
            Err(e) => {
                crate::log::warn(&format!("playlist page: {e}"));
                q.more = Some(token);
                return;
            }
        }
    }
}

fn play(q: Queue, start: f64) {
    if let Some(v) = q.items.get(q.index).cloned() {
        super::ctx().watch.open(v, Some(q), start);
    }
}

/// Plays mix `list` from `video` (or wherever YouTube starts it).
pub fn open_mix(video: Option<String>, list: String, start: f64) {
    runtime::spawn_local(async move {
        match fetch_panel(video.clone(), list.clone(), None).await {
            Ok(Some(p)) if !p.items.is_empty() => play(Queue::from_panel(p, &list), start),
            r => {
                if let Err(e) = r {
                    crate::log::warn(&format!("mix {list}: {e}"));
                }
                match video {
                    Some(id) => super::ctx().watch.open(VideoItem { id, ..Default::default() }, None, start),
                    None => super::toast("Could not load the mix"),
                }
            }
        }
    });
}

/// Plays playlist `list` with all of it as the queue, from `video` (its
/// first video when None). `shuffle` starts it shuffled.
pub fn open_playlist(video: Option<String>, list: String, start: f64, shuffle: bool) {
    if is_mix(&list) {
        return open_mix(video, list, start);
    }
    runtime::spawn_local(async move {
        let o = super::feed::opts();
        let l = list.clone();
        let page = runtime::run(async move {
            let v = api::client().browse(&format!("VL{l}"), None).await?;
            Ok(parse::parse_playlist(&v, &l, o))
        })
        .await;
        let mut q = match page {
            Ok(p) => Queue {
                total: video_count(&p.stats),
                title: p.title,
                owner: p.owner,
                items: super::playlist::videos_of(&p.feed.items),
                list_id: Some(list.clone()),
                more: p.feed.continuation,
                ..Default::default()
            },
            Err(e) => {
                crate::log::warn(&format!("playlist {list}: {e}"));
                Queue::default()
            }
        };
        if shuffle {
            load_rest(&mut q).await;
            q.more = None;
            q.shuffle(true);
        }
        // Find the video, loading further pages when needed.
        let index = loop {
            let found = match &video {
                None => (!q.items.is_empty()).then_some(0),
                Some(id) => q.items.iter().position(|v| &v.id == id),
            };
            if found.is_some() || q.more.is_none() {
                break found;
            }
            let more = q.more.take();
            load_page(&mut q, more).await;
        };
        match index {
            Some(i) => {
                q.index = i;
                play(q, start);
            }
            // Hidden from the playlist's page (or that didn't load): the
            // watch page's panel knows the playlist around the video.
            None => match fetch_panel(video.clone(), list.clone(), None).await {
                Ok(Some(p)) if !p.items.is_empty() => play(Queue::from_panel(p, &list), start),
                _ => match video {
                    Some(id) => super::ctx().watch.open(VideoItem { id, ..Default::default() }, None, start),
                    None => super::toast("Could not load the playlist"),
                },
            },
        }
    });
}

async fn load_page(q: &mut Queue, token: Option<String>) {
    let Some(token) = token else { return };
    match fetch_page(token).await {
        Ok((items, more)) => {
            q.append(items);
            q.more = more;
        }
        Err(e) => crate::log::warn(&format!("playlist page: {e}")),
    }
}

/// Plays `items` (a loaded list) from `index`, with the rest of the list
/// (also what isn't loaded yet, through `more`) as the queue.
pub fn play_list(title: &str, list_id: Option<&str>, items: &[Item], index: usize, more: Option<String>, shuffle: bool) {
    let videos = super::playlist::videos_of(items);
    let pos = match items.get(index) {
        Some(Item::Video(target)) => videos.iter().position(|v| v.id == target.id).unwrap_or(0),
        Some(other) if !shuffle => return super::cards::activate(other),
        _ => 0,
    };
    let mut q = Queue {
        title: title.to_string(),
        items: videos,
        index: pos,
        list_id: list_id.map(str::to_string),
        more,
        ..Default::default()
    };
    if !shuffle {
        return play(q, 0.0);
    }
    runtime::spawn_local(async move {
        load_rest(&mut q).await;
        q.more = None;
        q.shuffle(true);
        play(q, 0.0);
    });
}

// ---------------------------------------------------------------------------
// The panel on the watch page

struct Row {
    root: gtk::Box,
    num: gtk::Label,
    playing: gtk::Image,
    pic: gtk::Picture,
    duration: gtk::Label,
    title: gtk::Label,
    channel: gtk::Label,
    url: Rc<RefCell<String>>,
}

impl Row {
    fn new() -> Self {
        let num = gtk::Label::new(None);
        num.add_css_class("dim-label");
        num.add_css_class("caption");
        num.add_css_class("numeric");
        num.set_width_chars(3);
        let playing = gtk::Image::from_icon_name("media-playback-start-symbolic");
        playing.add_css_class("accent");
        playing.set_size_request(num.width_chars() * 8, -1);
        let pic = gtk::Picture::new();
        pic.set_content_fit(gtk::ContentFit::Cover);
        pic.set_can_shrink(true);
        let aspect = AspectBin::new(16.0 / 9.0);
        aspect.set_child(Some(&pic));
        aspect.add_css_class("thumb");
        let duration = gtk::Label::new(None);
        duration.add_css_class("badge");
        duration.set_halign(gtk::Align::End);
        duration.set_valign(gtk::Align::End);
        duration.set_margin_end(4);
        duration.set_margin_bottom(4);
        let thumb = gtk::Overlay::new();
        thumb.set_child(Some(&aspect));
        thumb.add_overlay(&duration);
        thumb.set_size_request(112, -1);
        thumb.set_valign(gtk::Align::Center);
        let title = gtk::Label::new(None);
        title.add_css_class("queue-title");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title.set_lines(2);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let channel = gtk::Label::new(None);
        channel.add_css_class("dim-label");
        channel.add_css_class("caption");
        channel.set_xalign(0.0);
        channel.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_valign(gtk::Align::Center);
        text.set_hexpand(true);
        text.append(&title);
        text.append(&channel);
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        root.add_css_class("queue-row");
        root.append(&num);
        root.append(&playing);
        root.append(&thumb);
        root.append(&text);
        Row { root, num, playing, pic, duration, title, channel, url: Rc::default() }
    }

    fn bind(&self, v: &VideoItem, pos: u32, current: u32) {
        let now = pos == current;
        self.num.set_text(&(pos + 1).to_string());
        self.num.set_visible(!now);
        self.playing.set_visible(now);
        if now {
            self.root.add_css_class("now-playing");
        } else {
            self.root.remove_css_class("now-playing");
        }
        self.title.set_text(&v.title);
        self.title.set_tooltip_text(Some(&v.title));
        self.channel.set_text(v.channel_name.as_deref().unwrap_or(""));
        self.duration.set_text(v.duration.as_deref().unwrap_or(""));
        self.duration.set_visible(v.duration.is_some());
        let url = v.thumbnail_url();
        if *self.url.borrow() == url {
            return;
        }
        self.url.replace(url.clone());
        self.pic.set_paintable(None::<&gdk::Paintable>);
        // Rows are recycled while scrolling: only set the image the row
        // still wants.
        let (pic, want) = (self.pic.downgrade(), self.url.clone());
        let u = url.clone();
        images::load(&url, move |t| {
            if let Some(p) = pic.upgrade()
                && *want.borrow() == u
            {
                p.set_paintable(Some(t));
            }
        });
    }
}

pub struct QueuePanel {
    pub widget: gtk::Box,
    title: gtk::Label,
    sub: gtk::Label,
    repeat: gtk::ToggleButton,
    shuffle: gtk::ToggleButton,
    store: gio::ListStore,
    view: gtk::ListView,
    scroller: gtk::ScrolledWindow,
    current: Rc<Cell<u32>>,
    queue: RefCell<Option<Queue>>,
    growing: Cell<bool>,
    waiters: RefCell<Vec<Waiter>>,
    /// Bumped when a different queue is set: fetches for the old one are dropped.
    generation: Cell<u64>,
    updating: Cell<bool>,
    /// Called when what's before / after the current video changes.
    pub on_changed: RefCell<Option<Box<dyn Fn()>>>,
}

impl QueuePanel {
    pub fn new() -> Rc<Self> {
        let title = gtk::Label::new(None);
        title.add_css_class("heading");
        title.set_xalign(0.0);
        title.set_wrap(true);
        title.set_lines(2);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let sub = gtk::Label::new(None);
        sub.add_css_class("dim-label");
        sub.add_css_class("caption");
        sub.set_xalign(0.0);
        sub.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        text.append(&title);
        text.append(&sub);
        let toggle = |icon: &str, tip: &str| {
            let b = gtk::ToggleButton::new();
            b.set_icon_name(icon);
            b.set_tooltip_text(Some(tip));
            b.add_css_class("flat");
            b.add_css_class("circular");
            b.set_valign(gtk::Align::Center);
            b
        };
        let repeat = toggle("media-playlist-repeat-symbolic", "Loop playlist");
        let shuffle = toggle("media-playlist-shuffle-symbolic", "Shuffle");
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Close queue"));
        close.add_css_class("flat");
        close.add_css_class("circular");
        close.set_valign(gtk::Align::Center);
        let head = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        head.append(&text);
        head.append(&repeat);
        head.append(&shuffle);
        head.append(&close);

        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let current = Rc::new(Cell::new(0u32));
        let rows: Rc<RefCell<Vec<Rc<Row>>>> = Rc::default();
        let factory = gtk::SignalListItemFactory::new();
        let r = rows.clone();
        factory.connect_setup(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let row = Rc::new(Row::new());
            li.set_child(Some(&row.root));
            r.borrow_mut().push(row);
        });
        let (r, cur) = (rows.clone(), current.clone());
        factory.connect_bind(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            let Some(child) = li.child() else { return };
            let Some(row) = r.borrow().iter().find(|row| row.root.upcast_ref::<gtk::Widget>() == &child).cloned() else { return };
            let Some(obj) = li.item().and_downcast::<glib::BoxedAnyObject>() else { return };
            row.bind(&obj.borrow::<VideoItem>(), li.position(), cur.get());
        });
        let r = rows.clone();
        factory.connect_teardown(move |_, li| {
            let Some(li) = li.downcast_ref::<gtk::ListItem>() else { return };
            if let Some(child) = li.child() {
                r.borrow_mut().retain(|row| row.root.upcast_ref::<gtk::Widget>() != &child);
            }
        });
        let view = gtk::ListView::new(Some(gtk::NoSelection::new(Some(store.clone()))), Some(factory));
        view.add_css_class("queue-list");
        view.set_single_click_activate(true);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .max_content_height(440)
            .propagate_natural_height(true)
            .child(&view)
            .build();

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        widget.add_css_class("queue-card");
        widget.append(&head);
        widget.append(&scroller);
        widget.set_visible(false);

        let this = Rc::new(Self {
            widget,
            title,
            sub,
            repeat,
            shuffle,
            store,
            view,
            scroller,
            current,
            queue: RefCell::new(None),
            growing: Cell::new(false),
            waiters: RefCell::default(),
            generation: Cell::new(0),
            updating: Cell::new(false),
            on_changed: RefCell::new(None),
        });

        let w = Rc::downgrade(&this);
        this.view.connect_activate(move |_, pos| {
            let Some(t) = w.upgrade() else { return };
            if let Some(mut q) = t.get() {
                q.index = pos as usize;
                play(q, 0.0);
            }
        });
        let w = Rc::downgrade(&this);
        this.scroller.vadjustment().connect_value_changed(move |adj| {
            if let Some(t) = w.upgrade()
                && adj.upper() > adj.page_size()
                && adj.value() + adj.page_size() > adj.upper() - 200.0
            {
                t.grow(None);
            }
        });
        let w = Rc::downgrade(&this);
        this.repeat.connect_toggled(move |b| {
            let Some(t) = w.upgrade() else { return };
            if t.updating.get() {
                return;
            }
            if let Some(q) = t.queue.borrow_mut().as_mut() {
                q.repeat = b.is_active();
            }
            t.changed();
        });
        let w = Rc::downgrade(&this);
        this.shuffle.connect_toggled(move |b| {
            let Some(t) = w.upgrade() else { return };
            if !t.updating.get() {
                t.set_shuffled(b.is_active());
            }
        });
        let w = Rc::downgrade(&this);
        close.connect_clicked(move |_| {
            if let Some(t) = w.upgrade() {
                t.set(None);
            }
        });
        this
    }

    pub fn get(&self) -> Option<Queue> {
        self.queue.borrow().clone()
    }

    pub fn is_active(&self) -> bool {
        self.queue.borrow().is_some()
    }

    pub fn has_previous(&self) -> bool {
        self.queue.borrow().as_ref().is_some_and(|q| q.index > 0 || (q.repeat && !q.is_mix()))
    }

    fn changed(&self) {
        if let Some(f) = self.on_changed.borrow().as_ref() {
            f();
        }
    }

    pub fn set(self: &Rc<Self>, queue: Option<Queue>) {
        let same = match (&*self.queue.borrow(), &queue) {
            (Some(a), Some(b)) => a.items.len() == b.items.len() && a.items.iter().zip(&b.items).all(|(x, y)| x.id == y.id),
            _ => false,
        };
        if !same {
            self.generation.set(self.generation.get() + 1);
        }
        let old = self.current.get();
        let n = self.store.n_items();
        match &queue {
            None => self.store.remove_all(),
            Some(q) => {
                self.current.set(q.index as u32);
                if same {
                    // Only the highlighted rows change. (Fresh objects: the
                    // list view doesn't re-bind a row for the same one.)
                    for i in [old, q.index as u32] {
                        if let Some(v) = self.store.item(i).and_downcast::<glib::BoxedAnyObject>() {
                            let v = v.borrow::<VideoItem>().clone();
                            self.store.splice(i, 1, &[glib::BoxedAnyObject::new(v)]);
                        }
                    }
                } else {
                    let objs: Vec<glib::BoxedAnyObject> = q.items.iter().cloned().map(glib::BoxedAnyObject::new).collect();
                    self.store.splice(0, n, &objs);
                }
            }
        }
        self.widget.set_visible(queue.is_some());
        self.queue.replace(queue);
        self.update_header();
        self.scroll_to_current();
        self.changed();
        let grow = self.queue.borrow().as_ref().is_some_and(|q| q.can_grow() && q.index + 5 >= q.items.len());
        if grow {
            self.grow(None);
        }
    }

    fn update_header(&self) {
        let q = self.queue.borrow();
        let Some(q) = q.as_ref() else { return };
        let title = if q.title.is_empty() { "Queue" } else { &q.title };
        self.title.set_text(title);
        self.title.set_tooltip_text(Some(title));
        let pos = if q.is_mix() {
            "Mix".to_string()
        } else {
            format!("{} / {}", q.index + 1, q.total.unwrap_or(0).max(q.items.len()))
        };
        let sub: Vec<&str> = [q.owner.as_deref(), Some(pos.as_str())].into_iter().flatten().collect();
        self.sub.set_text(&sub.join(" • "));
        self.updating.set(true);
        self.repeat.set_active(q.repeat);
        self.shuffle.set_active(q.unshuffled.is_some());
        self.updating.set(false);
        // Mixes are endless and already random.
        self.repeat.set_visible(!q.is_mix());
        self.shuffle.set_visible(!q.is_mix());
    }

    /// Puts the playing video near the top of the list.
    fn scroll_to_current(self: &Rc<Self>) {
        let n = self.store.n_items();
        let i = self.current.get();
        if i >= n {
            return;
        }
        self.view.scroll_to(i, gtk::ListScrollFlags::NONE, None);
        let w = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(t) = w.upgrade() else { return };
            let adj = t.scroller.vadjustment();
            let n = t.store.n_items().max(1) as f64;
            let row = adj.upper() / n;
            if row > 0.0 {
                let top = (t.current.get() as f64 - 1.0).max(0.0) * row;
                adj.set_value(top.min(adj.upper() - adj.page_size()).max(0.0));
            }
        });
    }

    fn set_shuffled(self: &Rc<Self>, on: bool) {
        let Some(mut q) = self.get() else { return };
        if !on {
            q.unshuffle();
            return self.set(Some(q));
        }
        if q.more.is_none() {
            q.shuffle(false);
            return self.set(Some(q));
        }
        // Shuffle all of a long playlist, not just what's loaded.
        self.shuffle.set_sensitive(false);
        let generation = self.generation.get();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            load_rest(&mut q).await;
            let Some(t) = w.upgrade() else { return };
            t.shuffle.set_sensitive(true);
            if t.generation.get() != generation {
                return;
            }
            // Playback may have moved on meanwhile.
            if let Some(now) = t.get() {
                q.index = now.index;
                q.repeat = now.repeat;
            }
            q.more = None;
            q.shuffle(false);
            t.set(Some(q));
        });
    }

    /// Loads more of the playlist or mix; `then` runs once that's done.
    fn grow(self: &Rc<Self>, then: Option<Waiter>) {
        if let Some(f) = then {
            self.waiters.borrow_mut().push(f);
        }
        if self.growing.get() {
            return;
        }
        let Some(q) = self.get().filter(Queue::can_grow) else {
            for f in self.waiters.take() {
                f(self);
            }
            return;
        };
        self.growing.set(true);
        let generation = self.generation.get();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let fetched = if q.is_mix() {
                let last = q.items.last().map(|v| v.id.clone());
                let list = q.list_id.clone().unwrap_or_default();
                fetch_panel(last, list, Some(q.items.len().saturating_sub(1)))
                    .await
                    .map(|p| (p.map(|p| p.items).unwrap_or_default(), None))
            } else {
                fetch_page(q.more.clone().unwrap_or_default()).await
            };
            let Some(t) = w.upgrade() else { return };
            t.growing.set(false);
            if t.generation.get() == generation {
                let added = {
                    let mut cur = t.queue.borrow_mut();
                    match (cur.as_mut(), fetched) {
                        (Some(q), Ok((items, more))) => {
                            let added = q.append(items);
                            if q.is_mix() {
                                q.mix_done = added == 0;
                            } else {
                                q.more = more;
                            }
                            q.items[q.items.len() - added..].to_vec()
                        }
                        (Some(q), Err(e)) => {
                            crate::log::warn(&format!("queue: {e}"));
                            // Don't keep retrying a failing list.
                            q.more = None;
                            q.mix_done = true;
                            Vec::new()
                        }
                        (None, _) => Vec::new(),
                    }
                };
                if !added.is_empty() {
                    let objs: Vec<glib::BoxedAnyObject> = added.into_iter().map(glib::BoxedAnyObject::new).collect();
                    t.store.splice(t.store.n_items(), 0, &objs);
                }
                t.update_header();
                t.changed();
            }
            for f in t.waiters.take() {
                f(&t);
            }
        });
    }

    /// The queue moved to its next video (None: it's over).
    pub fn next(self: &Rc<Self>, f: impl FnOnce(Option<Queue>) + 'static) {
        let Some(mut q) = self.get() else { return f(None) };
        let next = q.index + 1;
        if next < q.items.len() {
            q.index = next;
            return f(Some(q));
        }
        if q.can_grow() {
            self.grow(Some(Box::new(move |t: &Rc<QueuePanel>| {
                let q = t.get().and_then(|mut q| {
                    if next < q.items.len() {
                        q.index = next;
                        Some(q)
                    } else if q.repeat && !q.items.is_empty() {
                        q.index = 0;
                        Some(q)
                    } else {
                        None
                    }
                });
                f(q);
            })));
            return;
        }
        if q.repeat && !q.is_mix() && !q.items.is_empty() {
            q.index = 0;
            return f(Some(q));
        }
        f(None)
    }

    pub fn previous(&self) -> Option<Queue> {
        let mut q = self.get()?;
        if q.index > 0 {
            q.index -= 1;
        } else if q.repeat && !q.is_mix() && !q.items.is_empty() && q.more.is_none() {
            q.index = q.items.len() - 1;
        } else {
            return None;
        }
        Some(q)
    }

    /// "Play next" / "Add to queue". Without a queue, one is started with
    /// `playing` (the video that is playing).
    pub fn add(self: &Rc<Self>, v: VideoItem, next: bool, playing: Option<VideoItem>) {
        let mut q = self.get().unwrap_or_else(|| Queue { items: playing.into_iter().collect(), ..Default::default() });
        let at = if next { (q.index + 1).min(q.items.len()) } else { q.items.len() };
        if let Some(u) = &mut q.unshuffled {
            u.push(v.clone());
        }
        q.items.insert(at, v);
        self.set(Some(q));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(ids: &[&str]) -> Vec<VideoItem> {
        ids.iter().map(|i| VideoItem { id: i.to_string(), ..Default::default() }).collect()
    }

    fn ids(q: &Queue) -> Vec<&str> {
        q.items.iter().map(|v| v.id.as_str()).collect()
    }

    #[test]
    fn shuffle_keeps_current_first_and_restores() {
        let mut q = Queue { items: items(&["a", "b", "c", "d", "e", "f"]), index: 3, ..Default::default() };
        q.shuffle(false);
        assert_eq!((q.index, q.items[0].id.as_str()), (0, "d"));
        let mut sorted = ids(&q);
        sorted.sort();
        assert_eq!(sorted, ["a", "b", "c", "d", "e", "f"]);
        q.index = 2;
        let id = q.items[2].id.clone();
        q.unshuffle();
        assert_eq!(ids(&q), ["a", "b", "c", "d", "e", "f"]);
        assert_eq!(q.items[q.index].id, id);
    }

    #[test]
    fn mixes_skip_repeats() {
        let mut q = Queue { items: items(&["a", "b"]), list_id: Some("RDa".into()), ..Default::default() };
        assert_eq!(q.append(items(&["b", "c", "a", "d"])), 2);
        assert_eq!(ids(&q), ["a", "b", "c", "d"]);
        let mut p = Queue { items: items(&["a"]), list_id: Some("PLx".into()), ..Default::default() };
        assert_eq!(p.append(items(&["a"])), 1);
    }

    #[test]
    fn counts_videos() {
        assert_eq!(video_count(&["1,234 videos".into(), "5 views".into()]), Some(1234));
        assert_eq!(video_count(&["No views".into()]), None);
    }
}
