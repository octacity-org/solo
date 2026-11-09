//! Native child-view hosting for Solo.
//!
//! Most Solo elements are painted by GPUI. A few cannot be: `<webview>` is a
//! real WKWebView, an AppKit NSView that has to live in the window's NSView
//! hierarchy and composite *above* GPUI's opaque Metal surface. GPUI has no
//! API for hosting a foreign NSView, so Solo owns that bridge here.
//!
//! The division of labour is deliberate — GPUI owns layout, AppKit owns
//! pixels:
//!
//! ```text
//! retained Solo element
//!   → GPUI placeholder element (participates in normal layout)
//!     → prepaint() reports final post-layout bounds
//!       → registry.update_frame(element_id, frame)
//!         → NSView.setFrame:
//! ```
//!
//! Nothing here is driven from JavaScript: bounds arrive from GPUI's own
//! layout pass, so window resizes and sibling-driven reflows need no polling.
//!
//! The registry key is the Solo element ID, which is also the key the
//! retained tree uses. That is what lets `destroy` on the element and
//! `destroy_all` on app quit be the only two cleanup paths.

use futures::channel::oneshot;
use napi::bindgen_prelude::{AsyncTask, Error, Result, Task};
use napi_derive::napi;
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;

/// WKWebView host. macOS-only: `<webview>` has no implementation elsewhere, so
/// the element is simply not registered off macOS.
#[cfg(target_os = "macos")]
#[path = "native_view/webview_macos.rs"]
pub mod webview;
#[cfg(target_os = "linux")]
#[path = "native_view/webview_linux.rs"]
pub mod webview;

/// The napi task that waits for a WebKit JavaScript evaluation.
pub type EvaluateReceiver = oneshot::Receiver<anyhow::Result<String>>;
/// The napi task that waits for a WebView document to become ready.
pub type ReadyReceiver = oneshot::Receiver<anyhow::Result<u64>>;

/// Wrap an expression in an async function so Promise results are awaited and
/// serialize only the documented JSON-compatible value space. Returning the
/// JSON text (rather than an arbitrary native object) keeps the napi
/// boundary deterministic and lets the Solid layer validate it once.
pub(crate) fn evaluation_script(script: &str) -> String {
    format!(
        "const value = await ({script}); if (value === undefined) return \"null\"; const seen = new WeakSet(); return JSON.stringify(value, (_key, item) => {{ if (typeof item === \"number\" && !Number.isFinite(item)) throw new Error(\"unsupported non-finite number\"); if (typeof item === \"undefined\" || typeof item === \"function\" || typeof item === \"symbol\" || typeof item === \"bigint\") throw new Error(\"unsupported JavaScript result\"); if (item !== null && typeof item === \"object\") {{ if (seen.has(item)) throw new Error(\"unsupported cyclic result\"); seen.add(item); }} return item; }});"
    )
}

/// Napi task that waits for the main-thread WebKit callback without blocking
/// the JavaScript thread. Destruction/source changes drain the same channel,
/// so every evaluation promise settles even when WebKit never calls back.
pub struct EvaluateJavaScriptTask {
    receiver: Option<EvaluateReceiver>,
}

/// Napi task that resolves when the intended WebView document has finished
/// loading. The generation is returned for diagnostics; Solid only exposes the
/// readiness promise and guards it against unmount/source changes.
pub struct WebviewReadyTask {
    receiver: Option<ReadyReceiver>,
}

impl WebviewReadyTask {
    pub(crate) fn new(receiver: ReadyReceiver) -> AsyncTask<Self> {
        AsyncTask::new(Self {
            receiver: Some(receiver),
        })
    }
}

#[napi]
impl Task for WebviewReadyTask {
    type Output = u64;
    type JsValue = f64;

    fn compute(&mut self) -> Result<Self::Output> {
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::from_reason("WebView readiness task was already consumed"))?;
        futures::executor::block_on(receiver)
            .map_err(|_| Error::from_reason("WebView readiness channel closed"))?
            .map_err(|error| Error::from_reason(format!("WebView failed to become ready: {error}")))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output as f64)
    }
}

impl EvaluateJavaScriptTask {
    pub(crate) fn new(receiver: EvaluateReceiver) -> AsyncTask<Self> {
        AsyncTask::new(Self {
            receiver: Some(receiver),
        })
    }
}

#[napi]
impl Task for EvaluateJavaScriptTask {
    type Output = String;
    type JsValue = String;

    fn compute(&mut self) -> Result<Self::Output> {
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::from_reason("WebView evaluation task was already consumed"))?;
        futures::executor::block_on(receiver)
            .map_err(|_| Error::from_reason("WebView evaluation channel closed"))?
            .map_err(|error| Error::from_reason(format!("WebView JavaScript failed: {error}")))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// A rectangle in logical points, in the platform's native convention.
///
/// On macOS that means a **bottom-left** origin, which is what
/// `NSView.setFrame:` expects. GPUI measures from the top-left, so every
/// frame is produced by [`to_native_frame`] rather than by hand.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NativeViewFrame {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NativeViewFrame {
    /// Whether the frame has any area.
    ///
    /// A zero-area native view is hidden rather than left mounted: WebKit keeps
    /// compositing layers alive for an empty frame, and an empty WKWebView
    /// still costs a process.
    pub fn is_renderable(&self) -> bool {
        self.width > 0.0 && self.height > 0.0
    }
}

/// Convert a GPUI rect into the platform's native frame convention.
///
/// GPUI lays out from the top-left, AppKit from the bottom-left. Both work in
/// logical points, so no scale factor is involved — `NSView` frames are points,
/// not device pixels. `content_height` must be the height of the view the
/// child is mounted into, i.e. `Window::viewport_size().height`.
pub fn to_native_frame(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content_height: f32,
) -> NativeViewFrame {
    NativeViewFrame {
        x,
        y: content_height - y - height,
        width,
        height,
    }
}

/// A native child view owned by the registry.
///
/// Implementations hold raw platform handles and are `cfg`-gated per platform.
/// Every method runs on the main thread; the registry is thread-local
/// precisely so that holding an AppKit object never implies `Send`.
pub trait NativeViewInstance {
    /// Attach to `host` (an `NSView *` on macOS).
    ///
    /// Called at most once, on the main thread, before any `update_frame`.
    fn mount(&mut self, host: *mut c_void);

    /// Move and/or resize to `frame`.
    ///
    /// Called on every layout pass, so implementations must diff against the
    /// last applied frame instead of assigning unconditionally.
    ///
    /// Visibility is derived from the frame rather than signalled separately:
    /// Solo never skips a subtree during layout, so an element that is
    /// collapsed or clipped always reports a zero-area frame here.
    fn update_frame(&mut self, frame: NativeViewFrame);

    /// Update a content property.
    ///
    /// The webview reads `"url"` and `"userAgent"`; views that only occupy
    /// space ignore every key. A `None` value clears the property.
    ///
    /// Content is separate from geometry so that a reactive URL change never
    /// has to touch layout, and a layout pass never reloads the page.
    fn set_content(&mut self, key: &str, value: Option<&str>);

    /// Apply a WebView's mutually exclusive URL/HTML source as one update.
    fn set_webview_source(
        &mut self,
        url: Option<&str>,
        html: Option<&str>,
        base_url: Option<&str>,
    ) {
        self.set_content("url", url);
        self.set_content("baseUrl", base_url);
        self.set_content("html", html);
    }

    /// Start evaluating JavaScript in this view. WebKit invokes the callback
    /// on the main thread; the receiver is consumed by a napi async task.
    fn evaluate_javascript(
        &mut self,
        script: &str,
    ) -> anyhow::Result<EvaluateReceiver> {
        let _ = script;
        Err(anyhow::anyhow!("This native view does not support JavaScript"))
    }

    /// Wait until the current WebView document has finished loading.
    fn wait_for_ready(&mut self) -> anyhow::Result<ReadyReceiver> {
        Err(anyhow::anyhow!(
            "This native view does not support WebView readiness"
        ))
    }

    /// Enable the navigation-decision path while a Solid listener exists.
    fn set_navigation_interception(&mut self, _enabled: bool) {}

    /// Resolve or cancel a previously emitted navigation request.
    fn decide_navigation(&mut self, _navigation_id: u64, _allow: bool) -> bool {
        false
    }

    /// Detach from the host and release every native object.
    ///
    /// Must be idempotent: [`NativeViewRegistry::destroy`] and
    /// [`NativeViewRegistry::destroy_all`] can both reach the same view.
    fn destroy(&mut self);
}

/// Owns native child views by Solo element ID.
///
/// Platform-neutral: the concrete instances are supplied by the caller, so the
/// bookkeeping below is testable on any platform.
pub struct NativeViewRegistry {
    views: HashMap<u64, Box<dyn NativeViewInstance>>,
}

impl Default for NativeViewRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl NativeViewRegistry {
    pub fn new() -> Self {
        Self {
            views: HashMap::new(),
        }
    }

    /// Ensure the instance for `id` exists, creating and mounting it on first
    /// use.
    ///
    /// `create` runs only when the ID is absent. That is what makes a rerender
    /// reuse the existing WKWebView instead of rebuilding it, and what keeps a
    /// reactive `url` change from constructing a second webview.
    pub fn ensure_mounted<F>(&mut self, id: u64, host: *mut c_void, create: F)
    where
        F: FnOnce() -> Box<dyn NativeViewInstance>,
    {
        self.views.entry(id).or_insert_with(|| {
            let mut instance = create();
            instance.mount(host);
            instance
        });
    }

    pub fn update_frame(&mut self, id: u64, frame: NativeViewFrame) {
        if let Some(view) = self.views.get_mut(&id) {
            view.update_frame(frame);
        }
    }

    pub fn set_content(&mut self, id: u64, key: &str, value: Option<&str>) {
        if let Some(view) = self.views.get_mut(&id) {
            view.set_content(key, value);
        }
    }

    pub fn set_webview_source(
        &mut self,
        id: u64,
        url: Option<&str>,
        html: Option<&str>,
        base_url: Option<&str>,
    ) {
        if let Some(view) = self.views.get_mut(&id) {
            view.set_webview_source(url, html, base_url);
        }
    }

    pub fn evaluate_javascript(
        &mut self,
        id: u64,
        script: &str,
    ) -> anyhow::Result<EvaluateReceiver> {
        self.views
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("WebView {id} is not mounted"))?
            .evaluate_javascript(script)
    }

    pub fn wait_for_ready(
        &mut self,
        id: u64,
    ) -> anyhow::Result<ReadyReceiver> {
        self.views
            .get_mut(&id)
            .ok_or_else(|| anyhow::anyhow!("WebView {id} is not mounted"))?
            .wait_for_ready()
    }

    pub fn set_navigation_interception(&mut self, id: u64, enabled: bool) {
        if let Some(view) = self.views.get_mut(&id) {
            view.set_navigation_interception(enabled);
        }
    }

    pub fn decide_navigation(&mut self, id: u64, navigation_id: u64, allow: bool) -> bool {
        self.views
            .get_mut(&id)
            .is_some_and(|view| view.decide_navigation(navigation_id, allow))
    }

    /// Detach, release, and forget `id`. A no-op if it was never mounted.
    pub fn destroy(&mut self, id: u64) {
        if let Some(mut view) = self.views.remove(&id) {
            view.destroy();
        }
    }

    /// Release every view. Called when the app quits so nothing outlives the
    /// window it was mounted into.
    pub fn destroy_all(&mut self) {
        for (_, mut view) in self.views.drain() {
            view.destroy();
        }
    }
}

thread_local! {
    static NATIVE_VIEWS: RefCell<NativeViewRegistry> = RefCell::new(NativeViewRegistry::new());
}

/// Run `f` against this thread's native-view registry.
///
/// Thread-local on purpose: `NativeViewInstance` owns AppKit objects that are
/// only legal to touch on the main thread. Keeping the registry thread-local
/// means a stray `Send` shows up at the boundary instead of as a WebKit
/// crash. GPUI's `prepaint` and Solo's element `destroy` both run on the main
/// thread, so this is always the right instance.
pub fn with_registry<R>(f: impl FnOnce(&mut NativeViewRegistry) -> R) -> R {
    NATIVE_VIEWS.with(|cell| f(&mut cell.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    // ── Coordinate conversion ──────────────────────────────────────────
    //
    // These are the load-bearing assertions for the whole bridge: GPUI lays
    // out from the top-left, AppKit frames from the bottom-left. Getting this
    // wrong puts the webview off-screen or vertically mirrored, and there is
    // no way to catch that from a screenshot on a machine without a GPUI
    // window — so it is tested as pure arithmetic instead.

    #[test]
    fn flips_the_y_axis_from_gpui_to_appkit() {
        // A 200x100 box at the top-left of an 800pt-tall window sits 700pt
        // up from the AppKit origin.
        assert_eq!(
            to_native_frame(0.0, 0.0, 200.0, 100.0, 800.0),
            NativeViewFrame {
                x: 0.0,
                y: 700.0,
                width: 200.0,
                height: 100.0
            }
        );
    }

    #[test]
    fn a_box_at_the_bottom_of_gpui_space_maps_to_appkit_zero() {
        assert_eq!(to_native_frame(0.0, 700.0, 200.0, 100.0, 800.0).y, 0.0);
    }

    #[test]
    fn a_full_height_box_starts_at_appkit_zero() {
        assert_eq!(to_native_frame(0.0, 0.0, 600.0, 800.0, 800.0).y, 0.0);
    }

    #[test]
    fn conversion_preserves_x_and_size() {
        let frame = to_native_frame(37.5, 12.0, 640.0, 480.0, 900.0);
        assert_eq!(frame.x, 37.5);
        assert_eq!(frame.width, 640.0);
        assert_eq!(frame.height, 480.0);
    }

    #[test]
    fn conversion_is_its_own_inverse() {
        // Flipping twice returns the original GPSUI y.
        let flipped = to_native_frame(10.0, 120.0, 200.0, 50.0, 600.0);
        let back = to_native_frame(flipped.x, flipped.y, flipped.width, flipped.height, 600.0);
        assert_eq!(back.y, 120.0);
    }

    #[test]
    fn zero_area_frames_are_not_renderable() {
        assert!(!NativeViewFrame::default().is_renderable());
        assert!(!NativeViewFrame {
            width: 100.0,
            height: 0.0,
            ..Default::default()
        }
        .is_renderable());
        assert!(NativeViewFrame {
            width: 100.0,
            height: 1.0,
            ..Default::default()
        }
        .is_renderable());
    }

    // ── Registry bookkeeping ───────────────────────────────────────────

    #[derive(Default)]
    struct Log {
        mounts: u32,
        frames: Vec<NativeViewFrame>,
        content: Vec<(String, Option<String>)>,
        destroys: u32,
    }

    /// Stand-in for the macOS WKWebView instance so the registry's lifecycle
    /// guarantees are testable off macOS.
    struct RecordingView {
        log: Rc<RefCell<Log>>,
    }

    impl NativeViewInstance for RecordingView {
        fn mount(&mut self, _host: *mut c_void) {
            self.log.borrow_mut().mounts += 1;
        }
        fn update_frame(&mut self, frame: NativeViewFrame) {
            self.log.borrow_mut().frames.push(frame);
        }
        fn set_content(&mut self, key: &str, value: Option<&str>) {
            self.log
                .borrow_mut()
                .content
                .push((key.to_string(), value.map(str::to_string)));
        }
        fn destroy(&mut self) {
            self.log.borrow_mut().destroys += 1;
        }
    }

    fn recorder() -> (Rc<RefCell<Log>>, impl Fn() -> Box<dyn NativeViewInstance>) {
        let log = Rc::new(RefCell::new(Log::default()));
        let seen = Rc::clone(&log);
        (log, move || {
            Box::new(RecordingView {
                log: Rc::clone(&seen),
            })
        })
    }

    #[test]
    fn repeated_ensure_mounted_mounts_only_once() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();

        // Three layout passes over the same element must not rebuild the view.
        for _ in 0..3 {
            registry.ensure_mounted(7, std::ptr::null_mut(), &make);
        }

        assert_eq!(log.borrow().mounts, 1);
    }

    #[test]
    fn distinct_ids_get_distinct_instances() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(1, std::ptr::null_mut(), &make);
        registry.ensure_mounted(2, std::ptr::null_mut(), &make);
        assert_eq!(log.borrow().mounts, 2);
    }

    #[test]
    fn update_frame_reaches_the_instance() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(7, std::ptr::null_mut(), &make);

        let frame = to_native_frame(0.0, 0.0, 100.0, 50.0, 500.0);
        registry.update_frame(7, frame);

        assert_eq!(log.borrow().frames, vec![frame]);
    }

    #[test]
    fn content_updates_reach_the_instance_and_can_be_cleared() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(7, std::ptr::null_mut(), &make);

        registry.set_content(7, "url", Some("https://example.com"));
        registry.set_content(7, "userAgent", None);

        assert_eq!(
            log.borrow().content,
            vec![
                ("url".to_string(), Some("https://example.com".to_string())),
                ("userAgent".to_string(), None),
            ]
        );
    }

    #[test]
    fn updates_for_an_unmounted_id_are_ignored() {
        // Mirrors Rust's lenient retained-tree ops: unknown IDs are a no-op,
        // not a panic. Recording is shared with a mounted view so the
        // assertions prove the updates went nowhere, not merely that the
        // recorder was never wired up.
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(7, std::ptr::null_mut(), &make);

        registry.update_frame(999, NativeViewFrame::default());
        registry.set_content(999, "url", Some("https://example.com"));

        assert_eq!(log.borrow().frames.len(), 0);
        assert_eq!(log.borrow().content.len(), 0);
    }

    #[test]
    fn destroy_releases_and_forgets() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(7, std::ptr::null_mut(), &make);

        registry.destroy(7);
        assert_eq!(log.borrow().destroys, 1);

        // Forgotten, not merely hidden: a later frame for the same id reaches
        // nothing. If it still held the old instance this would mount a second
        // one and start recording frames again.
        registry.update_frame(7, NativeViewFrame::default());
        assert_eq!(log.borrow().frames.len(), 0);
        assert_eq!(log.borrow().mounts, 1);
    }

    #[test]
    fn destroying_an_unknown_id_is_a_noop() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(7, std::ptr::null_mut(), &make);

        registry.destroy(42);

        assert_eq!(log.borrow().destroys, 0);
        // The real view is untouched.
        registry.update_frame(7, NativeViewFrame::default());
        assert_eq!(log.borrow().frames.len(), 1);
    }

    #[test]
    fn destroy_all_releases_every_view() {
        let (log, make) = recorder();
        let mut registry = NativeViewRegistry::new();
        registry.ensure_mounted(1, std::ptr::null_mut(), &make);
        registry.ensure_mounted(2, std::ptr::null_mut(), &make);
        registry.ensure_mounted(3, std::ptr::null_mut(), &make);

        registry.destroy_all();

        assert_eq!(log.borrow().destroys, 3);

        // Every id is gone: each of these would otherwise mount a fresh view.
        for id in [1, 2, 3] {
            registry.update_frame(id, NativeViewFrame::default());
        }
        assert_eq!(log.borrow().frames.len(), 0);
        assert_eq!(log.borrow().mounts, 3);
    }

    #[test]
    fn with_registry_exposes_the_same_registry_across_calls() {
        with_registry(|r| {
            r.update_frame(12345, NativeViewFrame::default());
        });
        // Nothing mounted, so the update was dropped rather than panicking.
        with_registry(|r| {
            r.update_frame(12345, NativeViewFrame::default());
        });
    }
}
