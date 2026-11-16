//! Running from one of the self-contained packages (the AppImage, the macOS
//! app bundle, the Windows folder): the tools that ship next to the program
//! (yt-dlp, deno) and, in the macOS bundle, where GTK finds its data.

use std::path::{Path, PathBuf};

/// Call first thing, while the process has a single thread.
pub fn setup() {
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
    // Tools next to the program come after the system's own (those are
    // usually kept up to date); yt-dlp finds deno through PATH too.
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if !paths.contains(&dir) {
        paths.push(dir.clone());
        if let Ok(joined) = std::env::join_paths(&paths) {
            // SAFETY: no other threads exist yet.
            unsafe { std::env::set_var("PATH", joined) };
        }
    }
    #[cfg(target_os = "macos")]
    macos(&dir);
}

/// GTube.app/Contents/MacOS/gtube keeps GTK's icons, settings schemas and
/// image loaders in Contents/Resources.
#[cfg(target_os = "macos")]
fn macos(dir: &Path) {
    let Some(res) = dir.parent().map(|c| c.join("Resources")).filter(|r| r.join("share").is_dir()) else { return };
    let share = res.join("share");
    let mut data: Vec<PathBuf> = vec![share.clone()];
    if let Some(old) = std::env::var_os("XDG_DATA_DIRS") {
        data.extend(std::env::split_paths(&old));
    }
    // SAFETY (all set_var calls): no other threads exist yet.
    if let Ok(joined) = std::env::join_paths(&data) {
        unsafe { std::env::set_var("XDG_DATA_DIRS", joined) };
    }
    unsafe { std::env::set_var("GSETTINGS_SCHEMA_DIR", share.join("glib-2.0/schemas")) };
    // The image loaders' list names them by absolute path: write one for
    // wherever the bundle is now.
    let pixbuf = res.join("lib/gdk-pixbuf-2.0/2.10.0");
    if let Ok(template) = std::fs::read_to_string(pixbuf.join("loaders.cache.in")) {
        let out = crate::config::cache_dir().join("pixbuf-loaders.cache");
        if std::fs::write(&out, template.replace("@RESOURCES@", &res.to_string_lossy())).is_ok() {
            unsafe {
                std::env::set_var("GDK_PIXBUF_MODULE_FILE", &out);
                std::env::set_var("GDK_PIXBUF_MODULEDIR", pixbuf.join("loaders"));
            }
        }
    }
}
