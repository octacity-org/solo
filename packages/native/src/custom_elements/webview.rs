//! `<webview>` — a native web view embedded in a Solo window.
//!
//! The view does not paint web content. It returns a GPUI placeholder that
//! takes part in normal layout, and hands its post-layout bounds to the
//! native-view registry, which owns the real native view: a WKWebView (an
//! AppKit NSView) on macOS, or a WebKitGTK child of the GPUI X11 window on
//! Linux. GPUI keeps ownership of layout; AppKit/WebKit keep ownership of
//! pixels and input.
//!
//! ```text
//! <webview url=…>
//!   → WebviewElement (CustomElement: props, events, destroy)
//!     → WebviewPlaceholder (gpui::Element: layout + bounds capture)
//!       → prepaint(bounds)
//!         → native_view::with_registry(… update_frame …)
//!           → NSView.setFrame: (macOS) / XMoveResizeWindow (Linux)
//! ```
use std::cell::RefCell;

use gpui::prelude::*;
use gpui::{
    App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement, LayoutId,
    Pixels, Style, Window,
};

use crate::custom_elements::{CustomElement, CustomElementFactory, CustomRenderContext};
#[cfg(target_os = "macos")]
use crate::native_view::to_native_frame;
use crate::renderer::EventCallback;

/// Reserve the element's box. `relative(1.)` on both axes means the placeholder
/// fills whatever the retained tree's style gave it, so `<webview>` honours
/// `width: "100%"` and `flexGrow` like any other element.
fn placeholder_style() -> Style {
    let mut style = Style::default();
    style.size.width = gpui::relative(1.).into();
    style.size.height = gpui::relative(1.).into();
    // Flex items cannot shrink below their content without an explicit zero
    // minimum; a webview has no intrinsic size, so this is always safe.
    style.min_size.width = gpui::px(0.).into();
    style.min_size.height = gpui::px(0.).into();
    style.flex_grow = 1.0;
    style.flex_shrink = 1.0;
    style
}

thread_local! {
    /// Elements that already reported their unsupported platform, so a
    /// persistent Wayland window does not spam an event every frame.
    static REPORTED_UNSUPPORTED: RefCell<std::collections::HashSet<u64>> =
        RefCell::new(std::collections::HashSet::new());
}

/// The error for a Wayland window, delivered as the `loadError` event (once
/// per element) and the log, since a placeholder cannot throw from inside a
/// draw.
pub(crate) const WAYLAND_ERROR: &str =
    "<webview> is not supported on Wayland; it embeds a WebKitGTK window, which needs X11";

/// The Wayland error, delivered as the `loadError` event (once per element)
/// and the log, since a placeholder cannot throw from inside a draw.
fn report_unsupported(element_id: u64, callback: &Option<EventCallback>) {
    let first = REPORTED_UNSUPPORTED.with(|reported| reported.borrow_mut().insert(element_id));
    if !first {
        return;
    }
    log::error!("webview: {WAYLAND_ERROR}");
    crate::renderer::emit_event_full(callback, element_id, "loadError", |payload| {
        payload.value = Some(WAYLAND_ERROR.to_string());
    });
}

/// The GPUI element that occupies layout space for a WKWebView.
///
/// Created fresh by `WebviewElement::render` every frame and discarded after
/// paint; all durable state lives in the registry, keyed by element ID.
struct WebviewPlaceholder {
    element_id: u64,
    url: Option<String>,
    html: Option<String>,
    base_url: Option<String>,
    user_agent: Option<String>,
    intercept_navigation: bool,
    callback: Option<EventCallback>,
}

/// Which kind of host window GPUI gave this placeholder.
///
/// macOS mounts under the `GPUIView` (an `NSView *`) above GPUI's layer.
/// Linux mounts a WebKitGTK child under the X window: its XID is server-wide,
/// so the webview thread can parent a child under it on its own display —
/// `mount` is handed it as an opaque pointer and copies the id straight out.
/// A Wayland window has no such handle: embedding another toolkit's surface
/// is impossible there, so the placeholder reports an error instead of
/// mounting anything.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Host {
    AppKit(*mut std::ffi::c_void),
    X11(u64),
    Wayland,
    Unavailable,
}

impl WebviewPlaceholder {
    /// Resolve the host for Solo's GPUI window.
    ///
    /// Uses the public `HasWindowHandle` impl rather than reaching into
    /// GPUI's private window types, so this needs no changes to the pinned
    /// GPUI fork. The trait method has to be called explicitly: `Window` has
    /// an inherent `window_handle()` that returns GPUI's own
    /// `AnyWindowHandle` and would otherwise shadow it.
    fn host(window: &Window) -> Host {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};

        let Ok(handle) = HasWindowHandle::window_handle(window) else {
            return Host::Unavailable;
        };
        match handle.as_raw() {
            RawWindowHandle::AppKit(appkit) => Host::AppKit(appkit.ns_view.as_ptr()),
            RawWindowHandle::Xcb(xcb) => Host::X11(xcb.window.get() as u64),
            RawWindowHandle::Wayland(_) => Host::Wayland,
            _ => Host::Unavailable,
        }
    }
}

impl Element for WebviewPlaceholder {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        (window.request_layout(placeholder_style(), None, cx), ())
    }

    /// The one place with authoritative post-layout bounds.
    ///
    /// GPUI runs `request_layout → prepaint → paint` inside `Window::draw`, and
    /// `bounds` here is already in window space. This is also inside a redraw
    /// triggered by a window resize (`bounds_changed` calls `refresh`), so
    /// resizing needs no extra plumbing.
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        _: &mut App,
    ) -> Self::PrepaintState {
        // The native view swallows mouse events in its own rect. Claim the
        // same rect in GPUI's hit-test bookkeeping so a click over the
        // webview is not routed to an element underneath.
        window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal);

        let host = Self::host(window);
        let element_id = self.element_id;
        let callback = self.callback.clone();
        match host {
            Host::Wayland => {
                report_unsupported(element_id, &callback);
                return;
            }
            Host::Unavailable => {
                log::warn!("webview: window has no native handle yet");
                return;
            }
            Host::AppKit(_) | Host::X11(_) => {}
        }

        // macOS: AppKit frames are bottom-left points, so flip. Linux: X is
        // top-left physical pixels, so scale and keep the origin as is.
        let frame = {
            let x = f32::from(bounds.origin.x);
            let y = f32::from(bounds.origin.y);
            let width = f32::from(bounds.size.width);
            let height = f32::from(bounds.size.height);
            #[cfg(target_os = "macos")]
            {
                let content_height = window.viewport_size().height;
                to_native_frame(x, y, width, height, f32::from(content_height))
            }
            #[cfg(target_os = "linux")]
            {
                let scale = window.scale_factor();
                crate::native_view::NativeViewFrame {
                    x: x * scale,
                    y: y * scale,
                    width: width * scale,
                    height: height * scale,
                }
            }
        };

        // On Linux the host pointer carries the parent X window ID (see
        // `native_view/webview_linux.rs`): the mount reads it immediately.
        let host_ptr = match host {
            Host::AppKit(ptr) => ptr,
            Host::X11(id) => id as *mut std::ffi::c_void,
            Host::Wayland | Host::Unavailable => std::ptr::null_mut(),
        };

        let url = self.url.clone();
        let html = self.html.clone();
        let base_url = self.base_url.clone();
        let user_agent = self.user_agent.clone();
        let intercept_navigation = self.intercept_navigation;

        crate::native_view::with_registry(|registry| {
            registry.ensure_mounted(element_id, host_ptr, || {
                #[cfg(target_os = "macos")]
                {
                    Box::new(crate::native_view::webview::MacWebView::new(
                        element_id, callback,
                    )) as Box<dyn crate::native_view::NativeViewInstance>
                }
                #[cfg(target_os = "linux")]
                {
                    Box::new(crate::native_view::webview::LinuxWebView::new(
                        element_id, callback,
                    )) as Box<dyn crate::native_view::NativeViewInstance>
                }
            });
            registry.update_frame(element_id, frame);
            registry.set_navigation_interception(element_id, intercept_navigation);
            registry.set_webview_source(
                element_id,
                url.as_deref(),
                html.as_deref(),
                base_url.as_deref(),
            );
            registry.set_content(element_id, "userAgent", user_agent.as_deref());
        });
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        _: &mut Window,
        _: &mut App,
    ) {}
}

impl IntoElement for WebviewPlaceholder {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

// ── The custom element ───────────────────────────────────────────────

/// Retained-side state for `<webview>`.
///
/// Mirrors the `input` element's shape: the struct owns durable props, and each
/// frame pushes them into the placeholder.
pub struct WebviewElement {
    id: u64,
    url: Option<String>,
    html: Option<String>,
    base_url: Option<String>,
    user_agent: Option<String>,
    /// Emitted events carry the element ID, so it is captured at construction.
    callback: Option<EventCallback>,
}

impl WebviewElement {
    fn new(id: u64) -> Self {
        Self {
            id,
            url: None,
            html: None,
            base_url: None,
            user_agent: None,
            callback: None,
        }
    }
}

impl CustomElement for WebviewElement {
    fn render(
        &mut self,
        ctx: CustomRenderContext,
        _window: &mut Window,
        _cx: &mut gpui::Context<crate::renderer::SoloView>,
    ) -> gpui::AnyElement {
        self.callback = ctx.event_callback.clone();

        // The style from JSX (flexGrow, minHeight, …) is applied to the wrapper
        // so `<webview>` sizes like any other Solo element, without needing a
        // StyleDesc → gpui::Style conversion of our own.
        let mut wrapper = gpui::div();
        if let Some(style) = ctx.style {
            wrapper = crate::renderer::apply_styles(wrapper, style);
        }
        if ctx
            .style
            .and_then(|style| style.position.as_deref())
            .is_none()
        {
            wrapper = wrapper.relative();
        }

        wrapper
            .child(crate::automation::bounds_tracker(self.id))
            .child(WebviewPlaceholder {
                element_id: self.id,
                url: self.url.clone(),
                html: self.html.clone(),
                base_url: self.base_url.clone(),
                user_agent: self.user_agent.clone(),
                intercept_navigation: ctx.events.contains("navigationRequest"),
                callback: self.callback.clone(),
            })
            .into_any_element()
    }

    fn set_prop(&mut self, key: &str, value: serde_json::Value) {
        let as_string = |value: serde_json::Value| {
            if value.is_null() {
                None
            } else {
                value.as_str().map(str::to_string)
            }
        };
        match key {
            "url" => self.url = as_string(value),
            "html" => self.html = as_string(value),
            "baseUrl" => self.base_url = as_string(value),
            "userAgent" => self.user_agent = as_string(value),
            _ => {}
        }
    }

    fn supported_props(&self) -> &[&str] {
        &["url", "html", "baseUrl", "userAgent"]
    }

    fn get_prop(&self, key: &str) -> Option<serde_json::Value> {
        match key {
            "url" => self
                .url
                .as_ref()
                .map(|u| serde_json::Value::String(u.clone())),
            "html" => self
                .html
                .as_ref()
                .map(|u| serde_json::Value::String(u.clone())),
            "baseUrl" => self
                .base_url
                .as_ref()
                .map(|u| serde_json::Value::String(u.clone())),
            "userAgent" => self
                .user_agent
                .as_ref()
                .map(|u| serde_json::Value::String(u.clone())),
            _ => None,
        }
    }

    fn supported_events(&self) -> &[&str] {
        &["load", "navigation", "navigationRequest", "loadError"]
    }

    fn destroy(&mut self) {
        // The only cleanup path that matters: the registry owns the WKWebView,
        // so releasing it here detaches the AppKit view. Reached on unmount via
        // `prune_missing` and on app quit via `destroy_all`.
        crate::native_view::with_registry(|registry| registry.destroy(self.id));
        self.callback = None;
    }
}

pub struct WebviewFactory;

impl CustomElementFactory for WebviewFactory {
    fn element_type(&self) -> &str {
        "webview"
    }

    fn create(&self, id: u64) -> Box<dyn CustomElement> {
        Box::new(WebviewElement::new(id))
    }
}
