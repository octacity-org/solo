//! Connected screens, and the screen a new window opens on (macOS).
//!
//! A screen's `id` is its display UUID: it survives reconnects and restarts,
//! while GPUI's numeric `DisplayId` may not. Rectangles are what GPUI reports,
//! in logical points measured from the screen's own top-left corner, so
//! `bounds` starts at (0, 0) and `visibleBounds` is offset by the menu bar and
//! Dock. Display changes reach JS as an element-0 `screensChanged` event.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use napi::{Error, Result};
use napi_derive::napi;

/// A rectangle in logical points.
#[derive(Debug, Clone, PartialEq)]
#[napi(object)]
pub struct ScreenRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
#[napi(object)]
pub struct Screen {
    /// Stable identifier (the display UUID). Pass it as the `screen` window
    /// option, or store it to restore a window on the same screen.
    pub id: String,
    /// The whole screen, starting at (0, 0).
    pub bounds: ScreenRect,
    /// The area windows can use: the screen without the menu bar and Dock.
    pub visible_bounds: ScreenRect,
    /// The screen holding the menu bar.
    pub primary: bool,
}

fn rect(bounds: gpui::Bounds<gpui::Pixels>) -> ScreenRect {
    ScreenRect {
        x: f64::from(f32::from(bounds.origin.x)),
        y: f64::from(f32::from(bounds.origin.y)),
        width: f64::from(f32::from(bounds.size.width)),
        height: f64::from(f32::from(bounds.size.height)),
    }
}

/// The connected screens. A display without a UUID cannot be targeted later,
/// so it is left out.
pub(crate) fn list(cx: &gpui::App) -> Vec<Screen> {
    let primary = cx.primary_display().map(|display| display.id());
    cx.displays()
        .iter()
        .filter_map(|display| {
            Some(Screen {
                id: display.uuid().ok()?.to_string(),
                bounds: rect(display.bounds()),
                visible_bounds: rect(display.visible_bounds()),
                primary: Some(display.id()) == primary,
            })
        })
        .collect()
}

/// The connected display whose UUID is `screen`, if any.
pub(crate) fn find(screen: &str, cx: &gpui::App) -> Option<gpui::DisplayId> {
    cx.displays()
        .iter()
        .find(|display| {
            display
                .uuid()
                .is_ok_and(|uuid| uuid.to_string().eq_ignore_ascii_case(screen))
        })
        .map(|display| display.id())
}

/// The display for a window's `screen` option: `None` keeps the main screen,
/// and an id no connected screen has is an error.
pub(crate) fn display_for(screen: Option<&str>, cx: &gpui::App) -> Result<Option<gpui::DisplayId>> {
    screen
        .map(|screen| {
            find(screen, cx).ok_or_else(|| {
                Error::from_reason(format!(
                    "No connected screen has id {screen:?}; getScreens() lists the connected screens"
                ))
            })
        })
        .transpose()
}

/// Element-0 event: a screen was connected or disconnected, or changed size.
#[cfg(target_os = "macos")]
const SCREENS_CHANGED_EVENT: &str = "screensChanged";

/// Report display changes to the current app renderer for the life of the
/// process. Installed once in `run_main_loop`, so a `solo dev` reload keeps it.
#[cfg(target_os = "macos")]
pub(crate) fn observe(cx: &mut gpui::App) {
    cx.observe_displays(|_event, _cx| {
        crate::renderer::emit_event_full(
            &crate::renderer::app_event_callback(),
            0,
            SCREENS_CHANGED_EVENT,
            |_| {},
        );
    })
    .detach();
}

/// The connected screens (macOS). Ids are stable display UUIDs.
#[napi]
pub fn get_screens() -> Result<Vec<Screen>> {
    #[cfg(target_os = "macos")]
    {
        if crate::renderer::worker_main::must_forward() {
            return crate::renderer::on_main(|| crate::renderer::app_update(|cx| list(cx)));
        }
        // `TestSoloRenderer` runs GPUI on this thread.
        #[cfg(feature = "test-support")]
        if let Some(screens) = crate::test_renderer::with_app(list) {
            return Ok(screens);
        }
        Err(Error::from_reason("getScreens needs a running Solo app"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::from_reason("Listing screens is supported only on macOS"))
    }
}
