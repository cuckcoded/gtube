//! YouTube Music mode: its own sidebar (Home, Explore, Library and your
//! playlists), search, pages, play queue, player bar and "Now playing",
//! sharing the window's player.

mod ambient;
mod bar;
mod immersive;
mod lyrics;
mod now_playing;
mod page;
mod queue;
pub mod spectrum;
mod widgets;

use crate::api::music::{self as m, Entry, Suggestion, Target};
use crate::api::{self, models::Account, parse};
use crate::ui::player::Player;
use crate::{images, runtime};
use adw::prelude::*;
use gtk::{gdk, glib};
use page::{PageView, Source};
use queue::{Change, MusicQueue};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

pub fn open(t: &Target) {
    crate::ui::ctx().music.open(t);
}

pub fn play(t: &Target) {
    crate::ui::ctx().music.queue.play(t.clone());
}

/// Starts the visualizer's audio analysis (checked once per run).
fn init_visualizer() {
    spectrum::init(|| crate::ui::ctx().player.update_audio_filter());
}

pub fn queue() -> Rc<MusicQueue> {
    crate::ui::ctx().music.queue.clone()
}

/// The "YouTube | Music" switch at the top of both sidebars.
pub fn mode_switch() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    b.add_css_class("linked");
    b.add_css_class("mode-switch");
    b.set_homogeneous(true);
    b.set_margin_start(10);
    b.set_margin_end(10);
    b.set_margin_top(4);
    b.set_margin_bottom(8);
    for (target, label, icon) in [("video", "YouTube", "gtube-logo-symbolic"), ("music", "Music", "gtube-music-symbolic")] {
        let content = adw::ButtonContent::builder().icon_name(icon).label(label).build();
        let t = gtk::ToggleButton::builder().child(&content).build();
        t.set_action_name(Some("win.mode"));
        t.set_action_target(Some(&target));
        b.append(&t);
    }
    b
}

const NAV: &[(&str, &str, &str)] =
    &[("home", "Home", "go-home-symbolic"), ("explore", "Explore", "gtube-explore-symbolic"), ("library", "Library", "gtube-library-symbolic")];

const LIBRARY: &[(&str, &str)] = &[
    ("FEmusic_liked_playlists", "Playlists"),
    ("FEmusic_liked_videos", "Songs"),
    ("FEmusic_liked_albums", "Albums"),
    ("FEmusic_library_corpus_track_artists", "Artists"),
    ("FEmusic_library_corpus_artists", "Subscriptions"),
    ("FEmusic_history", "History"),
];

fn nav_row(icon: &str, label: &str, id: &str) -> gtk::ListBoxRow {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    b.append(&gtk::Image::from_icon_name(icon));
    let l = gtk::Label::new(Some(label));
    l.set_xalign(0.0);
    b.append(&l);
    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&b));
    row.set_widget_name(id);
    row
}

pub struct MusicShell {
    pub split: adw::OverlaySplitView,
    toolbar: adw::ToolbarView,
    pub nav: adw::NavigationView,
    content: gtk::Stack,
    back: gtk::Button,
    refresh: gtk::Button,
    search_entry: gtk::SearchEntry,
    suggest: gtk::Popover,
    suggest_list: gtk::ListBox,
    suggestions: RefCell<Vec<Suggestion>>,
    suggest_gen: Cell<u64>,
    sidebar: gtk::ListBox,
    playlists: gtk::ListBox,
    playlist_targets: RefCell<Vec<Target>>,
    pub account_button: gtk::MenuButton,
    account_avatar: adw::Avatar,
    side_avatar: adw::Avatar,
    side_name: gtk::Label,
    home: Rc<PageView>,
    home_page: adw::NavigationPage,
    explore: Rc<PageView>,
    explore_page: adw::NavigationPage,
    library: Rc<PageView>,
    library_page: adw::NavigationPage,
    search_view: Rc<PageView>,
    search_page: adw::NavigationPage,
    pushed: RefCell<Vec<(glib::WeakRef<adw::NavigationPage>, Weak<PageView>)>>,
    pub queue: Rc<MusicQueue>,
    pub bar: Rc<bar::PlayerBar>,
    pub now: Rc<now_playing::NowPlaying>,
    pub immersive: Rc<immersive::Immersive>,
    lyrics: Rc<lyrics::LyricsSource>,
    pub ambient: Rc<ambient::Ambient>,
    selecting: Cell<bool>,
    active: Cell<bool>,
    last_item: RefCell<String>,
}

impl MusicShell {
    pub fn new(player: &Rc<Player>) -> Rc<Self> {
        let queue = MusicQueue::new();
        let bar = bar::PlayerBar::new(&queue);
        let now = now_playing::NowPlaying::new(&queue);
        player.add_dock(&now.dock);
        // Added after "Now playing": while both are on screen, the player
        // goes to the immersive view.
        let immersive = immersive::Immersive::new(&queue);
        player.add_dock(&immersive.dock);
        let lyrics = lyrics::LyricsSource::new();
        lyrics.add_view(&now.lyrics);
        lyrics.add_view(&immersive.lyrics);

        // --- Pages
        let root_page = |title: &str, tag: &str, view: &Rc<PageView>| {
            adw::NavigationPage::builder().title(title).tag(tag).child(&view.widget).build()
        };
        let home = PageView::new();
        home.set_source(Source::browse("FEmusic_home"));
        let home_page = root_page("Home", "home", &home);
        let explore = PageView::new();
        explore.set_source(Source::browse("FEmusic_explore"));
        let explore_page = root_page("Explore", "explore", &explore);
        let search_view = PageView::new();
        search_view.show_title.set(false);
        let search_page = root_page("Search", "search", &search_view);

        let library = PageView::new();
        library.needs_login.set(true);
        library.show_title.set(false);
        library.set_source(Source::browse(LIBRARY[0].0));
        let lib_chips = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let mut group: Option<gtk::ToggleButton> = None;
        for (id, label) in LIBRARY {
            let t = gtk::ToggleButton::with_label(label);
            t.add_css_class("chip");
            match &group {
                Some(g) => t.set_group(Some(g)),
                None => {
                    t.set_active(true);
                    group = Some(t.clone());
                }
            }
            let lib = Rc::downgrade(&library);
            t.connect_toggled(move |t| {
                if t.is_active()
                    && let Some(l) = lib.upgrade()
                {
                    l.set_source(Source::browse(id));
                    l.load();
                }
            });
            lib_chips.append(&t);
        }
        let lib_chip_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .child(&lib_chips)
            .build();
        let lib_clamp = adw::Clamp::builder().maximum_size(1500).tightening_threshold(1200).child(&lib_chip_scroll).build();
        lib_clamp.set_margin_start(24);
        lib_clamp.set_margin_end(24);
        lib_clamp.set_margin_top(18);
        let lib_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let lib_title = gtk::Label::new(Some("Library"));
        lib_title.add_css_class("title-1");
        lib_title.set_xalign(0.0);
        let lib_head = gtk::Box::new(gtk::Orientation::Vertical, 12);
        lib_head.append(&lib_title);
        lib_head.append(&lib_clamp);
        lib_clamp.set_margin_start(0);
        lib_clamp.set_margin_end(0);
        lib_clamp.set_margin_top(0);
        let lib_head_clamp = adw::Clamp::builder().maximum_size(1500).tightening_threshold(1200).child(&lib_head).build();
        lib_head_clamp.set_margin_start(24);
        lib_head_clamp.set_margin_end(24);
        lib_head_clamp.set_margin_top(18);
        lib_box.append(&lib_head_clamp);
        lib_box.append(&library.widget);
        let library_page = adw::NavigationPage::builder().title("Library").tag("library").child(&lib_box).build();

        let nav = adw::NavigationView::new();
        nav.add(&home_page);

        let content = gtk::Stack::new();
        content.set_transition_duration(250);
        content.add_named(&nav, Some("browse"));
        content.add_named(&now.widget, Some("now"));

        // --- Header
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search songs, albums, artists, podcasts")
            .hexpand(true)
            .search_delay(180)
            .build();
        let search_clamp = adw::Clamp::builder().maximum_size(640).child(&search_entry).build();
        let suggest_list = gtk::ListBox::new();
        suggest_list.add_css_class("navigation-sidebar");
        let suggest = gtk::Popover::builder()
            .child(&suggest_list)
            .autohide(false)
            .has_arrow(false)
            .position(gtk::PositionType::Bottom)
            .halign(gtk::Align::Start)
            .build();
        suggest.set_parent(&search_entry);
        let pop = suggest.clone();
        search_entry.connect_destroy(move |_| pop.unparent());
        suggest.add_css_class("suggestions");

        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Back"));
        back.set_visible(false);
        let sidebar_toggle = gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").tooltip_text("Toggle sidebar").active(true).build();
        let account_avatar = adw::Avatar::new(28, None, false);
        let account_button = gtk::MenuButton::builder().child(&account_avatar).tooltip_text("Account").build();
        account_button.add_css_class("flat");
        account_button.add_css_class("circular");
        let header = adw::HeaderBar::new();
        header.pack_start(&sidebar_toggle);
        header.pack_start(&back);
        header.set_title_widget(Some(&search_clamp));
        header.pack_end(&account_button);
        let refresh = crate::ui::window::refresh_button();
        header.pack_end(&refresh);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_css_class("music-content");
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&content));
        toolbar.add_bottom_bar(&bar.widget);
        toolbar.set_bottom_bar_style(adw::ToolbarStyle::RaisedBorder);
        toolbar.set_reveal_bottom_bars(false);

        // --- Sidebar
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        for (id, label, icon) in NAV {
            sidebar.append(&nav_row(icon, label, id));
        }
        let playlists = gtk::ListBox::new();
        playlists.add_css_class("navigation-sidebar");
        playlists.set_selection_mode(gtk::SelectionMode::None);
        let pl_title = gtk::Label::new(Some("Playlists"));
        pl_title.add_css_class("heading");
        pl_title.add_css_class("dim-label");
        pl_title.set_xalign(0.0);
        pl_title.set_margin_start(18);
        pl_title.set_margin_top(12);
        let side_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side_content.append(&mode_switch());
        side_content.append(&sidebar);
        side_content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        side_content.append(&pl_title);
        side_content.append(&playlists);
        let side_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&side_content).build();

        let side_avatar = adw::Avatar::new(32, None, true);
        let side_name = gtk::Label::new(Some("Sign in"));
        side_name.set_xalign(0.0);
        side_name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        side_name.set_hexpand(true);
        let side_user = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        side_user.append(&side_avatar);
        side_user.append(&side_name);
        let side_user_btn = gtk::Button::builder().child(&side_user).build();
        side_user_btn.add_css_class("flat");
        let settings_btn = gtk::Button::from_icon_name("preferences-system-symbolic");
        settings_btn.set_tooltip_text(Some("Settings"));
        settings_btn.add_css_class("flat");
        settings_btn.set_action_name(Some("win.settings"));
        let side_bottom = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        side_bottom.set_margin_start(8);
        side_bottom.set_margin_end(8);
        side_bottom.set_margin_bottom(8);
        side_bottom.append(&side_user_btn);
        side_bottom.append(&settings_btn);

        let logo = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let logo_icon = gtk::Image::from_icon_name("gtube-music-symbolic");
        logo_icon.add_css_class("logo");
        logo.append(&logo_icon);
        let logo_label = gtk::Label::new(Some("GTube Music"));
        logo_label.add_css_class("heading");
        logo.append(&logo_label);
        let side_header = adw::HeaderBar::new();
        side_header.set_title_widget(Some(&logo));
        let side_toolbar = adw::ToolbarView::new();
        side_toolbar.add_top_bar(&side_header);
        side_toolbar.set_content(Some(&side_scroll));
        side_toolbar.add_bottom_bar(&side_bottom);

        let split = adw::OverlaySplitView::builder()
            .sidebar(&side_toolbar)
            .content(&toolbar)
            .min_sidebar_width(200.0)
            .max_sidebar_width(250.0)
            .build();
        split.add_css_class("music");
        split.bind_property("show-sidebar", &sidebar_toggle, "active").bidirectional().sync_create().build();
        let ambient = ambient::Ambient::new(&split);
        ambient.add_target(&immersive.widget);
        if crate::config::settings().music_visualizer {
            init_visualizer();
        }

        let this = Rc::new(Self {
            split,
            toolbar,
            nav,
            content,
            back,
            refresh,
            search_entry,
            suggest,
            suggest_list,
            suggestions: RefCell::new(Vec::new()),
            suggest_gen: Cell::new(0),
            sidebar,
            playlists,
            playlist_targets: RefCell::new(Vec::new()),
            account_button,
            account_avatar,
            side_avatar,
            side_name,
            home,
            home_page,
            explore,
            explore_page,
            library,
            library_page,
            search_view,
            search_page,
            pushed: RefCell::new(Vec::new()),
            queue,
            bar,
            now,
            immersive,
            lyrics,
            ambient,
            selecting: Cell::new(false),
            active: Cell::new(false),
            last_item: RefCell::new(String::new()),
        });
        this.connect(side_user_btn);
        this
    }

    fn connect(self: &Rc<Self>, side_user_btn: gtk::Button) {
        let wk = Rc::downgrade(self);
        self.back.connect_clicked(move |_| {
            if let Some(s) = wk.upgrade() {
                if s.now.visible.get() {
                    s.show_now(false);
                } else {
                    s.nav.pop();
                }
            }
        });
        let wk = Rc::downgrade(self);
        self.nav.connect_visible_page_notify(move |_| {
            if let Some(s) = wk.upgrade() {
                s.update_back();
            }
        });
        let wk = Rc::downgrade(self);
        self.sidebar.connect_row_selected(move |_, row| {
            let (Some(s), Some(row)) = (wk.upgrade(), row) else { return };
            if !s.selecting.get() {
                s.go(row.widget_name().as_str());
            }
        });
        let wk = Rc::downgrade(self);
        self.playlists.connect_row_activated(move |_, row| {
            let Some(s) = wk.upgrade() else { return };
            let t = usize::try_from(row.index()).ok().and_then(|i| s.playlist_targets.borrow().get(i).cloned());
            if let Some(t) = t {
                s.open(&t);
                if s.split.is_collapsed() {
                    s.split.set_show_sidebar(false);
                }
            }
        });
        side_user_btn.connect_clicked(|_| {
            if api::client().is_logged_in() {
                crate::ui::ctx().music.go("library");
            } else {
                crate::ui::login::show();
            }
        });

        // Search
        let wk = Rc::downgrade(self);
        self.search_entry.connect_activate(move |e| {
            if let Some(s) = wk.upgrade() {
                s.suggest.popdown();
                s.search(&e.text());
            }
        });
        let wk = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |e| {
            if let Some(s) = wk.upgrade() {
                s.update_suggestions(e.text().to_string());
            }
        });
        let wk = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(s) = wk.upgrade() {
                s.suggest.popdown();
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let wk = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(s) = wk.upgrade() {
                let pop = s.suggest.clone();
                glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || pop.popdown());
            }
        });
        self.search_entry.add_controller(focus);
        let wk = Rc::downgrade(self);
        self.suggest_list.connect_row_activated(move |_, row| {
            let Some(s) = wk.upgrade() else { return };
            let item = usize::try_from(row.index()).ok().and_then(|i| s.suggestions.borrow().get(i).cloned());
            s.suggest_gen.set(s.suggest_gen.get() + 1);
            s.suggest.popdown();
            match item {
                Some(Suggestion::Query(q)) => {
                    s.search_entry.set_text(&q);
                    s.search(&q);
                }
                Some(Suggestion::Entry(e)) => widgets::activate_entry(&e),
                None => {}
            }
        });

        // Queue → bar, "Now playing"
        let wk = Rc::downgrade(self);
        self.queue.connect(move |c| {
            let Some(s) = wk.upgrade() else { return };
            match c {
                Change::Queue => {
                    s.now.update_queue(&s.queue);
                    s.immersive.update_queue(&s.queue);
                    s.update_track();
                }
                Change::Loading => {
                    s.bar.update_state(&s.queue);
                    s.immersive.update_state(&s.queue);
                }
                Change::Current => {
                    s.now.update_current(&s.queue);
                    s.immersive.update_current(&s.queue);
                    s.update_track();
                    s.update_lyrics();
                }
                Change::Info => {
                    s.bar.update_info(&s.queue.info());
                    s.now.load_tab(&s.queue);
                    s.update_lyrics();
                }
                Change::Modes => s.bar.update_modes(&s.queue),
            }
        });
        self.immersive.on_exit.replace(Some(Box::new(|| crate::ui::ctx().music.set_immersive(false))));
        let wk = Rc::downgrade(self);
        self.bar.immersive.connect_clicked(move |_| {
            if let Some(s) = wk.upgrade() {
                s.set_immersive(true);
            }
        });
        let wk = Rc::downgrade(self);
        self.bar.on_toggle_now.replace(Some(Box::new(move || {
            if let Some(s) = wk.upgrade() {
                s.toggle_now();
            }
        })));
    }

    fn update_back(&self) {
        let can_pop = self.nav.visible_page().and_then(|p| self.nav.previous_page(&p)).is_some();
        self.back.set_visible(can_pop || self.now.visible.get());
        self.refresh.set_visible(!self.now.visible.get() && self.visible_view().is_some());
    }

    /// Music mode was shown or hidden.
    pub fn set_active(self: &Rc<Self>, on: bool) {
        self.active.set(on);
        if !on {
            self.suggest.popdown();
            self.set_immersive(false);
            return;
        }
        if let Some(v) = self.visible_view() {
            v.ensure_loaded();
        }
        if self.sidebar.selected_row().is_none() && self.nav.visible_page().as_ref() == Some(&self.home_page) {
            self.go("home");
        }
        self.on_player_state();
    }

    fn visible_view(&self) -> Option<Rc<PageView>> {
        let page = self.nav.visible_page()?;
        if page == self.home_page {
            return Some(self.home.clone());
        }
        if page == self.explore_page {
            return Some(self.explore.clone());
        }
        if page == self.library_page {
            return Some(self.library.clone());
        }
        if page == self.search_page {
            return Some(self.search_view.clone());
        }
        self.pushed.borrow().iter().find(|(p, _)| p.upgrade().as_ref() == Some(&page)).and_then(|(_, v)| v.upgrade())
    }

    pub fn refresh(self: &Rc<Self>) {
        if let Some(v) = self.visible_view() {
            v.load();
        }
    }

    pub fn focus_search(self: &Rc<Self>) {
        self.show_now_internal(false);
        self.search_entry.grab_focus();
    }

    /// Switches to a top-level section.
    pub fn go(self: &Rc<Self>, id: &str) {
        let (page, view) = match id {
            "home" => (self.home_page.clone(), self.home.clone()),
            "explore" => (self.explore_page.clone(), self.explore.clone()),
            "library" => (self.library_page.clone(), self.library.clone()),
            _ => return,
        };
        self.show_now(false);
        self.nav.replace(&[page]);
        view.ensure_loaded();
        self.selecting.set(true);
        let mut child = self.sidebar.first_child();
        while let Some(c) = child {
            if let Ok(row) = c.clone().downcast::<gtk::ListBoxRow>()
                && row.widget_name() == id
            {
                self.sidebar.select_row(Some(&row));
            }
            child = c.next_sibling();
        }
        self.selecting.set(false);
        if self.split.is_collapsed() {
            self.split.set_show_sidebar(false);
        }
    }

    pub fn open(self: &Rc<Self>, t: &Target) {
        match t {
            Target::Browse { id, params } => match id.as_str() {
                "FEmusic_home" if params.is_none() => self.go("home"),
                "FEmusic_explore" => self.go("explore"),
                "FEmusic_library_landing" => self.go("library"),
                _ => self.push_page(Source::Browse { id: id.clone(), params: params.clone() }),
            },
            Target::Watch { .. } => self.queue.play(t.clone()),
            Target::Search { query, params } => self.search_with(query, params.clone()),
        }
    }

    fn push_page(self: &Rc<Self>, src: Source) {
        self.show_now(false);
        let view = PageView::new();
        view.set_source(src);
        let page = adw::NavigationPage::builder().title("Music").child(&view.widget).build();
        let pw = page.downgrade();
        view.on_loaded.replace(Some(Box::new(move |p: &m::Page| {
            if let Some(page) = pw.upgrade()
                && let Some(t) = &p.title
            {
                page.set_title(t);
            }
        })));
        crate::ui::keep_alive(&page, view.clone());
        let mut pushed = self.pushed.borrow_mut();
        pushed.retain(|(p, _)| p.upgrade().is_some());
        pushed.push((page.downgrade(), Rc::downgrade(&view)));
        drop(pushed);
        self.nav.push(&page);
        view.load();
    }

    /// Searches, or opens the link if the text is a YouTube (Music) link.
    pub fn search(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(t) = m::parse_music_url(text) {
            self.open(&t);
            return;
        }
        if let Some(r) = parse::parse_youtube_url(text) {
            use crate::api::models::Resolved;
            match r {
                Resolved::Video { id, playlist, .. } => {
                    self.queue.play(Target::Watch { video_id: Some(id), playlist_id: playlist, params: None, index: None });
                    return;
                }
                Resolved::Playlist(id) => {
                    self.open(&Target::browse(&format!("VL{id}")));
                    return;
                }
                Resolved::Channel(id) if id.starts_with("UC") => {
                    self.open(&Target::browse(&id));
                    return;
                }
                _ => {}
            }
        }
        self.search_with(text, None);
    }

    fn search_with(self: &Rc<Self>, query: &str, params: Option<String>) {
        if self.search_entry.text() != query {
            self.search_entry.set_text(query);
        }
        self.suggest_gen.set(self.suggest_gen.get() + 1);
        self.suggest.popdown();
        self.show_now(false);
        self.search_page.set_title(&format!("“{query}”"));
        // Unfiltered search is what clicking the selected filter returns to.
        self.search_view.set_source(Source::Search { query: query.to_string(), params: None });
        if params.is_some() {
            self.search_view.set_current(Source::Search { query: query.to_string(), params });
        }
        self.search_view.load();
        if self.nav.visible_page().as_ref() != Some(&self.search_page) {
            let stack = self.nav.navigation_stack();
            let in_stack = (0..stack.n_items()).any(|i| stack.item(i).and_downcast::<adw::NavigationPage>().as_ref() == Some(&self.search_page));
            if in_stack {
                self.nav.pop_to_page(&self.search_page);
            } else {
                self.nav.push(&self.search_page);
            }
        }
        self.selecting.set(true);
        self.sidebar.unselect_all();
        self.selecting.set(false);
    }

    fn update_suggestions(self: &Rc<Self>, text: String) {
        let generation = self.suggest_gen.get() + 1;
        self.suggest_gen.set(generation);
        let text = text.trim().to_string();
        if text.is_empty() || text.contains("://") || text.starts_with("music.youtube.com") || !self.search_entry.has_focus() {
            self.suggest.popdown();
            return;
        }
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let q = text.clone();
            let res = runtime::run(async move { api::client().music_suggestions(&q).await }).await;
            let Some(s) = wk.upgrade() else { return };
            if s.suggest_gen.get() != generation {
                return;
            }
            let list: Vec<Suggestion> = res.unwrap_or_default().into_iter().take(12).collect();
            s.suggest_list.remove_all();
            for item in &list {
                s.suggest_list.append(&suggestion_row(item));
            }
            s.suggestions.replace(list);
            if s.suggestions.borrow().is_empty() {
                s.suggest.popdown();
            } else {
                s.suggest.set_size_request(s.search_entry.width(), -1);
                s.suggest.popup();
            }
        });
    }

    fn show_now_internal(self: &Rc<Self>, on: bool) {
        if on == self.now.visible.get() {
            return;
        }
        let tr = if on { gtk::StackTransitionType::SlideUp } else { gtk::StackTransitionType::SlideDown };
        self.content.set_visible_child_full(if on { "now" } else { "browse" }, tr);
        self.now.set_visible(on, &self.queue);
        self.update_lyrics();
        self.bar.set_now_open(on);
        if on {
            self.toolbar.add_css_class("now-open");
        } else {
            self.toolbar.remove_css_class("now-open");
        }
        self.update_back();
    }

    pub fn show_now(self: &Rc<Self>, on: bool) {
        if on && crate::ui::ctx().player.current().is_none() {
            return;
        }
        self.show_now_internal(on);
    }

    pub fn now_visible(&self) -> bool {
        self.now.visible.get()
    }

    pub fn toggle_now(self: &Rc<Self>) {
        self.show_now(!self.now.visible.get());
    }

    /// The player's position, pause, loading or volume changed.
    pub fn on_player_state(self: &Rc<Self>) {
        let p = crate::ui::ctx().player.clone();
        let current = p.current_id().unwrap_or_default();
        self.toolbar.set_reveal_bottom_bars(!current.is_empty());
        if current.is_empty() && self.now.visible.get() {
            self.show_now_internal(false);
        }
        if current.is_empty() {
            self.set_immersive(false);
        }
        self.bar.update_state(&self.queue);
        self.immersive.update_state(&self.queue);
        if *self.last_item.borrow() != current {
            self.last_item.replace(current);
            self.update_track();
            self.now.update_current(&self.queue);
            self.immersive.update_current(&self.queue);
            self.update_lyrics();
        }
        self.lyrics.set_position(p.position());
        if p.is_music_owned() && p.duration() > 0.0 && !p.is_loading() {
            self.queue.playing_ok();
        }
    }

    /// The immersive view: the playing track across the whole screen.
    pub fn set_immersive(self: &Rc<Self>, on: bool) {
        let c = crate::ui::ctx();
        if on == self.immersive.active.get() || (on && c.player.current().is_none()) {
            return;
        }
        self.immersive.set_active(on, &self.queue);
        self.update_lyrics();
        c.player.set_immersive(on);
        if on {
            self.immersive.update_track(&self.queue);
            self.immersive.update_state(&self.queue);
            c.window.fullscreen();
        } else if c.window.is_fullscreen() {
            // Don't let the player cover the window until it has left
            // fullscreen.
            c.player.set_fullscreen(false);
            c.window.unfullscreen();
        }
    }

    pub fn is_immersive(&self) -> bool {
        self.immersive.active.get()
    }

    /// Loads the lyrics of the track that is playing when they are on
    /// screen ("Now playing"'s tab or the immersive view's panel).
    pub fn update_lyrics(self: &Rc<Self>) {
        if self.now.lyrics_shown() || self.immersive.lyrics_shown() {
            self.lyrics.load(&self.queue);
        }
    }

    /// The lyrics settings changed: look again.
    pub fn lyrics_source_changed(self: &Rc<Self>) {
        self.lyrics.reset();
        self.update_lyrics();
    }

    fn update_track(&self) {
        self.bar.update_track(&self.queue);
        self.immersive.update_track(&self.queue);
        let url = self.bar.art_url.borrow().clone();
        self.ambient.set_art(url.as_deref());
    }

    /// The "Ambient color" / "Spectrum visualizer" settings changed.
    pub fn appearance_changed(&self) {
        self.ambient.apply();
        if crate::config::settings().music_visualizer {
            init_visualizer();
        }
        crate::ui::ctx().player.update_audio_filter();
        self.bar.spectrum.update_ticking();
        self.immersive.spectrum.update_ticking();
    }

    /// The player's Song / Video choice changed elsewhere ("A").
    pub fn sync_video_switch(&self, video: bool) {
        self.now.sync_video_switch(video);
        self.immersive.sync_video_switch(video);
    }

    pub fn set_account(&self, acc: Option<&Account>) {
        let logged_in = api::client().is_logged_in();
        match acc {
            Some(a) => {
                self.account_avatar.set_text(Some(&a.name));
                self.account_avatar.set_show_initials(true);
                images::avatar(&self.account_avatar, a.avatar.as_deref());
                self.side_avatar.set_text(Some(&a.name));
                images::avatar(&self.side_avatar, a.avatar.as_deref());
                self.side_name.set_text(&a.name);
            }
            None => {
                self.account_avatar.set_custom_image(None::<&gdk::Paintable>);
                self.account_avatar.set_text(None);
                self.account_avatar.set_show_initials(false);
                self.side_avatar.set_custom_image(None::<&gdk::Paintable>);
                self.side_avatar.set_text(None);
                self.side_name.set_text(if logged_in { "Signed in" } else { "Sign in" });
            }
        }
    }

    /// Signed in or out: reload what depends on the session.
    pub fn start_session(self: &Rc<Self>) {
        for v in [&self.home, &self.explore, &self.library, &self.search_view] {
            v.invalidate();
        }
        if self.active.get()
            && let Some(v) = self.visible_view()
        {
            v.load();
        }
        self.load_playlists();
    }

    fn load_playlists(self: &Rc<Self>) {
        self.playlists.remove_all();
        self.playlist_targets.borrow_mut().clear();
        if !api::client().is_logged_in() {
            let l = gtk::Label::new(Some("Sign in to see your playlists"));
            l.add_css_class("dim-label");
            l.add_css_class("caption");
            l.set_wrap(true);
            l.set_xalign(0.0);
            let row = gtk::ListBoxRow::builder().child(&l).activatable(false).build();
            self.playlists.append(&row);
            return;
        }
        let wk = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move { api::client().music_browse("FEmusic_liked_playlists", None).await.map(|v| m::parse_page(&v)) }).await;
            let Some(s) = wk.upgrade() else { return };
            let mut items: Vec<(String, String, Target)> = Vec::new();
            if let Ok(page) = res {
                for e in page.shelves.iter().flat_map(|sh| sh.entries.iter()) {
                    if let Entry::Card(c) = e
                        && let Some(t @ Target::Browse { .. }) = &c.target
                    {
                        items.push((c.title.clone(), c.subtitle_text(), t.clone()));
                    }
                }
            }
            if !items.iter().any(|(_, _, t)| matches!(t, Target::Browse { id, .. } if id == "VLLM")) {
                items.insert(0, ("Liked music".into(), "Auto playlist".into(), Target::browse("VLLM")));
            }
            s.playlists.remove_all();
            for (title, sub, _) in &items {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 0);
                b.append(&widgets::label(title, &[]));
                if !sub.is_empty() {
                    b.append(&widgets::label(sub, &["dim-label", "caption"]));
                }
                s.playlists.append(&b);
            }
            s.playlist_targets.replace(items.into_iter().map(|(_, _, t)| t).collect());
        });
    }
}

fn suggestion_row(s: &Suggestion) -> gtk::Widget {
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    b.set_margin_start(4);
    b.set_margin_end(4);
    match s {
        Suggestion::Query(q) => {
            b.append(&gtk::Image::from_icon_name("system-search-symbolic"));
            b.append(&widgets::label(q, &[]));
        }
        Suggestion::Entry(e) => {
            let (title, sub, thumb, shape) = match e {
                Entry::Track(t) => (t.title.clone(), t.artist_names(), t.thumbnail.clone(), m::Shape::Square),
                Entry::Card(c) => (c.title.clone(), c.subtitle_text(), c.thumbnail.clone(), if c.shape == m::Shape::Round { m::Shape::Round } else { m::Shape::Square }),
                Entry::Mood(md) => (md.title.clone(), String::new(), None, m::Shape::Square),
            };
            b.append(&widgets::art(thumb.as_deref(), 36, shape));
            let texts = gtk::Box::new(gtk::Orientation::Vertical, 0);
            texts.set_valign(gtk::Align::Center);
            texts.append(&widgets::label(&title, &[]));
            if !sub.is_empty() {
                texts.append(&widgets::label(&sub, &["dim-label", "caption"]));
            }
            b.append(&texts);
        }
    }
    b.upcast()
}
