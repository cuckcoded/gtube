//! Preferences dialog.

use crate::api::{extras, parse};
use crate::config::{self, SbMode};
use adw::prelude::*;
use gtk::gio;
use std::sync::atomic::Ordering;

fn switch(title: &str, subtitle: &str, active: bool, f: impl Fn(bool) + 'static) -> adw::SwitchRow {
    let r = adw::SwitchRow::builder().title(title).active(active).build();
    if !subtitle.is_empty() {
        r.set_subtitle(subtitle);
    }
    r.connect_active_notify(move |r| f(r.is_active()));
    r
}

pub fn show() {
    let s = config::settings();
    let dialog = adw::PreferencesDialog::new();
    dialog.set_title("Settings");
    // Room for all four tab labels.
    dialog.set_content_width(720);

    // --- General
    let general = adw::PreferencesPage::builder().title("General").icon_name("preferences-system-symbolic").build();
    let content = adw::PreferencesGroup::builder().title("Content").build();
    content.add(&switch(
        "Hide Shorts",
        "Shorts are removed from every feed, search and channel. Short links still open in the normal player.",
        s.hide_shorts,
        |v| config::update(|c| c.hide_shorts = v),
    ));
    let lang = adw::EntryRow::builder().title("Language (e.g. en, it, de)").text(&s.language).show_apply_button(true).build();
    lang.connect_apply(|r| {
        let v = r.text().trim().to_lowercase();
        if !v.is_empty() {
            config::update(|c| c.language = v);
            super::apply_locale();
        }
    });
    let region = adw::EntryRow::builder().title("Region (e.g. US, IT, DE)").text(&s.region).show_apply_button(true).build();
    region.connect_apply(|r| {
        let v = r.text().trim().to_uppercase();
        if !v.is_empty() {
            config::update(|c| c.region = v);
            super::apply_locale();
        }
    });
    content.add(&lang);
    content.add(&region);
    general.add(&content);

    let privacy = adw::PreferencesGroup::builder()
        .title("Ad blocking")
        .description("Playback goes straight to the media streams through yt-dlp and mpv, so video ads are never requested. Promoted results, ad slots and banners are removed from every feed.")
        .build();
    let blocked = adw::ActionRow::builder()
        .title("Removed this session")
        .subtitle(format!(
            "{} ads and promotions • {} Shorts",
            parse::ADS_BLOCKED.load(Ordering::Relaxed),
            parse::SHORTS_HIDDEN.load(Ordering::Relaxed)
        ))
        .build();
    blocked.add_prefix(&gtk::Image::from_icon_name("gtube-shield-symbolic"));
    privacy.add(&blocked);
    general.add(&privacy);

    let ryd = adw::PreferencesGroup::builder().title("Return YouTube Dislike").build();
    ryd.add(&switch(
        "Show dislikes",
        "Estimated dislike counts from returnyoutubedislikeapi.com",
        s.dislikes,
        |v| config::update(|c| c.dislikes = v),
    ));
    general.add(&ryd);

    let account = adw::PreferencesGroup::builder().title("Account").build();
    let logged_in = crate::api::client().is_logged_in();
    let acc_row = adw::ActionRow::builder()
        .title(if logged_in { "Signed in" } else { "Not signed in" })
        .subtitle(if logged_in {
            format!("Your YouTube session is stored in {}", config::cookies_path().display())
        } else {
            "Sign in to see your own feed, subscriptions and history".to_string()
        })
        .build();
    let acc_btn = gtk::Button::with_label(if logged_in { "Sign out" } else { "Sign in" });
    acc_btn.set_valign(gtk::Align::Center);
    if logged_in {
        acc_btn.add_css_class("destructive-action");
    } else {
        acc_btn.add_css_class("suggested-action");
    }
    let d = dialog.downgrade();
    acc_btn.connect_clicked(move |_| {
        if let Some(d) = d.upgrade() {
            d.close();
        }
        if crate::api::client().is_logged_in() {
            super::login::sign_out();
        } else {
            super::login::show();
        }
    });
    acc_row.add_suffix(&acc_btn);
    account.add(&acc_row);
    general.add(&account);
    dialog.add(&general);

    // --- Playback
    let playback = adw::PreferencesPage::builder().title("Playback").icon_name("media-playback-start-symbolic").build();
    let pg = adw::PreferencesGroup::new();
    let qualities = [2160u32, 1440, 1080, 720, 480, 360];
    let names: Vec<String> = qualities.iter().map(|q| format!("{q}p")).collect();
    let q = adw::ComboRow::builder()
        .title("Preferred quality")
        .model(&gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>()))
        .selected(qualities.iter().position(|q| *q == s.max_quality).unwrap_or(2) as u32)
        .build();
    q.connect_selected_notify(move |r| {
        let v = qualities[r.selected() as usize];
        config::update(|c| c.max_quality = v);
    });
    pg.add(&q);
    pg.add(&switch("Autoplay", "Play the next related video (or playlist entry) when a video ends", s.autoplay, |v| {
        config::update(|c| c.autoplay = v)
    }));
    pg.add(&switch("Audio only", "Hide video and stream audio only, also for new videos", s.audio_only, |v| {
        super::ctx().player.set_audio_only(v);
    }));
    pg.add(&switch("Hardware decoding", "Takes effect after restarting GTube", s.hwdec, |v| config::update(|c| c.hwdec = v)));
    pg.add(&switch(
        "Add to YouTube watch history",
        "Videos you watch here show up in your history on youtube.com, with how far you got (when signed in)",
        s.mark_watched,
        |v| config::update(|c| c.mark_watched = v),
    ));
    pg.add(&switch(
        "Resume where you left off",
        "Videos you watched part of, here or on another device, continue from there (when signed in)",
        s.resume_playback,
        |v| config::update(|c| c.resume_playback = v),
    ));
    playback.add(&pg);
    let mg = adw::PreferencesGroup::builder().title("YouTube Music").build();
    mg.add(&switch("Ambient color", "Tint Music mode with the colors of the album art that is playing", s.music_ambient, |v| {
        config::update(|c| c.music_ambient = v);
        super::ctx().music.appearance_changed();
    }));
    mg.add(&switch("Spectrum visualizer", "Show the music's spectrum behind the player bar", s.music_visualizer, |v| {
        config::update(|c| c.music_visualizer = v);
        super::ctx().music.appearance_changed();
    }));
    mg.add(&switch(
        "Synced lyrics from LRCLIB",
        "When YouTube Music has no timed lyrics for a song, look for them on lrclib.net (sends the song's title and artist)",
        s.lyrics_lrclib,
        |v| {
            config::update(|c| c.lyrics_lrclib = v);
            super::ctx().music.lyrics_source_changed();
        },
    ));
    playback.add(&mg);
    dialog.add(&playback);

    // --- SponsorBlock
    let sb = adw::PreferencesPage::builder().title("SponsorBlock").icon_name("gtube-shield-symbolic").build();
    let sbg = adw::PreferencesGroup::builder()
        .description("Crowd-sourced segments from sponsor.ajay.app. Lookups only send the first characters of a hash of the video ID.")
        .build();
    sbg.add(&switch("Enable SponsorBlock", "", s.sponsorblock, |v| config::update(|c| c.sponsorblock = v)));
    sb.add(&sbg);
    let cats = adw::PreferencesGroup::builder().title("Categories").build();
    let modes = ["Skip automatically", "Show in seek bar", "Off"];
    for (id, label, _) in extras::SB_CATEGORIES {
        let row = adw::ComboRow::builder().title(*label).model(&gtk::StringList::new(&modes)).build();
        if *id == "poi_highlight" {
            row.set_subtitle("“Skip” shows a Jump to highlight button");
        }
        let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        dot.set_size_request(12, 12);
        dot.set_valign(gtk::Align::Center);
        dot.add_css_class("sb-dot");
        dot.add_css_class(&format!("sb-{id}"));
        row.add_prefix(&dot);
        row.set_selected(match s.sb_mode(id) {
            SbMode::Skip => 0,
            SbMode::Show => 1,
            SbMode::Off => 2,
        });
        let id = id.to_string();
        row.connect_selected_notify(move |r| {
            let m = match r.selected() {
                0 => SbMode::Skip,
                1 => SbMode::Show,
                _ => SbMode::Off,
            };
            config::update(|c| {
                c.sb_modes.insert(id.clone(), m);
            });
        });
        cats.add(&row);
    }
    sb.add(&cats);
    dialog.add(&sb);

    // --- Downloads
    let dl = adw::PreferencesPage::builder().title("Downloads").icon_name("folder-download-symbolic").build();
    let dg = adw::PreferencesGroup::new();
    let folder = adw::ActionRow::builder().title("Download folder").subtitle(&s.download_dir).build();
    let choose = gtk::Button::from_icon_name("folder-open-symbolic");
    choose.set_valign(gtk::Align::Center);
    choose.add_css_class("flat");
    let fr = folder.downgrade();
    choose.connect_clicked(move |b| {
        let fd = gtk::FileDialog::builder().title("Choose download folder").modal(true).build();
        fd.set_initial_folder(Some(&gio::File::for_path(config::settings().download_dir)));
        let win = b.root().and_downcast::<gtk::Window>();
        let fr = fr.clone();
        fd.select_folder(win.as_ref(), None::<&gio::Cancellable>, move |res| {
            if let Ok(f) = res
                && let Some(p) = f.path()
            {
                let p = p.to_string_lossy().into_owned();
                config::update(|c| c.download_dir = p.clone());
                if let Some(r) = fr.upgrade() {
                    r.set_subtitle(&p);
                }
            }
        });
    });
    folder.add_suffix(&choose);
    dg.add(&folder);
    dg.add(&switch("Embed metadata, thumbnail and chapters", "", s.download_embed_metadata, |v| {
        config::update(|c| c.download_embed_metadata = v)
    }));
    dg.add(&switch("Cut out sponsor segments by default", "", s.download_sb_remove, |v| {
        config::update(|c| c.download_sb_remove = v)
    }));
    dl.add(&dg);
    dialog.add(&dl);

    dialog.present(Some(&super::ctx().window));
}
