//! Log file: `~/.cache/gtube/gtube.log` (the previous run is kept as
//! `gtube.log.1`).
//!
//! Launched from the app menu, GTube's stderr goes nowhere, so its own
//! messages, GLib/GTK/WebKit warnings and panics are also written here.
//! URL query strings are stripped (stream URLs carry your IP address) and
//! nothing from the session cookies is ever logged.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

use gtk::glib;

/// Stop writing once a single run has logged this much.
const MAX_BYTES: usize = 4 * 1024 * 1024;

struct Sink {
    file: Option<std::fs::File>,
    written: usize,
}

static SINK: Mutex<Sink> = Mutex::new(Sink { file: None, written: 0 });

pub fn path() -> PathBuf {
    crate::config::cache_dir().join("gtube.log")
}

/// Routes GLib logging and panics through this module. Call once, first
/// thing in `main` (GLib allows one log writer per process).
pub fn init() {
    glib::log_set_writer_func(|level, fields| {
        let tag = match level {
            glib::LogLevel::Error => Some("ERROR"),
            glib::LogLevel::Critical => Some("CRIT"),
            glib::LogLevel::Warning => Some("WARN"),
            glib::LogLevel::Message => Some("INFO"),
            _ => None,
        };
        if let Some(tag) = tag {
            let field = |k: &str| fields.iter().find(|f| f.key() == k).and_then(|f| f.value_str());
            let domain = field("GLIB_DOMAIN").unwrap_or("GLib");
            let msg = field("MESSAGE").unwrap_or("");
            write_file(tag, &redact(&format!("[{domain}] {msg}")));
        }
        // Still print to stderr the usual way.
        glib::log_writer_default(level, fields)
    });

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        write_file("PANIC", &format!("{info}\n{}", std::backtrace::Backtrace::force_capture()));
        default_hook(info);
    }));
}

/// Starts a new log file, keeping the previous one. Only the primary
/// instance calls this: `gtube <link>` while GTube is running hands the link
/// over and exits, and must not rotate the running instance's log away.
pub fn open_file() {
    let p = path();
    let _ = std::fs::rename(&p, p.with_extension("log.1"));
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    if let Ok(f) = opts.open(&p)
        && let Ok(mut s) = SINK.lock()
    {
        s.file = Some(f);
        s.written = 0;
    }
    info(&format!("GTube {} started", env!("CARGO_PKG_VERSION")));
}

pub fn info(msg: &str) {
    write("INFO", msg);
}

pub fn warn(msg: &str) {
    write("WARN", msg);
}

pub fn error(msg: &str) {
    write("ERROR", msg);
}

/// An error with its causes ("error sending request: …: connection refused").
pub fn chain(e: &dyn std::error::Error) -> String {
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s.push_str(": ");
        s.push_str(&c.to_string());
        src = c.source();
    }
    s
}

fn write(level: &str, msg: &str) {
    let msg = redact(msg);
    eprintln!("gtube {level}: {msg}");
    write_file(level, &msg);
}

fn write_file(level: &str, msg: &str) {
    // Formatted before taking the lock, so nothing that logs can run under it.
    let now = glib::DateTime::now_local().ok().and_then(|d| d.format("%F %T").ok()).map(|s| s.to_string()).unwrap_or_default();
    let Ok(mut s) = SINK.lock() else { return };
    if s.file.is_none() || s.written > MAX_BYTES {
        return;
    }
    let mut line = format!("{now} {level:<5} {msg}\n");
    s.written += line.len();
    if s.written > MAX_BYTES {
        line.push_str("… log size limit reached, further messages are only printed to stderr\n");
    }
    if let Some(f) = s.file.as_mut() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Strips URL query strings and fragments (signed stream URLs contain your
/// IP address and session parameters).
fn redact(msg: &str) -> String {
    static URL_QUERY: LazyLock<regex::Regex> = LazyLock::new(|| regex::Regex::new(r"(https?://[^\s?#'\x22()]+)[?#][^\s'\x22()]*").unwrap());
    URL_QUERY.replace_all(msg, "$1?…").into_owned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts_queries() {
        assert_eq!(
            super::redact("Failed to open https://rr1---sn.googlevideo.com/videoplayback?ip=1.2.3.4&sig=x now"),
            "Failed to open https://rr1---sn.googlevideo.com/videoplayback?… now"
        );
        assert_eq!(super::redact("plain https://example.com/a b"), "plain https://example.com/a b");
        assert_eq!(super::redact("(for url (https://x.com/?a=1))"), "(for url (https://x.com/?…))");
    }
}
