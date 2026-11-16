//! Small custom widgets: a fixed-aspect container, the floating player host
//! (which can be clipped to a scrolled viewport) and the seek bar.

use gtk::glib::subclass::prelude::*;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, RefCell};

// ---------------------------------------------------------------------------
// AspectBin: height follows width at a fixed ratio.

mod aspect_imp {
    use super::*;

    pub struct AspectBin {
        pub child: RefCell<Option<gtk::Widget>>,
        pub ratio: Cell<f64>,
    }

    impl Default for AspectBin {
        fn default() -> Self {
            Self { child: RefCell::new(None), ratio: Cell::new(16.0 / 9.0) }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AspectBin {
        const NAME: &'static str = "GtAspectBin";
        type Type = super::AspectBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for AspectBin {
        fn dispose(&self) {
            if let Some(c) = self.child.take() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for AspectBin {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let ratio = self.ratio.get().max(0.01);
            match orientation {
                gtk::Orientation::Horizontal => (0, 0, -1, -1),
                _ => {
                    // GTK requires the minimum height not to grow with the
                    // width, so the aspect height is only the natural size
                    // (like GtkPicture does).
                    let w = if for_size >= 0 { for_size } else { self.obj().width_request().max(0) };
                    let h = (w as f64 / ratio).round() as i32;
                    (0, h, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(c) = self.child.borrow().as_ref() {
                c.allocate(width, height, baseline, None);
            }
        }
    }
}

glib::wrapper! {
    pub struct AspectBin(ObjectSubclass<aspect_imp::AspectBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AspectBin {
    pub fn new(ratio: f64) -> Self {
        let o: Self = glib::Object::new();
        o.imp().ratio.set(ratio);
        o.set_overflow(gtk::Overflow::Hidden);
        o
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        if let Some(old) = self.imp().child.take() {
            old.unparent();
        }
        if let Some(c) = child {
            c.set_parent(self);
            self.imp().child.replace(Some(c.clone().upcast()));
        }
        self.queue_resize();
    }
}

// ---------------------------------------------------------------------------
// FitBin: shows its child at a fixed aspect ratio, as large as fits in the
// space it is given (centered), up to a maximum width. Music's artwork and
// video on "Now playing".

mod fit_imp {
    use super::*;

    pub struct FitBin {
        pub child: RefCell<Option<gtk::Widget>>,
        pub ratio: Cell<f64>,
        pub max_width: Cell<i32>,
    }

    impl Default for FitBin {
        fn default() -> Self {
            Self { child: RefCell::new(None), ratio: Cell::new(1.0), max_width: Cell::new(640) }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FitBin {
        const NAME: &'static str = "GtFitBin";
        type Type = super::FitBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for FitBin {
        fn dispose(&self) {
            if let Some(c) = self.child.take() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for FitBin {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let ratio = self.ratio.get().max(0.01);
            let max = self.max_width.get();
            match orientation {
                gtk::Orientation::Horizontal => (0, max, -1, -1),
                _ => {
                    let w = if for_size >= 0 { for_size.min(max) } else { max };
                    (0, (w as f64 / ratio).round() as i32, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let Some(c) = self.child.borrow().clone() else { return };
            let ratio = self.ratio.get().max(0.01);
            let mut w = (width.min(self.max_width.get())) as f64;
            let mut h = w / ratio;
            if h > height as f64 {
                h = height as f64;
                w = h * ratio;
            }
            let (w, h) = (w.floor() as i32, h.floor() as i32);
            let x = ((width - w) / 2) as f32;
            let y = ((height - h) / 2) as f32;
            c.allocate(w.max(0), h.max(0), -1, Some(gsk::Transform::new().translate(&graphene::Point::new(x, y))));
        }
    }
}

glib::wrapper! {
    pub struct FitBin(ObjectSubclass<fit_imp::FitBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl FitBin {
    pub fn new(ratio: f64, max_width: i32) -> Self {
        let o: Self = glib::Object::new();
        o.imp().ratio.set(ratio);
        o.imp().max_width.set(max_width);
        o
    }

    pub fn set_max_width(&self, w: i32) {
        if self.imp().max_width.replace(w) != w {
            self.queue_resize();
        }
    }

    pub fn set_child(&self, child: Option<&impl IsA<gtk::Widget>>) {
        if let Some(old) = self.imp().child.take() {
            old.unparent();
        }
        if let Some(c) = child {
            c.set_parent(self);
            self.imp().child.replace(Some(c.clone().upcast()));
        }
        self.queue_resize();
    }
}

// ---------------------------------------------------------------------------
// SideLayout: a main column plus a fixed-width sidebar (the watch page's
// "Up next"), or both stacked when narrow. Unlike a GtkBox it measures the
// main column at exactly the width it will get, so height-for-width content
// (the 16:9 player area) is never squeezed. It can also keep the main column
// narrow enough for a picture at its top to fit the scrolled view's height
// (the watch page's player on wide screens), centering both columns.

mod side_imp {
    use super::*;

    pub struct SideLayout {
        pub main: RefCell<Option<gtk::Widget>>,
        pub side: RefCell<Option<gtk::Widget>>,
        pub side_width: Cell<i32>,
        pub spacing: Cell<i32>,
        pub stacked: Cell<bool>,
        /// (aspect ratio, room to leave below it): the main column is no
        /// wider than a picture of that ratio that fits the view's height.
        pub fit: Cell<Option<(f64, i32)>>,
        /// The view height the last measurement was made for.
        pub measured_for: Cell<i32>,
    }

    impl Default for SideLayout {
        fn default() -> Self {
            Self {
                main: RefCell::new(None),
                side: RefCell::new(None),
                side_width: Cell::new(super::SideLayout::SIDE_WIDTH),
                spacing: Cell::new(24),
                stacked: Cell::new(false),
                fit: Cell::new(None),
                measured_for: Cell::new(-1),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SideLayout {
        const NAME: &'static str = "GtSideLayout";
        type Type = super::SideLayout;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SideLayout {
        fn dispose(&self) {
            for c in [self.main.take(), self.side.take()].into_iter().flatten() {
                c.unparent();
            }
        }
    }

    fn visible(c: &RefCell<Option<gtk::Widget>>) -> Option<gtk::Widget> {
        c.borrow().clone().filter(|w| w.is_visible())
    }

    fn measure(w: &Option<gtk::Widget>, o: gtk::Orientation, for_size: i32) -> (i32, i32) {
        match w {
            Some(w) => {
                let (min, nat, _, _) = w.measure(o, for_size);
                (min, nat)
            }
            None => (0, 0),
        }
    }

    impl SideLayout {
        /// The height of the scrolled view this is in (or its own).
        fn view_height(&self) -> i32 {
            let obj = self.obj();
            obj.ancestor(gtk::ScrolledWindow::static_type()).map_or_else(|| obj.height(), |sw| sw.height())
        }

        /// Width the main column gets out of `total`.
        fn main_width(&self, main: &Option<gtk::Widget>, total: i32) -> i32 {
            let min = measure(main, gtk::Orientation::Horizontal, -1).0;
            let mut w = total - self.spacing.get() - self.side_width.get();
            if let Some((ratio, below)) = self.fit.get() {
                let vh = self.view_height();
                self.measured_for.set(vh);
                if vh > 0 {
                    w = w.min(((vh - below).max(0) as f64 * ratio) as i32);
                }
            }
            w.max(min)
        }
    }

    impl WidgetImpl for SideLayout {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, o: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (main, side) = (visible(&self.main), visible(&self.side));
            let sp = if main.is_some() && side.is_some() { self.spacing.get() } else { 0 };
            let (min, nat) = match (self.stacked.get(), o) {
                (true, gtk::Orientation::Horizontal) => {
                    let (a, b) = measure(&main, o, -1);
                    let (c, d) = measure(&side, o, -1);
                    (a.max(c), b.max(d))
                }
                (true, _) => {
                    let (a, b) = measure(&main, o, for_size);
                    let (c, d) = measure(&side, o, for_size);
                    (a + sp + c, b + sp + d)
                }
                (false, gtk::Orientation::Horizontal) => {
                    let (a, b) = measure(&main, o, -1);
                    let sw = if side.is_some() { self.side_width.get() } else { 0 };
                    (a + sp + sw, b + sp + sw)
                }
                (false, _) => {
                    let mw = if for_size < 0 { -1 } else { self.main_width(&main, for_size) };
                    let (a, b) = measure(&main, o, mw);
                    let (c, d) = measure(&side, o, self.side_width.get());
                    (a.max(c), b.max(d))
                }
            };
            (min, nat, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let (main, side) = (visible(&self.main), visible(&self.side));
            let sp = self.spacing.get();
            if self.stacked.get() {
                let mh = measure(&main, gtk::Orientation::Vertical, width).1.min(height);
                if let Some(m) = &main {
                    m.allocate(width, mh, -1, None);
                }
                if let Some(s) = &side {
                    let y = if main.is_some() { mh + sp } else { 0 };
                    let t = gsk::Transform::new().translate(&graphene::Point::new(0.0, y as f32));
                    s.allocate(width, (height - y).max(0), -1, Some(t));
                }
            } else {
                let mw = if side.is_some() { self.main_width(&main, width) } else { width };
                // Both columns centered when the main one was kept narrower.
                let used = mw + if side.is_some() { sp + self.side_width.get() } else { 0 };
                let x0 = ((width - used) / 2).max(0);
                if let Some(m) = &main {
                    let t = gsk::Transform::new().translate(&graphene::Point::new(x0 as f32, 0.0));
                    m.allocate(mw, height, -1, Some(t));
                }
                if let Some(s) = &side {
                    let x = x0 + if main.is_some() { mw + sp } else { 0 };
                    let t = gsk::Transform::new().translate(&graphene::Point::new(x as f32, 0.0));
                    s.allocate(self.side_width.get(), height, -1, Some(t));
                }
            }
            // The view's height changed since the measurement it was laid
            // out with: measure again for the new height.
            if self.fit.get().is_some() && !self.stacked.get() && self.view_height() != self.measured_for.get() {
                let obj = self.obj().downgrade();
                glib::idle_add_local_once(move || {
                    if let Some(o) = obj.upgrade() {
                        o.queue_resize();
                    }
                });
            }
        }
    }
}

glib::wrapper! {
    pub struct SideLayout(ObjectSubclass<side_imp::SideLayout>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SideLayout {
    pub fn new(main: &impl IsA<gtk::Widget>, side: &impl IsA<gtk::Widget>) -> Self {
        let o: Self = glib::Object::new();
        main.set_parent(&o);
        side.set_parent(&o);
        o.imp().main.replace(Some(main.clone().upcast()));
        o.imp().side.replace(Some(side.clone().upcast()));
        o
    }

    pub const SIDE_WIDTH: i32 = 402;

    /// Stacks the sidebar below the main column (narrow windows).
    pub fn set_stacked(&self, stacked: bool) {
        if self.imp().stacked.replace(stacked) != stacked {
            self.queue_resize();
        }
    }

    pub fn set_side_width(&self, w: i32) {
        if self.imp().side_width.replace(w) != w {
            self.queue_resize();
        }
    }

    /// Keeps the main column narrow enough for a `ratio` picture at its top
    /// to fit the scrolled view's height with `below` pixels to spare.
    pub fn set_fit_height(&self, ratio: f64, below: i32) {
        self.imp().fit.set(Some((ratio, below)));
        self.queue_resize();
    }
}

// ---------------------------------------------------------------------------
// PlayerHost: a bin whose drawing and input can be clipped to a rectangle.

mod host_imp {
    use super::*;

    #[derive(Default)]
    pub struct PlayerHost {
        pub child: RefCell<Option<gtk::Widget>>,
        pub clip: Cell<Option<graphene::Rect>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayerHost {
        const NAME: &'static str = "GtPlayerHost";
        type Type = super::PlayerHost;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for PlayerHost {
        fn dispose(&self) {
            if let Some(c) = self.child.take() {
                c.unparent();
            }
        }
    }

    impl WidgetImpl for PlayerHost {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.child.borrow().as_ref() {
                Some(c) => {
                    let (min, _, _, _) = c.measure(orientation, for_size);
                    (min, min, -1, -1)
                }
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(c) = self.child.borrow().as_ref() {
                c.allocate(width, height, baseline, None);
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let Some(child) = self.child.borrow().clone() else { return };
            match self.clip.get() {
                Some(clip) if clip.width() <= 0.0 || clip.height() <= 0.0 => {}
                Some(clip) => {
                    snapshot.push_clip(&clip);
                    self.obj().snapshot_child(&child, snapshot);
                    snapshot.pop();
                }
                None => self.obj().snapshot_child(&child, snapshot),
            }
        }

        fn contains(&self, x: f64, y: f64) -> bool {
            if let Some(clip) = self.clip.get()
                && !clip.contains_point(&graphene::Point::new(x as f32, y as f32))
            {
                return false;
            }
            self.parent_contains(x, y)
        }
    }
}

glib::wrapper! {
    pub struct PlayerHost(ObjectSubclass<host_imp::PlayerHost>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayerHost {
    pub fn new(child: &impl IsA<gtk::Widget>) -> Self {
        let o: Self = glib::Object::new();
        child.set_parent(&o);
        o.imp().child.replace(Some(child.clone().upcast()));
        o
    }

    pub fn set_clip(&self, clip: Option<graphene::Rect>) {
        let old = self.imp().clip.get();
        let same = match (old, clip) {
            (None, None) => true,
            (Some(a), Some(b)) => a == b,
            _ => false,
        };
        if !same {
            self.imp().clip.set(clip);
            self.queue_draw();
        }
    }
}

// ---------------------------------------------------------------------------
// SeekBar

type SeekFn = Box<dyn Fn(f64, bool)>;

mod seek_imp {
    use super::*;

    #[derive(Default)]
    pub struct SeekBar {
        pub duration: Cell<f64>,
        pub position: Cell<f64>,
        pub buffered: Cell<f64>,
        pub segments: RefCell<Vec<(f64, f64, gdk::RGBA, String)>>,
        /// A segment being marked for SponsorBlock: start, and end once set
        /// (until then it follows the playback position).
        pub draft: Cell<Option<(f64, Option<f64>)>>,
        pub hover_x: Cell<Option<f64>>,
        pub drag_x: Cell<Option<f64>>,
        pub drag_start: Cell<f64>,
        pub on_seek: RefCell<Option<SeekFn>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SeekBar {
        const NAME: &'static str = "GtSeekBar";
        type Type = super::SeekBar;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SeekBar {}

    impl WidgetImpl for SeekBar {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (40, 200, -1, -1),
                _ => (20, 20, -1, -1),
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let w = obj.width() as f32;
            let h = obj.height() as f32;
            let active = self.hover_x.get().is_some() || self.drag_x.get().is_some();
            let bar_h: f32 = if active { 6.0 } else { 4.0 };
            let y = (h - bar_h) / 2.0;
            let dur = self.duration.get();
            let frac = |t: f64| if dur > 0.0 { (t / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };

            let bar = |snapshot: &gtk::Snapshot, x0: f32, x1: f32, color: &gdk::RGBA| {
                if x1 <= x0 {
                    return;
                }
                let r = graphene::Rect::new(x0, y, x1 - x0, bar_h);
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(r, bar_h / 2.0));
                snapshot.append_color(color, &r);
                snapshot.pop();
            };

            // Trough colors follow the text color: white over video, the
            // theme's foreground on the Music player bar.
            let fg = obj.color();
            let tint = |a: f32| gdk::RGBA::new(fg.red(), fg.green(), fg.blue(), a);
            bar(snapshot, 0.0, w, &tint(0.28));
            let pos = match self.drag_x.get() {
                Some(x) => (x as f32 / w).clamp(0.0, 1.0),
                None => frac(self.position.get()),
            };
            let buf = frac(self.buffered.get());
            if buf > pos {
                bar(snapshot, pos * w, buf * w, &tint(0.45));
            }
            // The played part in the accent color.
            let accent = adw::StyleManager::default().accent_color_rgba();
            bar(snapshot, 0.0, pos * w, &accent);
            // SponsorBlock segments on top, so they stay visible after
            // they've been played (longest first, so short ones inside long
            // ones show). Highlights are a single point: a small marker.
            if dur > 0.0 {
                let mut segs: Vec<_> = self.segments.borrow().iter().map(|(s, e, c, _)| (*s, *e, *c)).collect();
                segs.sort_by(|a, b| (b.1 - b.0).total_cmp(&(a.1 - a.0)));
                for (s, e, color) in segs {
                    let (x0, x1) = (frac(s) * w, frac(e) * w);
                    if x1 - x0 < 2.0 {
                        let m = graphene::Rect::new((x0 - 1.5).max(0.0), y - 2.0, 3.0, bar_h + 4.0);
                        snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(m, 1.5));
                        snapshot.append_color(&color, &m);
                        snapshot.pop();
                    } else {
                        bar(snapshot, x0, x1, &color);
                    }
                }
            }
            if let Some((s, e)) = self.draft.get()
                && dur > 0.0
            {
                let e = e.unwrap_or(self.position.get());
                let (a, b) = (frac(s.min(e)) * w, frac(s.max(e)) * w);
                let white = gdk::RGBA::new(1.0, 1.0, 1.0, 0.85);
                bar(snapshot, a, b.max(a + 2.0), &white);
                for x in [a, b] {
                    let m = graphene::Rect::new((x - 1.0).max(0.0), y - 4.0, 2.0, bar_h + 8.0);
                    snapshot.append_color(&white, &m);
                }
            }
            if active {
                let r = 7.0;
                let c = graphene::Rect::new(pos * w - r, h / 2.0 - r, 2.0 * r, 2.0 * r);
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(c, r));
                snapshot.append_color(&accent, &c);
                snapshot.pop();
            }
        }
    }
}

glib::wrapper! {
    pub struct SeekBar(ObjectSubclass<seek_imp::SeekBar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// A vertically scrolling window whose content gets its *natural* height.
/// (GtkViewport defaults to the minimum height, which would collapse
/// aspect-ratio thumbnails to nothing.)
pub fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    let vp = gtk::Viewport::builder().vscroll_policy(gtk::ScrollablePolicy::Natural).child(child).build();
    gtk::ScrolledWindow::builder().hscrollbar_policy(gtk::PolicyType::Never).child(&vp).build()
}

pub fn format_time(t: f64) -> String {
    if !t.is_finite() || t < 0.0 {
        return "0:00".into();
    }
    let t = t as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

impl SeekBar {
    pub fn new() -> Self {
        let o: Self = glib::Object::new();
        o.set_hexpand(true);
        o.set_cursor_from_name(Some("pointer"));
        let weak = o.downgrade();
        adw::StyleManager::default().connect_accent_color_rgba_notify(move |_| {
            if let Some(o) = weak.upgrade() {
                o.queue_draw();
            }
        });

        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(
            #[weak]
            o,
            move |_, x, _| {
                o.imp().drag_start.set(x);
                o.imp().drag_x.set(Some(x));
                o.emit_seek(x, false);
                o.queue_draw();
            }
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak]
            o,
            move |_, dx, _| {
                let x = o.imp().drag_start.get() + dx;
                o.imp().drag_x.set(Some(x));
                o.emit_seek(x, false);
                o.queue_draw();
            }
        ));
        drag.connect_drag_end(glib::clone!(
            #[weak]
            o,
            move |_, dx, _| {
                let x = o.imp().drag_start.get() + dx;
                o.emit_seek(x, true);
                o.imp().drag_x.set(None);
                o.queue_draw();
            }
        ));
        o.add_controller(drag);

        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(glib::clone!(
            #[weak]
            o,
            move |_, x, _| {
                o.imp().hover_x.set(Some(x));
                let t = o.time_at(x);
                // A few pixels of slack, so highlight markers can be hovered.
                let slack = 4.0 * o.imp().duration.get() / o.width().max(1) as f64;
                let seg = o
                    .imp()
                    .segments
                    .borrow()
                    .iter()
                    .filter(|(s, e, _, _)| t >= *s - slack && t <= *e + slack)
                    .min_by(|a, b| (a.1 - a.0).total_cmp(&(b.1 - b.0)))
                    .map(|(_, _, _, l)| l.clone());
                let tip = match seg {
                    Some(l) => format!("{} • {l}", format_time(t)),
                    None => format_time(t),
                };
                o.set_tooltip_text(Some(&tip));
                o.queue_draw();
            }
        ));
        motion.connect_leave(glib::clone!(
            #[weak]
            o,
            move |_| {
                o.imp().hover_x.set(None);
                o.queue_draw();
            }
        ));
        o.add_controller(motion);
        o
    }

    fn time_at(&self, x: f64) -> f64 {
        let w = self.width().max(1) as f64;
        (x / w).clamp(0.0, 1.0) * self.imp().duration.get()
    }

    fn emit_seek(&self, x: f64, final_: bool) {
        let t = self.time_at(x);
        if let Some(f) = self.imp().on_seek.borrow().as_ref() {
            f(t, final_);
        }
    }

    pub fn connect_seek(&self, f: impl Fn(f64, bool) + 'static) {
        self.imp().on_seek.replace(Some(Box::new(f)));
    }

    pub fn set_state(&self, position: f64, duration: f64, buffered: f64) {
        let imp = self.imp();
        if imp.position.get() != position || imp.duration.get() != duration || imp.buffered.get() != buffered {
            imp.position.set(position);
            imp.duration.set(duration);
            imp.buffered.set(buffered);
            self.queue_draw();
        }
    }

    pub fn set_segments(&self, segs: Vec<(f64, f64, gdk::RGBA, String)>) {
        self.imp().segments.replace(segs);
        self.queue_draw();
    }

    pub fn set_draft(&self, draft: Option<(f64, Option<f64>)>) {
        self.imp().draft.set(draft);
        self.queue_draw();
    }
}

impl Default for SeekBar {
    fn default() -> Self {
        Self::new()
    }
}
