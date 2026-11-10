//! The tab strip for Linux windows that share a tabbing identifier.
//!
//! Neither X11 nor Wayland groups windows into tabs; GTK and libadwaita apps
//! draw their own (`AdwTabView`). Solo does the same: windows opened with the
//! same `tabbingIdentifier` join one native window, which paints a strip above
//! the selected tab's tree. Every tab keeps the window API it had as a native
//! window of its own — `setTitle`, document state, close requests, events.

use gpui::prelude::*;
use gpui::{px, AnyElement, ClickEvent, SharedString};

use super::view::SoloView;
use crate::theme::Theme;

/// Height of the strip, above the tab's content.
const STRIP_HEIGHT: f32 = 30.0;
/// Element ids of the strip's tabs and their close buttons. The app's retained
/// elements use small integers from 1 and the titlebar's drag region uses
/// `u64::MAX`, so this range is free.
const TAB_ID_BASE: u64 = u64::MAX - 2;

/// `content` under a strip of `view`'s tabs.
pub(super) fn tabbed(
    view: &SoloView,
    content: AnyElement,
    theme: &Theme,
    cx: &mut gpui::Context<SoloView>,
) -> AnyElement {
    let mut strip = gpui::div()
        .flex()
        .flex_row()
        .flex_none()
        .w_full()
        .h(px(STRIP_HEIGHT))
        .bg(theme.bg)
        .border_b(px(1.0))
        .border_color(theme.border);

    for (index, tab) in view.tabs.iter().enumerate() {
        // The main window's tree root is never a tab: a tabbed window is one
        // `openWindow` created.
        let Some(root) = tab.root else { continue };
        strip = strip.child(entry(
            root,
            tab.native_title(),
            index == view.active,
            theme,
            cx,
        ));
    }

    gpui::div()
        .flex()
        .flex_col()
        .size_full()
        .child(strip)
        .child(gpui::div().flex_1().min_h_0().w_full().child(content))
        .into_any_element()
}

/// One tab: its title, which selects it, and a close button that follows that
/// tab's close request.
fn entry(
    root: u64,
    title: String,
    active: bool,
    theme: &Theme,
    cx: &mut gpui::Context<SoloView>,
) -> AnyElement {
    let id = TAB_ID_BASE - root * 2;
    let select = cx.listener(move |view: &mut SoloView, _event: &ClickEvent, window, cx| {
        view.activate_tab(root);
        super::refresh_view(window, cx);
    });
    let close = cx.listener(move |view: &mut SoloView, _event: &ClickEvent, window, cx| {
        // The tab under the button would otherwise select it too.
        cx.stop_propagation();
        // Closing one tab is not the window-level close a previous ✕ may have
        // started, so do not let this confirmation finish that one.
        view.clear_pending_close();
        // A tab that guards its close asks instead of closing, as its native
        // window would.
        if view.request_close(super::WindowTarget::Root(root)) {
            super::close_tab_on_ui(view, root, window, cx);
        } else {
            super::refresh_view(window, cx);
        }
    });

    gpui::div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .h_full()
        .max_w(px(220.0))
        .pl(px(12.0))
        .pr(px(4.0))
        .gap(px(6.0))
        .bg(theme.bg)
        .border_r(px(1.0))
        .border_color(theme.border)
        .text_color(if active { theme.text } else { theme.text_muted })
        .text_size(px(12.0))
        .child(crate::text::chrome_text(
            SharedString::from(title),
            None,
        ))
        .child(
            gpui::div()
                .id(id - 1)
                .flex()
                .items_center()
                .justify_center()
                .w(px(20.0))
                .h(px(20.0))
                .rounded(px(4.0))
                .hover(move |style| style.bg(theme.border))
                .child(crate::text::chrome_text(
                    SharedString::new_static("\u{2715}"),
                    None,
                ))
                .on_click(close),
        )
        .on_click(select)
        .into_any_element()
}
