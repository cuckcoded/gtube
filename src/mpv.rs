// Complete binding of the parts of the libmpv API we use; not every
// constant/variant is referenced.
#![allow(dead_code)]
//! Small hand-written binding to libmpv's client and OpenGL render API.
//!
//! mpv handles the hard parts of playback: it runs yt-dlp (via its builtin
//! ytdl_hook) to resolve the stream URLs, merges the separate DASH audio and
//! video streams, and renders frames into the GtkGLArea framebuffer we hand it.

#![allow(non_camel_case_types)]

use std::cell::Cell;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::ptr;

#[repr(C)]
struct mpv_handle {
    _p: [u8; 0],
}
#[repr(C)]
struct mpv_render_context {
    _p: [u8; 0],
}
#[repr(C)]
struct mpv_event {
    event_id: c_int,
    error: c_int,
    reply_userdata: u64,
    data: *mut c_void,
}
#[repr(C)]
struct mpv_event_property {
    name: *const c_char,
    format: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct mpv_event_end_file {
    reason: c_int,
    error: c_int,
    playlist_entry_id: i64,
    playlist_insert_id: i64,
    playlist_insert_num_entries: c_int,
}
#[repr(C)]
struct mpv_event_log_message {
    prefix: *const c_char,
    level: *const c_char,
    text: *const c_char,
    log_level: c_int,
}
#[repr(C)]
struct mpv_render_param {
    type_: c_int,
    data: *mut c_void,
}
#[repr(C)]
struct mpv_opengl_init_params {
    get_proc_address: Option<unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void>,
    get_proc_address_ctx: *mut c_void,
}
#[repr(C)]
struct mpv_opengl_fbo {
    fbo: c_int,
    w: c_int,
    h: c_int,
    internal_format: c_int,
}

// Linked through build.rs (pkg-config finds libmpv wherever it lives).
unsafe extern "C" {
    fn mpv_create() -> *mut mpv_handle;
    fn mpv_initialize(ctx: *mut mpv_handle) -> c_int;
    fn mpv_terminate_destroy(ctx: *mut mpv_handle);
    fn mpv_set_option_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_set_property_string(ctx: *mut mpv_handle, name: *const c_char, data: *const c_char) -> c_int;
    fn mpv_set_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_get_property(ctx: *mut mpv_handle, name: *const c_char, format: c_int, data: *mut c_void) -> c_int;
    fn mpv_get_property_string(ctx: *mut mpv_handle, name: *const c_char) -> *mut c_char;
    fn mpv_free(data: *mut c_void);
    fn mpv_command(ctx: *mut mpv_handle, args: *mut *const c_char) -> c_int;
    fn mpv_observe_property(ctx: *mut mpv_handle, userdata: u64, name: *const c_char, format: c_int) -> c_int;
    fn mpv_wait_event(ctx: *mut mpv_handle, timeout: f64) -> *mut mpv_event;
    fn mpv_set_wakeup_callback(ctx: *mut mpv_handle, cb: Option<unsafe extern "C" fn(*mut c_void)>, d: *mut c_void);
    fn mpv_request_log_messages(ctx: *mut mpv_handle, min_level: *const c_char) -> c_int;
    fn mpv_error_string(error: c_int) -> *const c_char;

    fn mpv_render_context_create(
        res: *mut *mut mpv_render_context,
        mpv: *mut mpv_handle,
        params: *mut mpv_render_param,
    ) -> c_int;
    fn mpv_render_context_set_update_callback(
        ctx: *mut mpv_render_context,
        cb: Option<unsafe extern "C" fn(*mut c_void)>,
        d: *mut c_void,
    );
    fn mpv_render_context_render(ctx: *mut mpv_render_context, params: *mut mpv_render_param) -> c_int;
    fn mpv_render_context_free(ctx: *mut mpv_render_context);
}

// libgtk-4 exports both backends' accessors on Linux (only there); we
// only call the matching one.
#[cfg(not(any(windows, target_os = "macos")))]
unsafe extern "C" {
    fn gdk_wayland_display_get_wl_display(display: *mut c_void) -> *mut c_void;
    fn gdk_x11_display_get_xdisplay(display: *mut c_void) -> *mut c_void;
}

const FORMAT_NONE: c_int = 0;
const FORMAT_STRING: c_int = 1;
const FORMAT_FLAG: c_int = 3;
const FORMAT_INT64: c_int = 4;
const FORMAT_DOUBLE: c_int = 5;

const EVENT_NONE: c_int = 0;
const EVENT_SHUTDOWN: c_int = 1;
const EVENT_LOG_MESSAGE: c_int = 2;
const EVENT_START_FILE: c_int = 6;
const EVENT_END_FILE: c_int = 7;
const EVENT_FILE_LOADED: c_int = 8;
const EVENT_PLAYBACK_RESTART: c_int = 21;
const EVENT_PROPERTY_CHANGE: c_int = 22;

const RENDER_PARAM_INVALID: c_int = 0;
const RENDER_PARAM_API_TYPE: c_int = 1;
const RENDER_PARAM_OPENGL_INIT_PARAMS: c_int = 2;
const RENDER_PARAM_OPENGL_FBO: c_int = 3;
const RENDER_PARAM_FLIP_Y: c_int = 4;
const RENDER_PARAM_X11_DISPLAY: c_int = 8;
const RENDER_PARAM_WL_DISPLAY: c_int = 9;

pub const END_FILE_EOF: i32 = 0;
pub const END_FILE_ERROR: i32 = 4;

#[derive(Debug, Clone)]
pub enum Value {
    None,
    Flag(bool),
    Int(i64),
    Double(f64),
    Str(String),
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Double(d) => Some(*d),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Flag(b) => Some(*b),
            _ => None,
        }
    }
}

#[derive(Copy, Clone)]
pub enum Format {
    Flag,
    Double,
    Int,
    Str,
}

#[derive(Debug)]
pub enum Event {
    StartFile,
    FileLoaded,
    PlaybackRestart,
    EndFile { reason: i32, error: String },
    Property { name: String, value: Value },
    Log { prefix: String, level: String, text: String },
    Shutdown,
}

fn cstr(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap()
}

fn err_string(code: c_int) -> String {
    unsafe { CStr::from_ptr(mpv_error_string(code)).to_string_lossy().into_owned() }
}

pub struct Mpv {
    handle: *mut mpv_handle,
    render: Cell<*mut mpv_render_context>,
    wakeup_tx: *mut async_channel::Sender<()>,
    redraw_tx: *mut async_channel::Sender<()>,
}

unsafe extern "C" fn notify_cb(data: *mut c_void) {
    let tx = unsafe { &*(data as *const async_channel::Sender<()>) };
    let _ = tx.try_send(());
}

/// The X11 or Wayland display, which lets mpv use zero-copy hardware decoding.
#[cfg(not(any(windows, target_os = "macos")))]
fn native_display(display: &gtk::gdk::Display) -> Option<mpv_render_param> {
    use gtk::glib::translate::ToGlibPtr;
    let raw: *mut gtk::gdk::ffi::GdkDisplay = display.to_glib_none().0;
    let type_name = gtk::prelude::ObjectExt::type_(display).name();
    let (type_, data) = unsafe {
        match type_name {
            "GdkWaylandDisplay" => (RENDER_PARAM_WL_DISPLAY, gdk_wayland_display_get_wl_display(raw as *mut c_void)),
            "GdkX11Display" => (RENDER_PARAM_X11_DISPLAY, gdk_x11_display_get_xdisplay(raw as *mut c_void)),
            _ => return None,
        }
    };
    (!data.is_null()).then_some(mpv_render_param { type_, data })
}

/// Windows and macOS: mpv needs no display handle there.
#[cfg(any(windows, target_os = "macos"))]
fn native_display(_: &gtk::gdk::Display) -> Option<mpv_render_param> {
    None
}

impl Mpv {
    /// Creates the player. The returned receivers fire when mpv has events
    /// queued and when a new video frame should be drawn, respectively.
    pub fn new(options: &[(&str, String)]) -> Result<(Self, async_channel::Receiver<()>, async_channel::Receiver<()>), String> {
        let handle = unsafe { mpv_create() };
        if handle.is_null() {
            return Err("mpv_create failed".into());
        }
        let (wtx, wrx) = async_channel::bounded(1);
        let (rtx, rrx) = async_channel::bounded(1);
        let mpv = Mpv {
            handle,
            render: Cell::new(ptr::null_mut()),
            wakeup_tx: Box::into_raw(Box::new(wtx)),
            redraw_tx: Box::into_raw(Box::new(rtx)),
        };
        for (k, v) in options {
            let r = unsafe { mpv_set_option_string(handle, cstr(k).as_ptr(), cstr(v).as_ptr()) };
            if r < 0 {
                crate::log::warn(&format!("mpv: option {k}={v} failed: {}", err_string(r)));
            }
        }
        let r = unsafe { mpv_initialize(handle) };
        if r < 0 {
            return Err(format!("mpv_initialize failed: {}", err_string(r)));
        }
        unsafe {
            mpv_request_log_messages(handle, c"warn".as_ptr());
            mpv_set_wakeup_callback(handle, Some(notify_cb), mpv.wakeup_tx as *mut c_void);
        }
        Ok((mpv, wrx, rrx))
    }

    pub fn has_render_context(&self) -> bool {
        !self.render.get().is_null()
    }

    /// Must be called with the GtkGLArea's context current.
    pub fn create_render_context(&self, display: &gtk::gdk::Display) -> Result<(), String> {
        if self.has_render_context() {
            return Ok(());
        }
        gl::detect();
        let mut init = mpv_opengl_init_params { get_proc_address: Some(gl::get_proc_address), get_proc_address_ctx: ptr::null_mut() };
        let api = c"opengl";
        let mut params = vec![
            mpv_render_param { type_: RENDER_PARAM_API_TYPE, data: api.as_ptr() as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_OPENGL_INIT_PARAMS, data: &mut init as *mut _ as *mut c_void },
        ];
        params.extend(native_display(display));
        params.push(mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() });
        let mut ctx = ptr::null_mut();
        let r = unsafe { mpv_render_context_create(&mut ctx, self.handle, params.as_mut_ptr()) };
        if r < 0 {
            return Err(format!("mpv render context: {}", err_string(r)));
        }
        unsafe { mpv_render_context_set_update_callback(ctx, Some(notify_cb), self.redraw_tx as *mut c_void) };
        self.render.set(ctx);
        Ok(())
    }

    pub fn free_render_context(&self) {
        let ctx = self.render.replace(ptr::null_mut());
        if !ctx.is_null() {
            unsafe {
                mpv_render_context_set_update_callback(ctx, None, ptr::null_mut());
                mpv_render_context_free(ctx);
            }
        }
    }

    pub fn render(&self, fbo: i32, width: i32, height: i32) {
        let ctx = self.render.get();
        if ctx.is_null() {
            return;
        }
        let mut fbo = mpv_opengl_fbo { fbo, w: width, h: height, internal_format: 0 };
        let mut flip: c_int = 1;
        let mut params = [
            mpv_render_param { type_: RENDER_PARAM_OPENGL_FBO, data: &mut fbo as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_FLIP_Y, data: &mut flip as *mut _ as *mut c_void },
            mpv_render_param { type_: RENDER_PARAM_INVALID, data: ptr::null_mut() },
        ];
        unsafe { mpv_render_context_render(ctx, params.as_mut_ptr()) };
    }

    pub fn command(&self, args: &[&str]) -> Result<(), String> {
        let owned: Vec<CString> = args.iter().map(|a| cstr(a)).collect();
        let mut ptrs: Vec<*const c_char> = owned.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(ptr::null());
        let r = unsafe { mpv_command(self.handle, ptrs.as_mut_ptr()) };
        if r < 0 { Err(err_string(r)) } else { Ok(()) }
    }

    pub fn set_str(&self, name: &str, value: &str) {
        unsafe { mpv_set_property_string(self.handle, cstr(name).as_ptr(), cstr(value).as_ptr()) };
    }

    pub fn set_f64(&self, name: &str, mut value: f64) {
        unsafe { mpv_set_property(self.handle, cstr(name).as_ptr(), FORMAT_DOUBLE, &mut value as *mut _ as *mut c_void) };
    }

    pub fn set_bool(&self, name: &str, value: bool) {
        let mut v: c_int = value as c_int;
        unsafe { mpv_set_property(self.handle, cstr(name).as_ptr(), FORMAT_FLAG, &mut v as *mut _ as *mut c_void) };
    }

    pub fn get_f64(&self, name: &str) -> Option<f64> {
        let mut v: f64 = 0.0;
        let r = unsafe { mpv_get_property(self.handle, cstr(name).as_ptr(), FORMAT_DOUBLE, &mut v as *mut _ as *mut c_void) };
        (r >= 0).then_some(v)
    }

    pub fn get_str(&self, name: &str) -> Option<String> {
        unsafe {
            let p = mpv_get_property_string(self.handle, cstr(name).as_ptr());
            if p.is_null() {
                return None;
            }
            let s = CStr::from_ptr(p).to_string_lossy().into_owned();
            mpv_free(p as *mut c_void);
            Some(s)
        }
    }

    pub fn observe(&self, name: &str, format: Format) {
        let f = match format {
            Format::Flag => FORMAT_FLAG,
            Format::Double => FORMAT_DOUBLE,
            Format::Int => FORMAT_INT64,
            Format::Str => FORMAT_STRING,
        };
        unsafe { mpv_observe_property(self.handle, 0, cstr(name).as_ptr(), f) };
    }

    /// Returns the next queued event without blocking.
    pub fn poll_event(&self) -> Option<Event> {
        loop {
            let ev = unsafe { &*mpv_wait_event(self.handle, 0.0) };
            let out = unsafe {
                match ev.event_id {
                    EVENT_NONE => return None,
                    EVENT_SHUTDOWN => Event::Shutdown,
                    EVENT_START_FILE => Event::StartFile,
                    EVENT_FILE_LOADED => Event::FileLoaded,
                    EVENT_PLAYBACK_RESTART => Event::PlaybackRestart,
                    EVENT_END_FILE => {
                        let d = &*(ev.data as *const mpv_event_end_file);
                        Event::EndFile { reason: d.reason, error: if d.error < 0 { err_string(d.error) } else { String::new() } }
                    }
                    EVENT_LOG_MESSAGE => {
                        let d = &*(ev.data as *const mpv_event_log_message);
                        let s = |p: *const c_char| if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() };
                        Event::Log { prefix: s(d.prefix), level: s(d.level), text: s(d.text) }
                    }
                    EVENT_PROPERTY_CHANGE => {
                        let d = &*(ev.data as *const mpv_event_property);
                        let name = CStr::from_ptr(d.name).to_string_lossy().into_owned();
                        let value = match d.format {
                            FORMAT_FLAG => Value::Flag(*(d.data as *const c_int) != 0),
                            FORMAT_DOUBLE => Value::Double(*(d.data as *const f64)),
                            FORMAT_INT64 => Value::Int(*(d.data as *const i64)),
                            FORMAT_STRING => {
                                let p = *(d.data as *const *const c_char);
                                Value::Str(if p.is_null() { String::new() } else { CStr::from_ptr(p).to_string_lossy().into_owned() })
                            }
                            FORMAT_NONE | _ => Value::None,
                        };
                        Event::Property { name, value }
                    }
                    _ => continue,
                }
            };
            return Some(out);
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.free_render_context();
        unsafe {
            mpv_set_wakeup_callback(self.handle, None, ptr::null_mut());
            mpv_terminate_destroy(self.handle);
            drop(Box::from_raw(self.wakeup_tx));
            drop(Box::from_raw(self.redraw_tx));
        }
    }
}

/// OpenGL symbol lookup for whichever platform API GTK picked (EGL or GLX).
pub mod gl {
    use super::*;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU8, Ordering};

    type GetProc = unsafe extern "C" fn(*const c_char) -> *mut c_void;
    type GetCurrent = unsafe extern "C" fn() -> *mut c_void;
    type GetIntegerv = unsafe extern "C" fn(u32, *mut c_int);

    static MODE: AtomicU8 = AtomicU8::new(0); // 1 = EGL, 2 = the platform's own (GLX, WGL, CGL)
    static EGL: OnceLock<Option<libloading::Library>> = OnceLock::new();
    static NATIVE: OnceLock<Option<libloading::Library>> = OnceLock::new();

    #[cfg(windows)]
    const EGL_LIB: &str = "libEGL.dll";
    #[cfg(target_os = "macos")]
    const EGL_LIB: &str = "libEGL.dylib";
    #[cfg(not(any(windows, target_os = "macos")))]
    const EGL_LIB: &str = "libEGL.so.1";

    #[cfg(windows)]
    const NATIVE_LIB: &str = "opengl32.dll";
    #[cfg(target_os = "macos")]
    const NATIVE_LIB: &str = "/System/Library/Frameworks/OpenGL.framework/OpenGL";
    #[cfg(not(any(windows, target_os = "macos")))]
    const NATIVE_LIB: &str = "libGL.so.1";

    fn egl() -> Option<&'static libloading::Library> {
        EGL.get_or_init(|| unsafe { libloading::Library::new(EGL_LIB).ok() }).as_ref()
    }
    fn native() -> Option<&'static libloading::Library> {
        NATIVE.get_or_init(|| unsafe { libloading::Library::new(NATIVE_LIB).ok() }).as_ref()
    }

    /// Notes which GL API GTK made current (call with the GLArea's context current).
    pub fn detect() {
        let is_egl = egl()
            .and_then(|l| unsafe { l.get::<GetCurrent>(b"eglGetCurrentContext\0").ok().map(|f| *f) })
            .map(|f| unsafe { !f().is_null() })
            .unwrap_or(false);
        MODE.store(if is_egl { 1 } else { 2 }, Ordering::Relaxed);
    }

    /// A function exported by the GL library itself.
    fn exported(lib: &libloading::Library, name: *const c_char) -> *mut c_void {
        let name = unsafe { CStr::from_ptr(name) }.to_bytes_with_nul();
        unsafe { lib.get::<unsafe extern "C" fn()>(name).map(|f| *f as *mut c_void).unwrap_or(ptr::null_mut()) }
    }

    fn native_proc(name: *const c_char) -> *mut c_void {
        let Some(lib) = native() else { return ptr::null_mut() };
        #[cfg(windows)]
        {
            // wglGetProcAddress only knows extensions and GL > 1.1 (and
            // returns small sentinel values for the rest); GL 1.1 itself is
            // exported by opengl32.dll.
            let wgl = unsafe { lib.get::<GetProc>(b"wglGetProcAddress\0").ok().map(|f| *f) };
            let p = wgl.map(|f| unsafe { f(name) }).unwrap_or(ptr::null_mut());
            if !matches!(p as isize, -1..=3) {
                return p;
            }
            exported(lib, name)
        }
        #[cfg(target_os = "macos")]
        {
            // CGL has no lookup function: everything is exported.
            exported(lib, name)
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = exported;
            match unsafe { lib.get::<GetProc>(b"glXGetProcAddressARB\0").ok().map(|f| *f) } {
                Some(f) => unsafe { f(name) },
                None => ptr::null_mut(),
            }
        }
    }

    pub unsafe extern "C" fn get_proc_address(_ctx: *mut c_void, name: *const c_char) -> *mut c_void {
        if MODE.load(Ordering::Relaxed) == 1 {
            return match egl().and_then(|l| unsafe { l.get::<GetProc>(b"eglGetProcAddress\0").ok().map(|f| *f) }) {
                Some(f) => unsafe { f(name) },
                None => ptr::null_mut(),
            };
        }
        native_proc(name)
    }

    /// The framebuffer GTK bound for the GLArea's `render` signal.
    pub fn current_framebuffer() -> i32 {
        static GET: OnceLock<Option<GetIntegerv>> = OnceLock::new();
        let f = GET.get_or_init(|| {
            let p = unsafe { get_proc_address(ptr::null_mut(), c"glGetIntegerv".as_ptr()) };
            (!p.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, GetIntegerv>(p) })
        });
        let mut fbo: c_int = 0;
        if let Some(f) = f {
            unsafe { f(0x8CA6, &mut fbo) }; // GL_FRAMEBUFFER_BINDING
        }
        fbo
    }
}
