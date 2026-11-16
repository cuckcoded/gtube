//! Submitting segments to SponsorBlock: mark where a segment starts and
//! ends while watching (the shield button in the player, or ";"), check it
//! in a dialog, and send it.

use super::{ctx, toast};
use crate::api::{self, extras};
use crate::{config, runtime};
use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone)]
struct Draft {
    video: String,
    start: f64,
    end: Option<f64>,
}

thread_local! {
    static DRAFT: RefCell<Option<Draft>> = const { RefCell::new(None) };
}

fn set_draft(d: Option<Draft>) {
    let p = ctx().player.clone();
    p.set_sb_draft(d.as_ref().map(|d| (d.start, d.end)));
    DRAFT.with(|x| x.replace(d));
}

/// Marks where a segment starts; the second time, where it ends, and opens
/// the submission dialog.
pub fn mark() {
    let p = ctx().player.clone();
    let Some(video) = p.current_id() else { return };
    if p.duration() <= 0.0 {
        return;
    }
    let pos = p.position();
    let draft = DRAFT.with(|d| d.borrow().clone()).filter(|d| d.video == video);
    match draft {
        None => {
            set_draft(Some(Draft { video, start: pos, end: None }));
            toast(&format!("Segment starts at {}. Mark its end the same way (;)", fmt_time(pos)));
        }
        Some(d) if d.end.is_none() => {
            let (start, end) = (d.start.min(pos), d.start.max(pos));
            set_draft(Some(Draft { video, start, end: Some(end) }));
            open_dialog();
        }
        Some(_) => open_dialog(),
    }
}

/// "1:23.4" (or "1:02:03.4").
fn fmt_time(t: f64) -> String {
    let tenths = (t.max(0.0) * 10.0).round() as u64;
    let (s, tenth) = (tenths / 10, tenths % 10);
    if s >= 3600 {
        format!("{}:{:02}:{:02}.{}", s / 3600, s / 60 % 60, s % 60, tenth)
    } else {
        format!("{}:{:02}.{}", s / 60, s % 60, tenth)
    }
}

/// "1:23.4", "83.4", "1:02:03" → seconds.
fn parse_time(text: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in text.trim().split(':') {
        let v: f64 = part.trim().parse().ok()?;
        if v.is_nan() || v < 0.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

/// The private ID SponsorBlock credits submissions to; made once.
fn user_id() -> String {
    let id = config::settings().sb_user_id;
    if id.len() >= 32 {
        return id;
    }
    let mut buf = [0u8; 24];
    let random = std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut buf));
    if random.is_err() {
        for b in buf.iter_mut() {
            *b = gtk::glib::random_int_range(0, 256) as u8;
        }
    }
    let id = hex::encode(buf);
    config::update(|c| c.sb_user_id = id.clone());
    id
}

fn time_row(title: &str, t: f64) -> adw::EntryRow {
    let row = adw::EntryRow::builder().title(title).text(fmt_time(t)).build();
    row.set_input_purpose(gtk::InputPurpose::Number);
    let now = gtk::Button::with_label("Now");
    now.set_tooltip_text(Some("Set to the current playback position"));
    now.add_css_class("flat");
    now.set_valign(gtk::Align::Center);
    let r = row.downgrade();
    now.connect_clicked(move |_| {
        if let Some(r) = r.upgrade() {
            r.set_text(&fmt_time(ctx().player.position()));
        }
    });
    row.add_suffix(&now);
    row
}

fn open_dialog() {
    let Some(draft) = DRAFT.with(|d| d.borrow().clone()) else { return };
    let player = ctx().player.clone();
    let duration = player.duration();

    let start = time_row("Start", draft.start);
    let end = time_row("End", draft.end.unwrap_or(draft.start));
    let cats: Vec<(&str, &str)> = extras::SB_CATEGORIES.iter().map(|c| (c.0, c.1)).collect();
    let names: Vec<&str> = cats.iter().map(|c| c.1).collect();
    let category = adw::ComboRow::builder().title("Category").model(&gtk::StringList::new(&names)).build();
    let action = adw::ComboRow::builder()
        .title("Action")
        .subtitle("What players do with it")
        .model(&gtk::StringList::new(&["Skip", "Mute"]))
        .build();
    let note = gtk::Label::new(None);
    note.add_css_class("dim-label");
    note.set_wrap(true);
    note.set_xalign(0.0);
    let times = adw::PreferencesGroup::new();
    times.add(&start);
    times.add(&end);
    let kind = adw::PreferencesGroup::new();
    kind.add(&category);
    kind.add(&action);

    let status = gtk::Label::new(None);
    status.add_css_class("error");
    status.set_wrap(true);
    status.set_xalign(0.0);
    status.set_visible(false);
    let preview = gtk::Button::with_label("Preview");
    preview.add_css_class("pill");
    let submit = gtk::Button::with_label("Submit");
    submit.add_css_class("pill");
    submit.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    buttons.set_halign(gtk::Align::Center);
    buttons.append(&preview);
    buttons.append(&submit);
    let about = gtk::Label::new(None);
    about.set_markup(
        "Submissions are public and help everyone who uses SponsorBlock. They're credited to an anonymous ID kept on this computer. \
         Please follow the <a href=\"https://wiki.sponsor.ajay.app/w/Guidelines\">guidelines</a>.",
    );
    about.add_css_class("dim-label");
    about.add_css_class("caption");
    about.set_wrap(true);
    about.set_xalign(0.0);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 18);
    body.set_margin_top(12);
    body.set_margin_bottom(24);
    body.set_margin_start(24);
    body.set_margin_end(24);
    for w in [times.upcast_ref::<gtk::Widget>(), kind.upcast_ref(), note.upcast_ref(), status.upcast_ref(), buttons.upcast_ref(), about.upcast_ref()] {
        body.append(w);
    }
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(&body));
    let dialog = adw::Dialog::builder().title("Submit Segment").content_width(440).child(&toolbar).build();

    // Highlights are a single point, "Exclusive access" is the whole video,
    // and only some categories can be muted instead of skipped.
    let update = {
        let (end, action, note, category, cats) = (end.clone(), action.clone(), note.clone(), category.clone(), cats.clone());
        move || {
            let cat = cats.get(category.selected() as usize).map(|c| c.0).unwrap_or("sponsor");
            end.set_visible(cat != "poi_highlight" && cat != "exclusive_access");
            action.set_visible(!matches!(cat, "poi_highlight" | "exclusive_access"));
            let text = match cat {
                "poi_highlight" => "A highlight marks the moment people came for; players offer to jump to it.",
                "exclusive_access" => "Marks the whole video as showcasing a product or service the creator got exclusive access to.",
                "sponsor" => "Paid promotion, paid referrals and direct advertisements.",
                "selfpromo" => "Unpaid or self promotion: merch, donations, shout-outs to other channels.",
                "interaction" => "A short reminder to like, subscribe or follow in the middle of the content.",
                "intro" => "An interval without actual content: a pause, a static frame, a repeating animation.",
                "outro" => "Credits or end cards.",
                "preview" => "A collection of clips that show what's coming up, or a recap of what came before.",
                "hook" => "Narrated trailers or greetings and goodbyes.",
                "music_offtopic" => "Only for music videos: a part that isn't music.",
                "filler" => "A tangent that isn't needed to understand the main content.",
                _ => "",
            };
            note.set_text(text);
        }
    };
    update();
    let u = update.clone();
    category.connect_selected_notify(move |_| u());

    let read = {
        let (start, end, category, action, cats, status) = (start.clone(), end.clone(), category.clone(), action.clone(), cats.clone(), status.clone());
        move || -> Option<extras::Segment> {
            let cat = cats.get(category.selected() as usize).map(|c| c.0).unwrap_or("sponsor").to_string();
            let fail = |msg: &str| {
                status.set_text(msg);
                status.set_visible(true);
                None
            };
            if cat == "exclusive_access" {
                return Some(extras::Segment { start: 0.0, end: 0.0, category: cat, action: "full".into() });
            }
            let Some(s) = parse_time(&start.text()) else { return fail("The start time isn't a time (like 1:23.4).") };
            if cat == "poi_highlight" {
                if s >= duration {
                    return fail("The highlight is past the end of the video.");
                }
                return Some(extras::Segment { start: s, end: s, category: cat, action: "poi".into() });
            }
            let Some(e) = parse_time(&end.text()) else { return fail("The end time isn't a time (like 1:23.4).") };
            if e - s < 0.5 {
                return fail("The segment must end after it starts.");
            }
            if e > duration + 1.0 {
                return fail("The segment ends after the video.");
            }
            let action = if action.selected() == 1 { "mute" } else { "skip" };
            Some(extras::Segment { start: s, end: e.min(duration), category: cat, action: action.into() })
        }
    };
    let read = Rc::new(read);

    let (r, st) = (read.clone(), status.clone());
    preview.connect_clicked(move |_| {
        st.set_visible(false);
        let Some(seg) = r() else { return };
        let p = ctx().player.clone();
        p.set_sb_draft(Some((seg.start, Some(seg.end))));
        p.seek_to((seg.start - 2.0).max(0.0));
        if p.is_paused() {
            p.toggle_pause();
        }
    });
    let submitted = Rc::new(std::cell::Cell::new(false));
    let (dw, done, st) = (dialog.downgrade(), submitted.clone(), status.clone());
    let video = draft.video.clone();
    submit.connect_clicked(move |b| {
        st.set_visible(false);
        let Some(seg) = read() else { return };
        b.set_sensitive(false);
        let (b, dw, done, st, video) = (b.clone(), dw.clone(), done.clone(), st.clone(), video.clone());
        runtime::spawn_local(async move {
            let uid = user_id();
            let v = video.clone();
            let res = runtime::run(async move { extras::submit_segment(api::client().http(), &v, &uid, duration, &seg).await }).await;
            b.set_sensitive(true);
            match res {
                Ok(()) => {
                    done.set(true);
                    if let Some(d) = dw.upgrade() {
                        d.close();
                    }
                    toast("Segment submitted. Thanks for helping!");
                    let p = ctx().player.clone();
                    if p.current_id().as_deref() == Some(video.as_str()) {
                        p.refresh_segments();
                    }
                }
                Err(e) => {
                    crate::log::warn(&format!("SponsorBlock submission: {e}"));
                    st.set_text(&e.to_string());
                    st.set_visible(true);
                }
            }
        });
    });
    // Closing the dialog drops the marked segment (also once it's sent).
    dialog.connect_closed(move |_| set_draft(None));
    dialog.present(Some(&ctx().window));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times() {
        assert_eq!(fmt_time(83.44), "1:23.4");
        assert_eq!(fmt_time(3723.0), "1:02:03.0");
        assert_eq!(parse_time("1:23.4"), Some(83.4));
        assert_eq!(parse_time(" 83 "), Some(83.0));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("x"), None);
        assert_eq!(parse_time("1:-3"), None);
        for t in [0.0, 59.9, 61.2, 3599.5, 4000.1] {
            assert!((parse_time(&fmt_time(t)).unwrap() - t).abs() < 0.051);
        }
    }
}
