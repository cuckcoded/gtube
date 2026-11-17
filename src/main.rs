// A GUI program: no console window on Windows.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod api;
mod bundle;
mod config;
mod extract;
mod images;
mod log;
mod mpv;
mod runtime;
mod ui;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

const APP_ID: &str = "io.github.gtube";

fn load_css() {
    let Some(display) = gdk::Display::default() else { return };
    let provider = gtk::CssProvider::new();
    provider.load_from_resource("/io/github/gtube/style.css");
    gtk::style_context_add_provider_for_display(&display, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    // SponsorBlock category colors (settings dots).
    let mut extra = String::new();
    for (id, _, color) in api::extras::SB_CATEGORIES {
        extra.push_str(&format!(".sb-dot.sb-{id} {{ background-color: {color}; }}\n"));
    }
    let p2 = gtk::CssProvider::new();
    p2.load_from_string(&extra);
    gtk::style_context_add_provider_for_display(&display, &p2, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    gtk::IconTheme::for_display(&display).add_resource_path("/io/github/gtube/icons");
}

fn build_ui(app: &adw::Application) {
    if let Some(w) = app.active_window() {
        w.present();
        return;
    }
    let s = config::settings();
    api::init(config::load_cookies(), s.language.clone(), s.region.clone());

    let player = ui::player::Player::new();
    let watch = ui::watch::WatchPage::new();
    player.add_dock(&watch.placeholder);
    let downloads = ui::downloads::Downloads::new();
    let music = ui::music::MusicShell::new(&player);
    let (shell, toasts) = ui::window::Shell::new(app, &player, &watch, &downloads.page, &music);
    ui::set_ctx(ui::Ctx {
        window: shell.window.clone(),
        toasts,
        nav: shell.nav.clone(),
        player: player.clone(),
        watch: watch.clone(),
        downloads,
        shell: shell.clone(),
        music: music.clone(),
    });

    // The player is shared: what it does next depends on which mode
    // started the current item.
    player.on_ended.replace(Some(Box::new(|| {
        let c = ui::ctx();
        if c.player.is_music_owned() {
            c.music.queue.on_ended();
        } else {
            c.watch.play_next();
        }
    })));
    player.on_expand.replace(Some(Box::new(|| {
        let c = ui::ctx();
        if c.player.is_music_owned() {
            c.shell.set_mode("music");
            c.music.show_now(true);
        } else {
            if c.shell.is_music() {
                c.shell.set_mode("video");
            }
            c.watch.show();
        }
    })));
    player.on_state.replace(Some(Box::new(|| {
        ui::ctx().music.on_player_state();
        ui::mpris::update();
    })));
    player.on_seeked.replace(Some(Box::new(ui::mpris::seeked)));
    player.on_theater.replace(Some(Box::new(|on| ui::ctx().watch.set_theater(on))));
    player.on_error.replace(Some(Box::new(|_| {
        let c = ui::ctx();
        if c.player.is_music_owned() {
            c.music.queue.on_error();
        }
    })));
    player.on_music_video.replace(Some(Box::new(|video| {
        let c = ui::ctx();
        c.music.sync_video_switch(video);
        c.music.queue.video_switched(video);
    })));
    player.on_minimize.replace(Some(Box::new(|| {
        let c = ui::ctx();
        if c.nav.visible_page().as_ref() == Some(&c.watch.page)
            && c.nav.previous_page(&c.watch.page).is_some()
        {
            c.nav.pop();
        }
    })));
    player.on_download.replace(Some(Box::new(|| {
        if let Some(v) = ui::ctx().player.current() {
            ui::downloads::show_dialog(&v);
        }
    })));
    player.on_segments.replace(Some(Box::new(|segs| ui::ctx().watch.set_segments(segs))));

    ui::mpris::start(app);
    shell.window.present();
    shell.go("home");
    shell.start_session();
    shell.set_mode(&s.ui_mode);
}

fn main() -> glib::ExitCode {
    log::init();
    bundle::setup();
    // GTube 0.1 kept watch positions here; YouTube keeps them now.
    let _ = std::fs::remove_file(config::data_dir().join("positions.json"));
    gio::resources_register_include!("gtube.gresource").expect("resources");
    glib::set_application_name("GTube");
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    // Registering an option also turns on GLib's --help (otherwise every
    // argument, "--help" included, would be treated as a search).
    app.add_main_option("version", glib::Char::from(b'v'), glib::OptionFlags::NONE, glib::OptionArg::None, "Show the version and exit", None);
    app.set_option_context_parameter_string(Some("[LINK | SEARCH TERMS…]"));
    app.set_option_context_summary(Some(
        "Watch YouTube. Opens a video, channel or playlist link, or searches for the given terms\n(also in an already running window).",
    ));
    app.connect_handle_local_options(|_, opts| {
        if opts.contains("version") {
            println!("GTube {}", env!("CARGO_PKG_VERSION"));
            return std::ops::ControlFlow::Break(glib::ExitCode::SUCCESS);
        }
        std::ops::ControlFlow::Continue(())
    });
    app.connect_startup(|app| {
        log::open_file();
        // Ctrl+Q closes the window the normal way, so the "downloads still
        // running" confirmation still applies.
        let quit = gio::SimpleAction::new("quit", None);
        let a = app.downgrade();
        quit.connect_activate(move |_, _| {
            if let Some(app) = a.upgrade() {
                for w in app.windows() {
                    w.close();
                }
            }
        });
        app.add_action(&quit);
        app.set_accels_for_action("app.quit", &["<Control>q"]);
        // libmpv refuses to start unless numbers are formatted the C way;
        // GTK has just set the locale from the environment.
        unsafe {
            libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
        }
        gtk::Window::set_default_icon_name(APP_ID);
        // Titles and descriptions are selectable for copying, but shouldn't
        // show up fully highlighted just because a page focused them.
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_label_select_on_focus(false);
        }
        load_css();
    });
    app.connect_activate(build_ui);
    app.connect_shutdown(|_| {
        if let Some(c) = ui::try_ctx() {
            c.player.finish_report_blocking();
        }
        ui::music::spectrum::cleanup();
    });
    // `gtube <link or search terms>` opens that link / search, also when
    // GTube is already running.
    app.connect_command_line(|app, cmd| {
        let args: Vec<String> = cmd.arguments().iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
        build_ui(app);
        let text = args.join(" ");
        if !text.trim().is_empty() {
            ui::search(&text);
        }
        glib::ExitCode::SUCCESS
    });
    app.run()
}
