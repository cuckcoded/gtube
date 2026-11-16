//! Signing in. Three ways to get a youtube.com session:
//! - a Google sign-in page inside the app (WebKitGTK),
//! - importing the session from an installed browser (via yt-dlp),
//! - importing a Netscape cookies.txt file.

use crate::api::cookies::CookieJar;
#[cfg(feature = "webkit")]
use crate::api::cookies::Cookie;
use crate::config;
use adw::prelude::*;
use gtk::{gio, glib};

pub fn show() {
    let body = format!(
        "Signing in makes your home feed, subscriptions, history and playlists the same as on youtube.com. \
         The session is stored only on this computer ({}).",
        config::cookies_path().display()
    );
    let d = adw::AlertDialog::new(Some("Sign in to YouTube"), Some(&body));
    d.add_response("cancel", "Cancel");
    d.add_response("file", "cookies.txt…");
    d.add_response("browser", "From browser…");
    #[cfg(feature = "webkit")]
    {
        d.add_response("google", "Sign in with Google");
        d.set_response_appearance("google", adw::ResponseAppearance::Suggested);
        d.set_default_response(Some("google"));
    }
    d.set_prefer_wide_layout(true);
    d.set_close_response("cancel");
    d.connect_response(None, |_, r| match r {
        #[cfg(feature = "webkit")]
        "google" => webview_login(),
        "browser" => browser_import(),
        "file" => file_import(),
        _ => {}
    });
    d.present(Some(&super::ctx().window));
}

fn only_youtube(mut jar: CookieJar) -> CookieJar {
    // Keep just what's needed for youtube.com; browsers export every site.
    jar.retain(|c| c.domain.trim_start_matches('.').ends_with("youtube.com"));
    jar
}

fn finish(jar: CookieJar) {
    let jar = only_youtube(jar);
    if !jar.has_login() {
        super::toast("No signed-in YouTube session found in those cookies");
        return;
    }
    super::apply_cookies(jar);
}

fn file_import() {
    let filter = gtk::FileFilter::new();
    filter.add_pattern("*.txt");
    filter.set_name(Some("Cookie files"));
    let filters = gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    let fd = gtk::FileDialog::builder().title("Import cookies.txt").filters(&filters).modal(true).build();
    fd.open(Some(&super::ctx().window), None::<&gio::Cancellable>, |res| {
        let Ok(file) = res else { return };
        let Some(path) = file.path() else { return };
        match std::fs::read_to_string(&path) {
            Ok(t) => finish(CookieJar::parse_netscape(&t)),
            Err(e) => super::toast(&format!("Could not read {}: {e}", path.display())),
        }
    });
}

const BROWSERS: &[(&str, &str)] = &[
    ("firefox", "Firefox"),
    ("chrome", "Google Chrome"),
    ("chromium", "Chromium"),
    ("brave", "Brave"),
    ("edge", "Microsoft Edge"),
    ("vivaldi", "Vivaldi"),
    ("opera", "Opera"),
    ("whale", "Whale"),
];

fn browser_import() {
    let d = adw::AlertDialog::new(
        Some("Import from browser"),
        Some("Uses the YouTube session of a browser where you are already signed in. yt-dlp reads the browser's cookie store; only youtube.com cookies are kept."),
    );
    d.add_response("cancel", "Cancel");
    d.add_response("import", "Import");
    d.set_response_appearance("import", adw::ResponseAppearance::Suggested);
    d.set_close_response("cancel");
    let names: Vec<&str> = BROWSERS.iter().map(|b| b.1).collect();
    let combo = adw::ComboRow::builder().title("Browser").model(&gtk::StringList::new(&names)).build();
    let profile = adw::EntryRow::builder().title("Profile (optional)").build();
    let group = adw::PreferencesGroup::new();
    group.add(&combo);
    group.add(&profile);
    d.set_extra_child(Some(&group));
    d.connect_response(None, move |_, r| {
        if r != "import" {
            return;
        }
        let mut spec = BROWSERS[combo.selected() as usize].0.to_string();
        let p = profile.text().trim().to_string();
        if !p.is_empty() {
            spec = format!("{spec}:{p}");
        }
        run_browser_import(spec);
    });
    d.present(Some(&super::ctx().window));
}

fn run_browser_import(spec: String) {
    let tmp = config::config_dir().join("import-cookies.tmp");
    let _ = std::fs::remove_file(&tmp);
    // Without a URL yt-dlp exits with an error, but it still writes the
    // cookie jar to --cookies on the way out.
    let ytdlp = crate::extract::ytdlp();
    let args = [ytdlp.as_str(), "--cookies-from-browser", spec.as_str(), "--cookies", tmp.to_str().unwrap_or_default()];
    let argv: Vec<&std::ffi::OsStr> = args.iter().map(std::ffi::OsStr::new).collect();
    let proc = match gio::Subprocess::newv(&argv, gio::SubprocessFlags::STDOUT_SILENCE | gio::SubprocessFlags::STDERR_PIPE) {
        Ok(p) => p,
        Err(e) => {
            super::toast(&format!("Could not run yt-dlp: {e}"));
            return;
        }
    };
    super::toast("Reading browser cookies…");
    glib::spawn_future_local(async move {
        let (_, err) = proc.communicate_utf8_future(None).await.unwrap_or_default();
        let text = std::fs::read_to_string(&tmp).unwrap_or_default();
        let _ = std::fs::remove_file(&tmp);
        if text.is_empty() {
            let msg = err
                .as_deref()
                .unwrap_or("")
                .lines()
                .find(|l| l.contains("ERROR") && !l.contains("provide at least one URL"))
                .unwrap_or("no cookies found")
                .trim_start_matches("ERROR: ")
                .to_string();
            super::toast(&format!("Import failed: {msg}"));
            return;
        }
        finish(CookieJar::parse_netscape(&text));
    });
}

#[cfg(feature = "webkit")]
thread_local! {
    // One WebKit session for the app's lifetime. A per-window session died
    // with the window, which the cookie callback itself closes; the finished
    // request then released the last reference to the session's cookie
    // manager and WebKit crashed (SIGSEGV on every successful sign-in).
    static WEBKIT_SESSION: std::cell::OnceCell<webkit::NetworkSession> = const { std::cell::OnceCell::new() };
}

#[cfg(feature = "webkit")]
fn webkit_session() -> webkit::NetworkSession {
    WEBKIT_SESSION.with(|cell| {
        cell.get_or_init(|| {
            let data = config::data_dir().join("webkit");
            let cache = config::cache_dir().join("webkit");
            let session = webkit::NetworkSession::new(data.to_str(), cache.to_str());
            if let Some(cm) = session.cookie_manager()
                && let Some(p) = data.join("cookies.sqlite").to_str()
            {
                cm.set_persistent_storage(p, webkit::CookiePersistentStorage::Sqlite);
            }
            session
        })
        .clone()
    })
}

#[cfg(feature = "webkit")]
fn webview_login() {
    use webkit::prelude::*;

    let session = webkit_session();
    let Some(cm) = session.cookie_manager() else {
        super::toast("WebKit cookie manager unavailable");
        return;
    };
    crate::log::info("sign-in: opening Google sign-in window");
    let view = webkit::WebView::builder().network_session(&session).vexpand(true).hexpand(true).build();
    view.load_uri(
        "https://accounts.google.com/ServiceLogin?service=youtube&passive=true&hl=en\
         &continue=https%3A%2F%2Fwww.youtube.com%2Fsignin%3Faction_handle_signin%3Dtrue%26app%3Ddesktop%26next%3D%252F",
    );

    let header = adw::HeaderBar::new();
    let done = gtk::Button::with_label("Done");
    done.add_css_class("suggested-action");
    done.set_tooltip_text(Some("Use the current session (if sign-in wasn't detected automatically)"));
    header.pack_end(&done);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&view));
    let win = adw::Window::builder()
        .title("Sign in with Google")
        .default_width(520)
        .default_height(760)
        .modal(true)
        .transient_for(&super::ctx().window)
        .content(&toolbar)
        .build();

    // Every youtube.com page load checks for a session; only the first
    // success may finish the sign-in.
    let finished = std::rc::Rc::new(std::cell::Cell::new(false));
    let check = {
        let (cm, win, finished) = (cm.clone(), win.downgrade(), finished.clone());
        move |manual: bool| {
            if finished.get() {
                return;
            }
            let (win, finished) = (win.clone(), finished.clone());
            cm.cookies("https://www.youtube.com", None::<&gio::Cancellable>, move |res| {
                let list = match res {
                    Ok(l) => l,
                    Err(e) => {
                        crate::log::warn(&format!("sign-in: reading cookies failed: {e}"));
                        return;
                    }
                };
                if finished.get() {
                    return;
                }
                let jar = soup_to_jar(list);
                if jar.has_login() {
                    finished.set(true);
                    crate::log::info("sign-in: session detected");
                    if let Some(w) = win.upgrade() {
                        w.close();
                    }
                    finish(jar);
                } else if manual {
                    super::toast("Not signed in yet — finish signing in first");
                }
            });
        }
    };
    let c = check.clone();
    view.connect_load_changed(move |v, ev| {
        if ev == webkit::LoadEvent::Finished && v.uri().is_some_and(|u| u.starts_with("https://www.youtube.com")) {
            c(false);
        }
    });
    done.connect_clicked(move |_| check(true));
    win.present();
}

#[cfg(feature = "webkit")]
fn soup_to_jar(list: Vec<webkit::soup::Cookie>) -> CookieJar {
    let cookies = list
        .into_iter()
        .filter_map(|mut c| {
            let domain = c.domain()?.to_string();
            Some(Cookie {
                include_subdomains: domain.starts_with('.'),
                domain,
                path: c.path().map(|p| p.to_string()).unwrap_or_else(|| "/".into()),
                secure: c.is_secure(),
                http_only: c.is_http_only(),
                expires: c.expires().map(|d| d.to_unix()).unwrap_or(0),
                name: c.name()?.to_string(),
                value: c.value().map(|v| v.to_string()).unwrap_or_default(),
            })
        })
        .collect();
    CookieJar::from_cookies(cookies)
}

/// Forgets the session everywhere (our cookie file and the WebKit profile).
pub fn sign_out() {
    crate::log::info("signing out");
    config::delete_cookies();
    #[cfg(feature = "webkit")]
    if let Some(session) = WEBKIT_SESSION.with(|c| c.get().cloned()) {
        // The session is open: clear it through WebKit rather than deleting
        // files underneath it.
        if let Some(dm) = session.website_data_manager() {
            dm.clear(webkit::WebsiteDataTypes::ALL, glib::TimeSpan(0), None::<&gio::Cancellable>, |r| {
                if let Err(e) = r {
                    crate::log::warn(&format!("sign-out: clearing WebKit data failed: {e}"));
                }
            });
        }
        super::apply_cookies(CookieJar::default());
        return;
    }
    let _ = std::fs::remove_dir_all(config::data_dir().join("webkit"));
    let _ = std::fs::remove_dir_all(config::cache_dir().join("webkit"));
    super::apply_cookies(CookieJar::default());
}
