//! Turns plain descriptions/comments into Pango markup with clickable links
//! and timestamps.

use gtk::glib;
use std::sync::LazyLock;

static TOKEN_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"(https?://[^\s<>"]+[^\s<>".,;:!?)\]])|(\b(?:\d{1,2}:)?\d{1,2}:\d{2}\b)|(#[\p{L}\p{N}_]+)"#).unwrap()
})
;

pub fn timestamp_seconds(s: &str) -> Option<f64> {
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + part.parse::<f64>().ok()?;
    }
    Some(total)
}

/// Byte offset where a collapsed preview of `text` should end: after at most
/// `max_lines` lines and `max_chars` characters. (`GtkLabel:lines` can't do
/// this: Pango applies it to every paragraph separately.)
pub fn excerpt_end(text: &str, max_lines: usize, max_chars: usize) -> usize {
    let text = text.trim_end();
    let mut cut = text.len();
    if max_lines > 0
        && let Some((i, _)) = text.match_indices('\n').nth(max_lines - 1)
    {
        cut = i;
    }
    if let Some((i, _)) = text.char_indices().nth(max_chars) {
        cut = cut.min(i);
    }
    cut
}

pub fn linkify(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for cap in TOKEN_RE.captures_iter(text) {
        let m = cap.get(0).unwrap();
        out.push_str(&glib::markup_escape_text(&text[last..m.start()]));
        let s = m.as_str();
        let esc = glib::markup_escape_text(s);
        if cap.get(1).is_some() {
            let shown = if s.chars().count() > 60 { format!("{}…", s.chars().take(57).collect::<String>()) } else { s.to_string() };
            out.push_str(&format!("<a href=\"{esc}\">{}</a>", glib::markup_escape_text(&shown)));
        } else if cap.get(2).is_some() {
            match timestamp_seconds(s) {
                Some(t) => out.push_str(&format!("<a href=\"seek:{t}\">{esc}</a>")),
                None => out.push_str(&esc),
            }
        } else {
            let q = urlencoding::encode(s);
            out.push_str(&format!("<a href=\"search:{q}\">{esc}</a>"));
        }
        last = m.end();
    }
    out.push_str(&glib::markup_escape_text(&text[last..]));
    out
}

/// Like [`linkify`], but first applies YouTube's own link ranges (byte
/// offsets into `text`), which cover links whose text isn't a URL.
pub fn linkify_runs(text: &str, links: &[(usize, usize, String)]) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    let mut last = 0;
    for (start, len, target) in links {
        let end = start + len;
        if *start < last || end > text.len() || !text.is_char_boundary(*start) || !text.is_char_boundary(end) {
            continue;
        }
        out.push_str(&linkify(&text[last..*start]));
        out.push_str(&format!(
            "<a href=\"{}\">{}</a>",
            glib::markup_escape_text(target),
            glib::markup_escape_text(&text[*start..end])
        ));
        last = end;
    }
    out.push_str(&linkify(&text[last..]));
    out
}

/// Shared handler for links in descriptions and comments. Returns true if
/// the link was handled inside the app.
pub fn handle_link(uri: &str) -> bool {
    if let Some(t) = uri.strip_prefix("seek:") {
        if let Ok(t) = t.parse::<f64>() {
            super::ctx().player.seek_to(t);
        }
        return true;
    }
    if let Some(q) = uri.strip_prefix("search:") {
        let q = urlencoding::decode(q).map(|c| c.into_owned()).unwrap_or_default();
        super::search(&q);
        return true;
    }
    // Links pointing back to YouTube open inside the app.
    if let Some(r) = crate::api::parse::parse_youtube_url(uri)
        && !matches!(r, crate::api::models::Resolved::Search(_))
    {
        super::open_resolved(r);
        return true;
    }
    false
}

pub fn label(markup_src: &str) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_markup(&linkify(markup_src));
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_xalign(0.0);
    l.set_selectable(true);
    l.connect_activate_link(|_, uri| if handle_link(uri) { glib::Propagation::Stop } else { glib::Propagation::Proceed });
    l
}
