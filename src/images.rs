//! Async thumbnail/avatar loading with an in-memory cache.

use crate::runtime;
use gtk::{gdk, glib, prelude::*};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::LazyLock;

type Callback = Box<dyn FnOnce(&gdk::Texture)>;

thread_local! {
    static CACHE: RefCell<HashMap<String, gdk::Texture>> = RefCell::new(HashMap::new());
    static PENDING: RefCell<HashMap<String, Vec<Callback>>> = RefCell::new(HashMap::new());
}

static LIMIT: LazyLock<tokio::sync::Semaphore> = LazyLock::new(|| tokio::sync::Semaphore::new(8));

/// Fetches `url` and calls `f` with the decoded texture (immediately if cached).
pub fn load(url: &str, f: impl FnOnce(&gdk::Texture) + 'static) {
    if url.is_empty() {
        return;
    }
    if let Some(t) = CACHE.with_borrow(|c| c.get(url).cloned()) {
        f(&t);
        return;
    }
    let first = PENDING.with_borrow_mut(|p| {
        let list = p.entry(url.to_string()).or_default();
        list.push(Box::new(f));
        list.len() == 1
    });
    if !first {
        return;
    }
    let url = url.to_string();
    runtime::spawn_local(async move {
        let u = url.clone();
        let res = runtime::run(async move {
            let _permit = LIMIT.acquire().await?;
            let bytes = crate::api::client().http().get(&u).send().await?.error_for_status()?.bytes().await?;
            let tex = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes.to_vec()))?;
            Ok(tex)
        })
        .await;
        let callbacks = PENDING.with_borrow_mut(|p| p.remove(&url)).unwrap_or_default();
        if let Ok(tex) = res {
            CACHE.with_borrow_mut(|c| {
                if c.len() > 1500 {
                    c.clear();
                }
                c.insert(url, tex.clone());
            });
            for cb in callbacks {
                cb(&tex);
            }
        }
    });
}

pub fn picture(pic: &gtk::Picture, url: &str) {
    let weak = pic.downgrade();
    load(url, move |t| {
        if let Some(p) = weak.upgrade() {
            p.set_paintable(Some(t));
        }
    });
}

pub fn avatar(av: &adw::Avatar, url: Option<&str>) {
    let Some(url) = url else { return };
    let weak = av.downgrade();
    load(url, move |t| {
        if let Some(a) = weak.upgrade() {
            a.set_custom_image(Some(t));
        }
    });
}
