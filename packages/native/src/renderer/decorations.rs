//! Client-side window decorations for Linux.
//!
//! Wayland compositors without `xdg-decoration` (GNOME) hand the titlebar to
//! the application, and X11 does too when a compositor is around and the app
//! asked for client decorations. GPUI reports that through
//! [`gpui::Window::window_decorations`] and draws nothing itself, so Solo
//! paints a minimal titlebar and resize edges; a server-decorated window keeps
//! its window manager's and renders exactly as before.

use gpui::prelude::*;
use gpui::{
    px, AnyElement, CursorStyle, Decorations, MouseButton, ResizeEdge, SharedString, Tiling,
    Window, WindowButton, WindowButtonLayout, WindowControls,
};

use super::view::SoloView;
use crate::theme::Theme;

/// Height of the drawn titlebar.
const TITLEBAR_HEIGHT: f32 = 34.0;
/// Thickness of a resize edge strip.
const EDGE: f32 = 6.0;
/// Side of a corner handle: wide enough to hit, narrow enough to leave the
/// edges between corners.
const CORNER: f32 = 14.0;
/// The titlebar drag region's element ID. The app's retained elements use small
/// integers from 1, so this cannot collide with them.
const DRAG_ID: u64 = u64::MAX;

/// Wrap `content` with Solo's titlebar and resize edges when the window is
/// client-decorated.
///
/// `layout` is `App::button_layout()`: the desktop's own button order, so a
/// titlebar lands where the user expects it.
pub(super) fn decorate(
    view: &SoloView,
    content: AnyElement,
    theme: &Theme,
    layout: Option<WindowButtonLayout>,
    window: &Window,
) -> AnyElement {
    let tiling = match window.window_decorations() {
        Decorations::Client { tiling } => tiling,
        Decorations::Server => return content,
    };

    log::debug!(
        "client decorations: titlebar_hidden={}, fullscreen={}, resizable={}",
        view.titlebar_hidden,
        window.is_fullscreen(),
        window.is_resizable()
    );

    let mut frame = gpui::div().relative().size_full();
    frame = if view.titlebar_hidden || window.is_fullscreen() {
        // The app draws its own titlebar, or the window is fullscreen and has
        // none. Resize edges still apply to a client-decorated window.
        frame.child(content)
    } else {
        frame
            .flex()
            .flex_col()
            .child(titlebar(
                view,
                theme,
                layout.unwrap_or_else(WindowButtonLayout::linux_default),
                window,
            ))
            .child(gpui::div().flex_1().min_h_0().w_full().child(content))
    };

    frame
        .children(resize_handles(window.is_resizable(), tiling))
        .into_any_element()
}

/// The default titlebar: window buttons where the desktop puts them, and the
/// window title in a drag region between them.
fn titlebar(
    view: &SoloView,
    theme: &Theme,
    layout: WindowButtonLayout,
    window: &Window,
) -> AnyElement {
    let controls = window.window_controls();

    let drag = super::build::window_drag_region(
        gpui::div()
            .id(DRAG_ID)
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .px(px(10.0))
            .text_color(theme.text_muted)
            .text_size(px(12.5))
            .child(crate::text::chrome_text(
                SharedString::from(view.native_title()),
                None,
            )),
        DRAG_ID,
    );

    let mut bar = gpui::div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w_full()
        .h(px(TITLEBAR_HEIGHT))
        .bg(theme.bg)
        .border_b_1()
        .border_color(theme.border);

    for button in layout.left.into_iter().flatten() {
        if let Some(element) = control_button(button, view, theme, controls, window) {
            bar = bar.child(element);
        }
    }
    bar = bar.child(drag);
    for button in layout.right.into_iter().flatten() {
        if let Some(element) = control_button(button, view, theme, controls, window) {
            bar = bar.child(element);
        }
    }

    bar.into_any_element()
}

/// One window control button, or `None` where the platform or the window's own
/// options say that control is unavailable.
fn control_button(
    button: WindowButton,
    view: &SoloView,
    theme: &Theme,
    controls: WindowControls,
    window: &Window,
) -> Option<AnyElement> {
    let available = match button {
        WindowButton::Minimize => controls.minimize && window.is_minimizable(),
        WindowButton::Maximize => controls.maximize && window.is_resizable(),
        WindowButton::Close => true,
    };
    if !available {
        return None;
    }

    let (id, glyph) = match button {
        WindowButton::Minimize => ("solo-titlebar-minimize", "\u{2013}"),
        WindowButton::Maximize => ("solo-titlebar-maximize", "\u{25a1}"),
        WindowButton::Close => ("solo-titlebar-close", "\u{2715}"),
    };
    let hover = match button {
        WindowButton::Close => gpui::hsla(0.0, 0.75, 0.5, 0.9),
        _ => theme.border,
    };

    let guarded = view.tab().close_guarded;
    let callback = view.event_callback.clone();
    let root = view.root_for_events();

    Some(
        gpui::div()
            .id(id)
            .h_full()
            .w(px(46.0))
            .flex()
            .items_center()
            .justify_center()
            .text_color(theme.text_muted)
            .text_size(px(12.0))
            .hover(move |style| style.bg(hover))
            .child(crate::text::chrome_text(SharedString::new_static(glyph), None))
            .on_click(move |_event, window, _cx| match button {
                WindowButton::Minimize => window.minimize_window(),
                WindowButton::Maximize => window.zoom_window(),
                WindowButton::Close => {
                    if guarded {
                        super::emit_event_full(
                            &callback,
                            root,
                            super::WINDOW_CLOSE_REQUESTED_EVENT,
                            |_| {},
                        );
                    } else {
                        window.remove_window();
                    }
                }
            })
            .into_any_element(),
    )
}

/// Resize strips around a client-decorated window. A side the window manager
/// still owns (`tiling`) gets none, so a maximized or tiled window is not
/// offered an edge that would do nothing.
fn resize_handles(resizable: bool, tiling: Tiling) -> Vec<AnyElement> {
    if !resizable {
        return Vec::new();
    }

    let mut handles = Vec::new();

    if !tiling.top {
        handles.push(handle(
            "solo-resize-top",
            ResizeEdge::Top,
            CursorStyle::ResizeUp,
            |el| {
                el.top_0()
                    .left(px(CORNER))
                    .right(px(CORNER))
                    .h(px(EDGE))
            },
        ));
        handles.push(corner_handle(
            "solo-resize-top-left",
            ResizeEdge::TopLeft,
            CursorStyle::ResizeUpLeftDownRight,
            |el| el.top_0().left_0(),
        ));
        handles.push(corner_handle(
            "solo-resize-top-right",
            ResizeEdge::TopRight,
            CursorStyle::ResizeUpRightDownLeft,
            |el| el.top_0().right_0(),
        ));
    }
    if !tiling.bottom {
        handles.push(handle(
            "solo-resize-bottom",
            ResizeEdge::Bottom,
            CursorStyle::ResizeDown,
            |el| {
                el.bottom_0()
                    .left(px(CORNER))
                    .right(px(CORNER))
                    .h(px(EDGE))
            },
        ));
        handles.push(corner_handle(
            "solo-resize-bottom-left",
            ResizeEdge::BottomLeft,
            CursorStyle::ResizeUpRightDownLeft,
            |el| el.bottom_0().left_0(),
        ));
        handles.push(corner_handle(
            "solo-resize-bottom-right",
            ResizeEdge::BottomRight,
            CursorStyle::ResizeUpLeftDownRight,
            |el| el.bottom_0().right_0(),
        ));
    }
    if !tiling.left {
        handles.push(handle(
            "solo-resize-left",
            ResizeEdge::Left,
            CursorStyle::ResizeLeft,
            |el| {
                el.left_0()
                    .top(px(CORNER))
                    .bottom(px(CORNER))
                    .w(px(EDGE))
            },
        ));
    }
    if !tiling.right {
        handles.push(handle(
            "solo-resize-right",
            ResizeEdge::Right,
            CursorStyle::ResizeRight,
            |el| {
                el.right_0()
                    .top(px(CORNER))
                    .bottom(px(CORNER))
                    .w(px(EDGE))
            },
        ));
    }

    handles
}

fn corner_handle(
    id: &'static str,
    edge: ResizeEdge,
    cursor: CursorStyle,
    place: impl FnOnce(gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div>,
) -> AnyElement {
    handle(id, edge, cursor, move |el| {
        place(el).w(px(CORNER)).h(px(CORNER))
    })
}

/// One resize strip: absolutely positioned over the window, and starting a
/// native resize on press. The window manager drives the drag from there, so
/// the pointer may leave the strip at once.
fn handle(
    id: &'static str,
    edge: ResizeEdge,
    cursor: CursorStyle,
    place: impl FnOnce(gpui::Stateful<gpui::Div>) -> gpui::Stateful<gpui::Div>,
) -> AnyElement {
    place(gpui::div().id(id).absolute())
        .cursor(cursor)
        .on_mouse_down(MouseButton::Left, move |_event, window, _cx| {
            window.start_window_resize(edge);
        })
        .into_any_element()
}
