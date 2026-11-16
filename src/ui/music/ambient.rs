//! "Ambient color": tints Music mode with the dominant color of the album
//! art that is playing, like music.youtube.com does.

use crate::config;
use gtk::{gdk, glib, prelude::*};
use std::cell::{Cell, RefCell};

pub struct Ambient {
    provider: gtk::CssProvider,
    /// Widgets that get the "ambient" style class while a color is applied.
    targets: RefCell<Vec<gtk::Widget>>,
    url: RefCell<String>,
    color: Cell<Option<[f32; 3]>>,
}

impl Ambient {
    /// `target` is the Music shell; it gets the "ambient" style class while
    /// a color is applied.
    pub fn new(target: &impl IsA<gtk::Widget>) -> std::rc::Rc<Self> {
        let provider = gtk::CssProvider::new();
        if let Some(d) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(&d, &provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
        }
        let this = std::rc::Rc::new(Self {
            provider,
            targets: RefCell::new(vec![target.clone().upcast()]),
            url: RefCell::new(String::new()),
            color: Cell::new(None),
        });
        // The tint is mixed differently for light and dark styles.
        let wk = std::rc::Rc::downgrade(&this);
        adw::StyleManager::default().connect_dark_notify(move |_| {
            if let Some(a) = wk.upgrade() {
                a.apply();
            }
        });
        this
    }

    /// The art of the item that is playing (None: nothing is).
    pub fn set_art(self: &std::rc::Rc<Self>, url: Option<&str>) {
        let url = url.unwrap_or_default().to_string();
        if *self.url.borrow() == url {
            return;
        }
        self.url.replace(url.clone());
        if url.is_empty() {
            self.color.set(None);
            self.apply();
            return;
        }
        let wk = std::rc::Rc::downgrade(self);
        let load = url.clone();
        crate::images::load(&load, move |tex| {
            let Some(a) = wk.upgrade() else { return };
            if *a.url.borrow() != url {
                return;
            }
            a.color.set(dominant_color(tex));
            a.apply();
        });
    }

    /// Tints `w` too (the immersive view).
    pub fn add_target(&self, w: &impl IsA<gtk::Widget>) {
        self.targets.borrow_mut().push(w.clone().upcast());
        self.apply();
    }

    /// Re-applies the tint (after the setting changed).
    pub fn apply(&self) {
        let color = self.color.get().filter(|_| config::settings().music_ambient);
        let Some(rgb) = color else {
            for t in self.targets.borrow().iter() {
                t.remove_css_class("ambient");
            }
            return;
        };
        let dark = adw::StyleManager::default().is_dark();
        let css = |c: [f32; 3]| format!("rgb({},{},{})", (c[0] * 255.0).round(), (c[1] * 255.0).round(), (c[2] * 255.0).round());
        let base = css(rgb);
        // The visualizer needs a brighter (dark style) or deeper (light
        // style) shade to stand out from the tinted bar.
        let (h, s, _) = rgb_to_hsl(rgb);
        let bars = css(hsl_to_rgb(h, (s * 1.15).min(0.9), if dark { 0.68 } else { 0.42 }));
        // The immersive view is always dark.
        let glow = css(hsl_to_rgb(h, (s * 1.2).min(0.95), 0.66));
        let (top, bar, now) = if dark { (0.34, 0.24, 0.5) } else { (0.28, 0.18, 0.38) };
        self.provider.load_from_string(&format!(
            ".music.ambient .music-content {{ background-image: linear-gradient(to bottom, alpha({base}, {top}), alpha({base}, 0) 520px); }}\n\
             .music.ambient .music-bar {{ background-color: mix(@window_bg_color, {base}, {bar}); }}\n\
             .music.ambient .music-content.now-open {{ background-image: linear-gradient(170deg, alpha({base}, {now}), alpha({base}, 0.08) 85%); }}\n\
             .music.ambient .music-visualizer {{ color: {bars}; }}\n\
             .immersive.ambient {{ background-color: mix(#0b0b0f, {base}, 0.3); }}\n\
             .immersive.ambient .immersive-shade {{ background-image: radial-gradient(ellipse at 50% 35%, alpha({base}, 0.32), alpha(#000, 0.55) 75%); }}\n\
             .immersive.ambient .music-visualizer {{ color: {glow}; }}\n"
        ));
        for t in self.targets.borrow().iter() {
            t.add_css_class("ambient");
        }
    }
}

/// The most prominent color of an image: the strongest hue (by area and
/// saturation), with its lightness evened out so text stays readable over
/// it. Black and white artwork gives a neutral grey.
pub fn dominant_color(tex: &gdk::Texture) -> Option<[f32; 3]> {
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let stride = w * 4;
    let mut buf = vec![0u8; stride * h];
    // GDK_MEMORY_DEFAULT: premultiplied B, G, R, A bytes.
    tex.download(&mut buf, stride);
    const BINS: usize = 24;
    let mut weight = [0f32; BINS];
    let mut sum = [[0f32; 3]; BINS];
    let (mut total, mut avg, mut n) = (0f32, [0f32; 3], 0f32);
    let step = (w.max(h) / 64).max(1);
    for y in (0..h).step_by(step) {
        for x in (0..w).step_by(step) {
            let i = y * stride + x * 4;
            let a = buf[i + 3] as f32 / 255.0;
            if a < 0.5 {
                continue;
            }
            let rgb = [buf[i + 2] as f32 / 255.0 / a, buf[i + 1] as f32 / 255.0 / a, buf[i] as f32 / 255.0 / a];
            let (hue, s, l) = rgb_to_hsl(rgb);
            for c in 0..3 {
                avg[c] += rgb[c];
            }
            n += 1.0;
            // Ignore near-black (letterboxing), near-white and grey pixels.
            if !(0.12..=0.92).contains(&l) || s < 0.18 {
                continue;
            }
            let wgt = s * s * (1.0 - (l - 0.5).abs() * 1.4).max(0.05);
            let b = ((hue / 360.0) * BINS as f32) as usize % BINS;
            weight[b] += wgt;
            for c in 0..3 {
                sum[b][c] += rgb[c] * wgt;
            }
            total += wgt;
        }
    }
    if n == 0.0 {
        return None;
    }
    // Colorful pixels must cover a meaningful part of the image.
    if total / n < 0.02 {
        let (hue, s, _) = rgb_to_hsl([avg[0] / n, avg[1] / n, avg[2] / n]);
        return Some(hsl_to_rgb(hue, s.min(0.12), 0.5));
    }
    // The strongest hue, together with its neighbours.
    let best = (0..BINS)
        .max_by(|&a, &b| {
            let around = |i: usize| weight[(i + BINS - 1) % BINS] * 0.5 + weight[i] + weight[(i + 1) % BINS] * 0.5;
            around(a).total_cmp(&around(b))
        })
        .unwrap_or(0);
    let mut c = [0f32; 3];
    let mut wsum = 0f32;
    for i in [(best + BINS - 1) % BINS, best, (best + 1) % BINS] {
        for k in 0..3 {
            c[k] += sum[i][k];
        }
        wsum += weight[i];
    }
    let (hue, s, _) = rgb_to_hsl([c[0] / wsum, c[1] / wsum, c[2] / wsum]);
    Some(hsl_to_rgb(hue, s.clamp(0.3, 0.8), 0.5))
}

/// A heavily blurred, small copy of `tex` (a soft wash of its colors when
/// scaled up), for backdrops.
pub fn blurred(tex: &gdk::Texture) -> Option<gdk::Texture> {
    const N: usize = 24;
    let (w, h) = (tex.width() as usize, tex.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let stride = w * 4;
    let mut buf = vec![0u8; stride * h];
    tex.download(&mut buf, stride);
    // Average into an N×N grid (premultiplied BGRA, so averaging is fine).
    let mut grid = vec![[0f32; 4]; N * N];
    for (gy, row) in grid.chunks_mut(N).enumerate() {
        let (y0, y1) = (gy * h / N, ((gy + 1) * h / N).max(gy * h / N + 1));
        for (gx, cell) in row.iter_mut().enumerate() {
            let (x0, x1) = (gx * w / N, ((gx + 1) * w / N).max(gx * w / N + 1));
            let mut n = 0f32;
            for y in (y0..y1.min(h)).step_by(((y1 - y0) / 8).max(1)) {
                for x in (x0..x1.min(w)).step_by(((x1 - x0) / 8).max(1)) {
                    let i = y * stride + x * 4;
                    for c in 0..4 {
                        cell[c] += buf[i + c] as f32;
                    }
                    n += 1.0;
                }
            }
            for v in cell.iter_mut() {
                *v /= n.max(1.0);
            }
        }
    }
    // A few box blur passes.
    for _ in 0..3 {
        let src = grid.clone();
        for y in 0..N {
            for x in 0..N {
                let mut acc = [0f32; 4];
                let mut n = 0f32;
                for dy in -2i32..=2 {
                    for dx in -2i32..=2 {
                        let (sx, sy) = ((x as i32 + dx).clamp(0, N as i32 - 1) as usize, (y as i32 + dy).clamp(0, N as i32 - 1) as usize);
                        for c in 0..4 {
                            acc[c] += src[sy * N + sx][c];
                        }
                        n += 1.0;
                    }
                }
                for c in 0..4 {
                    grid[y * N + x][c] = acc[c] / n;
                }
            }
        }
    }
    let bytes: Vec<u8> = grid.iter().flat_map(|c| c.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect();
    let tex = gdk::MemoryTexture::new(N as i32, N as i32, gdk::MemoryFormat::B8g8r8a8Premultiplied, &glib::Bytes::from_owned(bytes), N * 4);
    Some(tex.upcast())
}

fn rgb_to_hsl([r, g, b]: [f32; 3]) -> (f32, f32, f32) {
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return (0.0, 0.0, l);
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs()).max(1e-6);
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    (h, s.min(1.0), l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = match (h.rem_euclid(360.0) / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [(r + m).clamp(0.0, 1.0), (g + m).clamp(0.0, 1.0), (b + m).clamp(0.0, 1.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_round_trip() {
        for rgb in [[0.8, 0.2, 0.1], [0.1, 0.5, 0.9], [0.3, 0.3, 0.3], [0.2, 0.7, 0.4]] {
            let (h, s, l) = rgb_to_hsl(rgb);
            let back = hsl_to_rgb(h, s, l);
            for k in 0..3 {
                assert!((back[k] - rgb[k]).abs() < 0.01, "{rgb:?} -> {back:?}");
            }
        }
    }
}
