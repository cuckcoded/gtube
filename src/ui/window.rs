//! Main window: sidebar, header bar with search, navigation, keyboard
//! shortcuts and fullscreen handling.

use super::cards::Layout;
use super::feed::{FeedView, Source};
use super::library::Library;
use super::music::MusicShell;
use super::player::{Mode, Player};
use super::watch::WatchPage;
use crate::api::{self, models::Account, parse};
use crate::{config, images, runtime};
use adw::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub struct Shell {
    pub window: adw::ApplicationWindow,
    split: adw::OverlaySplitView,
    /// "video" (YouTube) or "music" (YouTube Music).
    mode_stack: gtk::Stack,
    music: Rc<MusicShell>,
    toolbar: adw::ToolbarView,
    pub nav: adw::NavigationView,
    search_entry: gtk::SearchEntry,
    suggest: gtk::Popover,
    suggest_list: gtk::ListBox,
    suggest_gen: Cell<u64>,
    sidebar: gtk::ListBox,
    account_avatar: adw::Avatar,
    account_menu: gio::Menu,
    side_avatar: adw::Avatar,
    side_name: gtk::Label,
    home: Rc<FeedView>,
    home_page: adw::NavigationPage,
    subs: Rc<FeedView>,
    subs_page: adw::NavigationPage,
    pub library: Rc<Library>,
    downloads_page: adw::NavigationPage,
    search_feed: Rc<FeedView>,
    search_page: adw::NavigationPage,
    search_query: RefCell<String>,
    search_filter: RefCell<Option<String>>,
    account: RefCell<Option<Account>>,
    sidebar_before_fs: Cell<bool>,
    /// While compact: the window's size and maximized state to go back to.
    compact_saved: Cell<Option<(i32, i32, bool)>>,
    selecting: Cell<bool>,
}

const NAV_ITEMS: &[(&str, &str, &str)] = &[
    ("home", "Home", "go-home-symbolic"),
    ("subs", "Subscriptions", "gtube-subscriptions-symbolic"),
    ("-", "", ""),
    ("you", "You", "avatar-default-symbolic"),
    ("history", "History", "document-open-recent-symbolic"),
    ("playlists", "Playlists", "view-list-symbolic"),
    ("later", "Watch later", "alarm-symbolic"),
    ("liked", "Liked videos", "gtube-like-symbolic"),
    ("channels", "Channels", "gtube-subscriptions-symbolic"),
    ("-", "", ""),
    ("downloads", "Downloads", "folder-download-symbolic"),
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

impl Shell {
    pub fn new(
        app: &adw::Application,
        player: &Rc<Player>,
        watch: &Rc<WatchPage>,
        downloads: &adw::NavigationPage,
        music: &Rc<MusicShell>,
    ) -> (Rc<Self>, adw::ToastOverlay) {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("GTube")
            .default_width(1440)
            .default_height(900)
            .width_request(360)
            .height_request(400)
            .icon_name("io.github.gtube")
            .build();

        // --- Pages
        let home = FeedView::new(Source::Browse { id: "FEwhat_to_watch".into(), params: None }, Layout::Grid);
        home.set_empty_state(
            "system-search-symbolic",
            "Your feed is empty",
            "Search for something to watch, or sign in to see your recommendations.",
        );
        let home_page = adw::NavigationPage::builder().title("Home").tag("home").child(&home.widget).build();
        let subs = FeedView::new(Source::Browse { id: "FEsubscriptions".into(), params: None }, Layout::Grid);
        subs.needs_login.set(true);
        subs.set_empty_state("gtube-subscriptions-symbolic", "No new videos", "Videos from channels you subscribe to show up here.");
        let subs_page = adw::NavigationPage::builder().title("Subscriptions").tag("subs").child(&subs.widget).build();
        let library = Library::new();

        let search_feed = FeedView::new(Source::Empty, Layout::Grid);
        search_feed.set_empty_state("system-search-symbolic", "No results", "Try different keywords or remove filters.");
        let filters = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let search_page = adw::NavigationPage::builder().title("Search").tag("search").child(&search_feed.widget).build();
        search_feed.header.append(&filters);

        let nav = adw::NavigationView::new();
        nav.add(&home_page);

        // --- Header
        let search_entry = gtk::SearchEntry::builder()
            .placeholder_text("Search YouTube or paste a link")
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
        // The entry doesn't know about this child; detach it ourselves.
        let pop = suggest.clone();
        search_entry.connect_destroy(move |_| pop.unparent());
        suggest.add_css_class("suggestions");

        let back = gtk::Button::from_icon_name("go-previous-symbolic");
        back.set_tooltip_text(Some("Back"));
        back.set_visible(false);
        let sidebar_toggle = gtk::ToggleButton::builder().icon_name("sidebar-show-symbolic").tooltip_text("Toggle sidebar").active(true).build();

        let account_menu = gio::Menu::new();
        let account_avatar = adw::Avatar::new(28, None, false);
        let account_btn = gtk::MenuButton::builder().child(&account_avatar).menu_model(&account_menu).tooltip_text("Account").build();
        account_btn.add_css_class("flat");
        account_btn.add_css_class("circular");

        let refresh = refresh_button();

        let header = adw::HeaderBar::new();
        header.pack_start(&sidebar_toggle);
        header.pack_start(&back);
        header.set_title_widget(Some(&search_clamp));
        header.pack_end(&account_btn);
        header.pack_end(&refresh);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&nav));


        // --- Sidebar
        let sidebar = gtk::ListBox::new();
        sidebar.add_css_class("navigation-sidebar");
        for (id, label, icon) in NAV_ITEMS {
            if *id == "-" {
                let row = gtk::ListBoxRow::new();
                row.set_child(Some(&gtk::Separator::new(gtk::Orientation::Horizontal)));
                row.set_selectable(false);
                row.set_activatable(false);
                row.add_css_class("separator-row");
                sidebar.append(&row);
            } else {
                sidebar.append(&nav_row(icon, label, id));
            }
        }
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
        let side_bottom = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        side_bottom.set_margin_start(8);
        side_bottom.set_margin_end(8);
        side_bottom.set_margin_bottom(8);
        side_bottom.append(&side_user_btn);
        side_bottom.append(&settings_btn);
        let side_content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        side_content.append(&super::music::mode_switch());
        side_content.append(&sidebar);
        let side_scroll = gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).vexpand(true).child(&side_content).build();
        let side_header = adw::HeaderBar::builder().show_title(true).build();
        let logo = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let logo_icon = gtk::Image::from_icon_name("gtube-logo-symbolic");
        logo_icon.add_css_class("logo");
        logo.append(&logo_icon);
        let logo_label = gtk::Label::new(Some("GTube"));
        logo_label.add_css_class("heading");
        logo.append(&logo_label);
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
        split.bind_property("show-sidebar", &sidebar_toggle, "active").bidirectional().sync_create().build();

        // YouTube and YouTube Music side by side; the player floats above
        // both in this overlay.
        let mode_stack = gtk::Stack::new();
        mode_stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        mode_stack.add_named(&split, Some("video"));
        mode_stack.add_named(&music.split, Some("music"));
        let root_overlay = gtk::Overlay::new();
        root_overlay.set_child(Some(&mode_stack));
        // Below the player, which shows the music video inside it.
        root_overlay.add_overlay(&music.immersive.widget);
        player.attach(&root_overlay);
        music.account_button.set_menu_model(Some(&account_menu));

        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&root_overlay));
        window.set_content(Some(&toasts));

        let bp = adw::Breakpoint::new(adw::BreakpointCondition::parse("max-width: 860sp").unwrap());
        for sp in [&split, &music.split] {
            bp.add_setter(sp, "collapsed", Some(&true.to_value()));
            bp.add_setter(sp, "show-sidebar", Some(&false.to_value()));
        }
        for w in &music.bar.optional {
            bp.add_setter(w, "visible", Some(&false.to_value()));
        }
        window.add_breakpoint(bp);

        let this = Rc::new(Self {
            window,
            split,
            mode_stack,
            music: music.clone(),
            toolbar,
            nav,
            search_entry,
            suggest,
            suggest_list,
            suggest_gen: Cell::new(0),
            sidebar,
            account_avatar,
            account_menu,
            side_avatar,
            side_name,
            home,
            home_page,
            subs,
            subs_page,
            library,
            downloads_page: downloads.clone(),
            search_feed,
            search_page,
            search_query: RefCell::new(String::new()),
            search_filter: RefCell::new(None),
            account: RefCell::new(None),
            sidebar_before_fs: Cell::new(true),
            compact_saved: Cell::new(None),
            selecting: Cell::new(false),
        });
        this.build_filters(&filters);
        this.connect(back, refresh, side_user_btn, settings_btn, watch);
        this.update_account_menu();
        (this, toasts)
    }

    fn build_filters(self: &Rc<Self>, b: &gtk::Box) {
        let mut group: Option<gtk::ToggleButton> = None;
        for (label, params) in [("All", None), ("Videos", Some("EgIQAQ%3D%3D")), ("Channels", Some("EgIQAg%3D%3D")), ("Playlists", Some("EgIQAw%3D%3D"))] {
            let t = gtk::ToggleButton::with_label(label);
            t.add_css_class("chip");
            match &group {
                Some(g) => t.set_group(Some(g)),
                None => {
                    t.set_active(true);
                    group = Some(t.clone());
                }
            }
            let w = Rc::downgrade(self);
            t.connect_toggled(move |t| {
                if !t.is_active() {
                    return;
                }
                if let Some(s) = w.upgrade() {
                    s.search_filter.replace(params.map(str::to_string));
                    let q = s.search_query.borrow().clone();
                    if !q.is_empty() {
                        s.search_feed.set_source(Source::Search { query: q, params: params.map(str::to_string) });
                    }
                }
            });
            b.append(&t);
        }
    }

    fn connect(self: &Rc<Self>, back: gtk::Button, refresh: gtk::Button, side_user_btn: gtk::Button, settings_btn: gtk::Button, watch: &Rc<WatchPage>) {
        // Back button follows the navigation stack.
        let nav = self.nav.clone();
        back.connect_clicked(move |_| {
            nav.pop();
        });
        let b = back.clone();
        let w = Rc::downgrade(self);
        refresh.set_visible(self.can_refresh());
        self.nav.connect_visible_page_notify(move |nav| {
            let can_pop = nav.visible_page().and_then(|p| nav.previous_page(&p)).is_some();
            b.set_visible(can_pop);
            if let Some(s) = w.upgrade() {
                refresh.set_visible(s.can_refresh());
            }
        });

        // Sidebar navigation
        let w = Rc::downgrade(self);
        self.sidebar.connect_row_selected(move |_, row| {
            let (Some(s), Some(row)) = (w.upgrade(), row) else { return };
            if s.selecting.get() {
                return;
            }
            s.go(row.widget_name().as_str());
        });

        side_user_btn.connect_clicked(|_| {
            if api::client().is_logged_in() {
                super::ctx().shell.go("you");
            } else {
                super::login::show();
            }
        });
        settings_btn.connect_clicked(|_| super::settings::show());

        // Search
        let w = Rc::downgrade(self);
        self.search_entry.connect_activate(move |e| {
            if let Some(s) = w.upgrade() {
                s.suggest.popdown();
                let text = e.text().to_string();
                s.search(&text);
            }
        });
        let w = Rc::downgrade(self);
        self.search_entry.connect_search_changed(move |e| {
            if let Some(s) = w.upgrade() {
                s.update_suggestions(e.text().to_string());
            }
        });
        let w = Rc::downgrade(self);
        self.search_entry.connect_stop_search(move |_| {
            if let Some(s) = w.upgrade() {
                s.suggest.popdown();
            }
        });
        let focus = gtk::EventControllerFocus::new();
        let w = Rc::downgrade(self);
        focus.connect_leave(move |_| {
            if let Some(s) = w.upgrade() {
                let pop = s.suggest.clone();
                // Let a click on a suggestion land first.
                glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || pop.popdown());
            }
        });
        self.search_entry.add_controller(focus);
        let w = Rc::downgrade(self);
        self.suggest_list.connect_row_activated(move |_, row| {
            if let Some(s) = w.upgrade()
                && let Some(l) = row.child().and_downcast::<gtk::Label>()
            {
                let q = l.text().to_string();
                s.suggest_gen.set(s.suggest_gen.get() + 1);
                s.search_entry.set_text(&q);
                s.suggest.popdown();
                s.search(&q);
            }
        });

        // Fullscreen: hide chrome so the player can cover the whole window.
        let w = Rc::downgrade(self);
        self.window.connect_fullscreened_notify(move |win| {
            let Some(s) = w.upgrade() else { return };
            let fs = win.is_fullscreen();
            if fs {
                s.sidebar_before_fs.set(s.split.shows_sidebar());
                s.split.set_show_sidebar(false);
            } else {
                s.split.set_show_sidebar(s.sidebar_before_fs.get() && !s.split.is_collapsed());
            }
            s.toolbar.set_reveal_top_bars(!fs);
            super::ctx().player.set_fullscreen(fs);
            // Leaving fullscreen (also from the compositor) leaves the
            // immersive view.
            if !fs {
                s.music.set_immersive(false);
            }
        });

        // Keyboard shortcuts
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        let w = Rc::downgrade(self);
        key.connect_key_pressed(move |_, keyval, _, state| {
            let Some(s) = w.upgrade() else { return glib::Propagation::Proceed };
            s.on_key(keyval, state)
        });
        self.window.add_controller(key);

        // Window actions (menus, shortcuts; also reachable over D-Bus as
        // org.gtk.Actions on the window object).
        let win = self.window.clone();
        let add = |name: &str, f: fn()| {
            let a = gio::SimpleAction::new(name, None);
            a.connect_activate(move |_, _| f());
            win.add_action(&a);
        };
        add("settings", || super::settings::show());
        add("signin", || super::login::show());
        add("signout", || super::login::sign_out());
        add("downloads", || super::ctx().shell.go("downloads"));
        add("you", || super::ctx().shell.go("you"));
        add("about", about);
        add("refresh", || super::ctx().shell.refresh_current());
        add("back", || {
            super::ctx().nav.pop();
        });
        add("miniplayer", || {
            let p = super::ctx().player.clone();
            match p.mode() {
                Mode::Mini => p.expand(),
                Mode::Docked | Mode::Fullscreen => p.minimize(),
                Mode::Hidden | Mode::Compact => {}
            }
        });
        add("compact", || {
            let s = super::ctx().shell.clone();
            s.set_compact(!s.is_compact());
        });
        add("sb-mark", super::sponsorblock::mark);
        add("immersive", || {
            let c = super::ctx();
            if c.player.current_id().is_none() {
                return;
            }
            if !c.shell.is_music() {
                c.shell.set_mode("music");
            }
            c.music.set_immersive(!c.music.is_immersive());
        });
        add("audio-only", || {
            let p = super::ctx().player.clone();
            p.set_audio_only(!p.is_audio_only());
        });
        add("now-playing", || {
            let c = super::ctx();
            if c.shell.is_music() {
                c.music.toggle_now();
            } else if c.player.is_music_owned() {
                c.shell.set_mode("music");
                c.music.show_now(true);
            }
        });
        add("toggle-mode", || {
            let s = super::ctx().shell.clone();
            s.set_mode(if s.is_music() { "video" } else { "music" });
        });
        // Next / previous in whichever queue is playing.
        add("next", || {
            let c = super::ctx();
            if c.player.is_music_owned() {
                c.music.queue.next();
            } else if c.player.current_id().is_some() {
                c.watch.next();
            }
        });
        add("previous", || {
            let c = super::ctx();
            if c.player.is_music_owned() {
                c.music.queue.previous();
            } else if c.player.current_id().is_some() {
                c.watch.previous();
            }
        });
        // `mode::video` / `mode::music`
        let mode = gio::SimpleAction::new_stateful("mode", Some(glib::VariantTy::STRING), &"video".to_variant());
        mode.connect_change_state(|_, v| {
            if let Some(m) = v.and_then(|v| v.str()) {
                super::ctx().shell.set_mode(m);
            }
        });
        self.window.add_action(&mode);
        add("fullscreen", || super::ctx().player.toggle_fullscreen());
        add("play-pause", || super::ctx().player.toggle_pause());
        // `go::home`, `go::subs`, `go::history`, …
        let go = gio::SimpleAction::new("go", Some(glib::VariantTy::STRING));
        go.connect_activate(|_, p| {
            if let Some(id) = p.and_then(|p| p.str()) {
                let c = super::ctx();
                match id {
                    "explore" | "library" => {
                        c.shell.set_mode("music");
                        c.music.go(id);
                    }
                    "home" if c.shell.is_music() => c.music.go(id),
                    _ => {
                        if c.shell.is_music() {
                            c.shell.set_mode("video");
                        }
                        c.shell.go(id);
                    }
                }
            }
        });
        self.window.add_action(&go);
        // `open::<link or search>`
        let open = gio::SimpleAction::new("open", Some(glib::VariantTy::STRING));
        open.connect_activate(|_, p| {
            if let Some(q) = p.and_then(|p| p.str()) {
                super::search(q);
            }
        });
        self.window.add_action(&open);
        if let Some(app) = self.window.application() {
            app.set_accels_for_action("win.toggle-mode", &["<Control>m"]);
            app.set_accels_for_action("win.compact", &["<Control><Shift>m"]);
            app.set_accels_for_action("win.next", &["<Shift>n"]);
            app.set_accels_for_action("win.previous", &["<Shift>p"]);
            app.set_accels_for_action("win.settings", &["<Control>comma"]);
            app.set_accels_for_action("win.back", &["<Alt>Left"]);
            app.set_accels_for_action("win.go::home", &["<Alt>1"]);
            app.set_accels_for_action("win.go::subs", &["<Alt>2"]);
            app.set_accels_for_action("win.go::you", &["<Alt>3"]);
            app.set_accels_for_action("win.go::downloads", &["<Alt>4"]);
        }

        // Ask before quitting with running downloads.
        let wd = Rc::downgrade(watch);
        self.window.connect_close_request(move |win| {
            let _ = &wd;
            let ctx = super::ctx();
            let n = ctx.downloads.active();
            if n == 0 {
                return glib::Propagation::Proceed;
            }
            let d = adw::AlertDialog::new(
                Some("Downloads in progress"),
                Some(&format!("{n} download(s) are still running. Quit and cancel them?")),
            );
            d.add_response("cancel", "Keep running");
            d.add_response("quit", "Quit");
            d.set_response_appearance("quit", adw::ResponseAppearance::Destructive);
            let win2 = win.clone();
            d.connect_response(Some("quit"), move |_, _| {
                super::ctx().downloads.cancel_all();
                win2.destroy();
            });
            d.present(Some(win));
            glib::Propagation::Stop
        });
    }

    pub fn is_compact(&self) -> bool {
        self.compact_saved.get().is_some()
    }

    /// Shrinks the window to a small player only (or restores it). The
    /// compositor decides where it goes; GTK can't keep it on top.
    pub fn set_compact(&self, on: bool) {
        let p = super::ctx().player.clone();
        if on == self.is_compact() || (on && p.current_id().is_none()) {
            return;
        }
        let win = &self.window;
        if on {
            if win.is_fullscreen() {
                win.unfullscreen();
            }
            self.music.set_immersive(false);
            self.compact_saved.set(Some((win.width(), win.height(), win.is_maximized())));
            let s = config::settings();
            // Only the player is left to take space.
            self.mode_stack.set_visible(false);
            win.set_size_request(200, 112);
            win.unmaximize();
            win.set_default_size(s.compact_width.max(200), s.compact_height.max(112));
            win.add_css_class("compact");
            p.set_compact(true);
        } else {
            let Some((w, h, max)) = self.compact_saved.take() else { return };
            let (cw, ch) = (win.width(), win.height());
            if !win.is_fullscreen() && cw > 0 && ch > 0 {
                config::update(|c| {
                    c.compact_width = cw;
                    c.compact_height = ch;
                });
            }
            if win.is_fullscreen() {
                win.unfullscreen();
            }
            p.set_compact(false);
            win.remove_css_class("compact");
            self.mode_stack.set_visible(true);
            win.set_size_request(360, 400);
            win.set_default_size(w, h);
            if max {
                win.maximize();
            }
        }
    }

    fn on_key(&self, keyval: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use glib::Propagation::{Proceed, Stop};
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let typing = gtk::prelude::GtkWindowExt::focus(&self.window)
            .is_some_and(|f| f.is::<gtk::Text>() || f.ancestor(gtk::Text::static_type()).is_some() || f.is::<gtk::TextView>());
        let music = self.is_music();
        if (ctrl && (keyval == gdk::Key::l || keyval == gdk::Key::f)) || (!typing && keyval == gdk::Key::slash) {
            if music {
                self.music.focus_search();
            } else {
                self.search_entry.grab_focus();
            }
            return Stop;
        }
        if ctrl && keyval == gdk::Key::r {
            self.refresh_current();
            return Stop;
        }
        if typing || ctrl || state.contains(gdk::ModifierType::ALT_MASK) {
            return Proceed;
        }
        let p = super::ctx().player.clone();
        if p.current_id().is_none() {
            return Proceed;
        }
        if music {
            let docked = matches!(p.mode(), Mode::Docked | Mode::Fullscreen);
            match keyval {
                gdk::Key::space | gdk::Key::k => p.toggle_pause(),
                gdk::Key::m => p.toggle_mute(),
                gdk::Key::j => p.seek_relative(-10.0),
                gdk::Key::l => p.seek_relative(10.0),
                gdk::Key::Left if self.music.now_visible() => p.seek_relative(-5.0),
                gdk::Key::Right if self.music.now_visible() => p.seek_relative(5.0),
                gdk::Key::f if !self.is_compact() => self.music.set_immersive(!self.music.is_immersive()),
                gdk::Key::f if docked => p.toggle_fullscreen(),
                gdk::Key::Escape if self.music.is_immersive() => self.music.set_immersive(false),
                gdk::Key::Escape if p.is_fullscreen() => p.toggle_fullscreen(),
                gdk::Key::Escape if self.is_compact() => self.set_compact(false),
                gdk::Key::Escape if self.music.now_visible() => self.music.show_now(false),
                gdk::Key::a => p.set_audio_only(!p.is_audio_only()),
                gdk::Key::plus | gdk::Key::equal | gdk::Key::KP_Add => p.change_volume(5.0),
                gdk::Key::minus | gdk::Key::KP_Subtract => p.change_volume(-5.0),
                _ => return Proceed,
            }
            return Stop;
        }
        let big = matches!(p.mode(), Mode::Docked | Mode::Fullscreen);
        match keyval {
            gdk::Key::space | gdk::Key::k => p.toggle_pause(),
            gdk::Key::m => p.toggle_mute(),
            gdk::Key::f if big => p.toggle_fullscreen(),
            gdk::Key::i => {
                if p.mode() == Mode::Mini {
                    p.expand();
                } else {
                    p.minimize();
                }
            }
            gdk::Key::Escape if p.is_fullscreen() => p.toggle_fullscreen(),
            gdk::Key::Escape if self.is_compact() => self.set_compact(false),
            gdk::Key::semicolon => super::sponsorblock::mark(),
            gdk::Key::j => p.seek_relative(-10.0),
            gdk::Key::l => p.seek_relative(10.0),
            gdk::Key::Left if big => p.seek_relative(-5.0),
            gdk::Key::Right if big => p.seek_relative(5.0),
            gdk::Key::Up if p.is_fullscreen() => p.change_volume(5.0),
            gdk::Key::Down if p.is_fullscreen() => p.change_volume(-5.0),
            gdk::Key::a => p.set_audio_only(!p.is_audio_only()),
            _ => return Proceed,
        }
        Stop
    }

    fn update_suggestions(self: &Rc<Self>, text: String) {
        let generation = self.suggest_gen.get() + 1;
        self.suggest_gen.set(generation);
        let text = text.trim().to_string();
        if text.is_empty() || parse::parse_youtube_url(&text).is_some() || !self.search_entry.has_focus() {
            self.suggest.popdown();
            return;
        }
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let q = text.clone();
            let res = runtime::run(async move { api::client().suggestions(&q).await }).await;
            let Some(s) = w.upgrade() else { return };
            if s.suggest_gen.get() != generation {
                return;
            }
            let list = res.unwrap_or_default();
            s.suggest_list.remove_all();
            for item in list.iter().take(10) {
                let l = gtk::Label::new(Some(item));
                l.set_xalign(0.0);
                l.set_margin_start(6);
                l.set_margin_end(6);
                s.suggest_list.append(&l);
            }
            if list.is_empty() {
                s.suggest.popdown();
            } else {
                s.suggest.set_size_request(s.search_entry.width(), -1);
                s.suggest.popup();
            }
        });
    }

    /// Searches, or opens the link if the text is a YouTube URL.
    pub fn search(self: &Rc<Self>, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(r) = parse::parse_youtube_url(text) {
            super::open_resolved(r);
            return;
        }
        if self.search_entry.text() != text {
            self.search_entry.set_text(text);
        }
        self.suggest_gen.set(self.suggest_gen.get() + 1);
        self.suggest.popdown();
        self.search_query.replace(text.to_string());
        self.search_page.set_title(&format!("“{text}”"));
        let params = self.search_filter.borrow().clone();
        self.search_feed.set_source(Source::Search { query: text.to_string(), params });
        self.show_page(&self.search_page);
    }

    fn show_page(&self, page: &adw::NavigationPage) {
        if self.nav.visible_page().as_ref() == Some(page) {
            return;
        }
        let stack = self.nav.navigation_stack();
        let in_stack = (0..stack.n_items()).any(|i| stack.item(i).and_downcast::<adw::NavigationPage>().as_ref() == Some(page));
        if in_stack {
            self.nav.pop_to_page(page);
        } else {
            self.nav.push(page);
        }
    }

    /// Switches to a top-level section.
    pub fn go(&self, id: &str) {
        let page = match id {
            "home" => {
                self.home.ensure_loaded();
                self.home_page.clone()
            }
            "subs" => {
                self.subs.ensure_loaded();
                self.subs_page.clone()
            }
            "you" | "history" | "playlists" | "later" | "liked" | "channels" => {
                self.library.show_tab(if id == "you" { "history" } else { id });
                self.library.page.clone()
            }
            "downloads" => self.downloads_page.clone(),
            _ => return,
        };
        self.nav.replace(&[page]);
        // Reflect the choice in the sidebar without re-triggering navigation.
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

    pub fn is_music(&self) -> bool {
        self.mode_stack.visible_child_name().as_deref() == Some("music")
    }

    /// Switches between YouTube ("video") and YouTube Music ("music").
    pub fn set_mode(self: &Rc<Self>, mode: &str) {
        let music = mode == "music";
        let name = if music { "music" } else { "video" };
        if let Some(a) = self.window.lookup_action("mode").and_downcast::<gio::SimpleAction>() {
            a.set_state(&name.to_variant());
        }
        let changed = self.mode_stack.visible_child_name().as_deref() != Some(name);
        self.mode_stack.set_visible_child_name(name);
        self.window.set_title(Some(if music { "GTube Music" } else { "GTube" }));
        if config::settings().ui_mode != name {
            config::update(|s| s.ui_mode = name.into());
        }
        let player = super::ctx().player.clone();
        player.set_music_presentation(music);
        self.music.set_active(music);
        if changed {
            self.suggest.popdown();
            if !music && self.nav.visible_page().is_none() {
                self.go("home");
            }
        }
    }

    /// Whether the page on screen can be refreshed (Ctrl+R, the header's button).
    fn can_refresh(&self) -> bool {
        let Some(page) = self.nav.visible_page() else { return false };
        [&self.home_page, &self.subs_page, &self.library.page, &self.search_page].contains(&&page)
    }

    fn refresh_current(&self) {
        if self.is_music() {
            self.music.refresh();
            return;
        }
        let page = self.nav.visible_page();
        if page.as_ref() == Some(&self.home_page) {
            self.home.refresh();
        } else if page.as_ref() == Some(&self.subs_page) {
            self.subs.refresh();
        } else if page.as_ref() == Some(&self.library.page) {
            self.library.reset();
        } else if page.as_ref() == Some(&self.search_page) {
            self.search_feed.refresh();
        }
    }

    /// (Re)loads everything that depends on the session: called at startup
    /// and after signing in or out.
    pub fn start_session(self: &Rc<Self>) {
        self.home.invalidate();
        self.subs.invalidate();
        self.library.reset();
        self.set_account(None);
        self.music.start_session();
        let w = Rc::downgrade(self);
        runtime::spawn_local(async move {
            let res = runtime::run(async move {
                let c = api::client();
                c.bootstrap().await?;
                c.account().await
            })
            .await;
            let Some(s) = w.upgrade() else { return };
            match res {
                Ok(acc) => s.set_account(acc),
                Err(e) => {
                    crate::log::error(&format!("session start failed: {e:#}"));
                    super::toast(&format!("Could not reach YouTube: {e}"));
                }
            }
            let visible = s.nav.visible_page();
            if visible.as_ref() == Some(&s.home_page) || visible.is_none() {
                s.home.ensure_loaded();
            } else if visible.as_ref() == Some(&s.subs_page) {
                s.subs.ensure_loaded();
            } else if visible.as_ref() == Some(&s.library.page) {
                s.library.reset();
            }
        });
    }

    fn set_account(&self, acc: Option<Account>) {
        let logged_in = api::client().is_logged_in();
        match &acc {
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
        self.library.set_account(acc.as_ref());
        self.music.set_account(acc.as_ref());
        self.account.replace(acc);
        self.update_account_menu();
    }

    fn update_account_menu(&self) {
        let m = &self.account_menu;
        m.remove_all();
        let top = gio::Menu::new();
        if api::client().is_logged_in() {
            let name = self.account.borrow().as_ref().map(|a| a.name.clone()).unwrap_or_else(|| "Your account".into());
            top.append(Some(&name), Some("win.you"));
        } else {
            top.append(Some("Sign in…"), Some("win.signin"));
        }
        top.append(Some("Downloads"), Some("win.downloads"));
        m.append_section(None, &top);
        let modes = gio::Menu::new();
        modes.append(Some("YouTube"), Some("win.mode::video"));
        modes.append(Some("YouTube Music"), Some("win.mode::music"));
        m.append_section(None, &modes);
        let mid = gio::Menu::new();
        mid.append(Some("Refresh"), Some("win.refresh"));
        mid.append(Some("Compact Player"), Some("win.compact"));
        mid.append(Some("Settings"), Some("win.settings"));
        mid.append(Some("About GTube"), Some("win.about"));
        m.append_section(None, &mid);
        if api::client().is_logged_in() {
            let bottom = gio::Menu::new();
            bottom.append(Some("Sign out"), Some("win.signout"));
            m.append_section(None, &bottom);
        }
    }
}

fn about() {
    let d = adw::AboutDialog::builder()
        .application_name("GTube")
        .application_icon("io.github.gtube")
        .version(env!("CARGO_PKG_VERSION"))
        .comments("A native YouTube client with SponsorBlock, Return YouTube Dislike, ad blocking and downloads.")
        .license_type(gtk::License::Gpl30)
        .build();
    d.add_credit_section(
        Some("Built on"),
        &["mpv https://mpv.io", "yt-dlp https://github.com/yt-dlp/yt-dlp", "SponsorBlock https://sponsor.ajay.app", "Return YouTube Dislike https://returnyoutubedislike.com"],
    );
    d.present(Some(&super::ctx().window));
}

/// The header's refresh button (the same as Ctrl+R).
pub fn refresh_button() -> gtk::Button {
    let b = gtk::Button::from_icon_name("view-refresh-symbolic");
    b.set_tooltip_text(Some("Refresh (Ctrl+R)"));
    b.set_action_name(Some("win.refresh"));
    b
}
