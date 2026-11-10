//! GPUI-drawn context menus on Linux.
//!
//! `context_menu` runs an AppKit helper process because an open `NSMenu` is a
//! system-wide modal. There is no such thing on Linux: a menu is an overlay in
//! the app's own window. This module draws it with GPUI, so the normal frame
//! loop keeps running and nothing leaves the process.
//!
//! State lives in a UI-thread local rather than on `SoloView` so element
//! destruction (`retained_tree`) can cancel a menu without reaching a window.
//! Selecting sends the ID through a channel; the napi `Task` waiting on it
//! resolves the JavaScript promise.

use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, MouseDownEvent};
use napi::bindgen_prelude::{AsyncTask, Error, Result, Task};
use napi_derive::napi;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};

use crate::renderer::SoloView;
use crate::text::chrome_text;
use crate::theme::{neutral, Theme};

const MENU_WIDTH: f32 = 220.0;
const ITEM_HEIGHT: f32 = 26.0;
const SEPARATOR_HEIGHT: f32 = 9.0;
const PANEL_PADDING: f32 = 4.0;
const EDGE_MARGIN: f32 = 4.0;
const MAX_ITEMS: usize = 256;
const MAX_ID_BYTES: usize = 1024;
const MAX_LABEL_BYTES: usize = 4096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MenuItem {
    /// `None` for a separator.
    pub(crate) id: Option<String>,
    pub(crate) label: String,
    pub(crate) disabled: bool,
    pub(crate) checked: bool,
}

struct OpenMenu {
    request_id: u64,
    owner: u64,
    x: f32,
    y: f32,
    items: Vec<MenuItem>,
    highlighted: Option<usize>,
    /// Focused while the menu is open, so keys reach the app's capture
    /// listener even when the owner element is not focusable.
    focus: gpui::FocusHandle,
    /// Focused before the menu opened, restored when it closes as AppKit does
    /// for an `NSMenu`.
    previous_focus: Option<gpui::FocusHandle>,
    sender: Sender<Option<String>>,
}

thread_local! {
    // One context menu per app, as on macOS: opening another cancels the first.
    static ACTIVE: RefCell<Option<OpenMenu>> = const { RefCell::new(None) };
    // Keystrokes for the open menu, installed on the app so they arrive before
    // key bindings and regardless of which element has focus.
    static INTERCEPTOR: RefCell<Option<gpui::Subscription>> = const { RefCell::new(None) };
    // Focus to restore on the next repaint-capable caller after a menu settles.
    static RESTORE: RefCell<Option<gpui::FocusHandle>> = const { RefCell::new(None) };
}

/// Readable from any thread: automation asks whether keys belong to a menu.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Whether a menu is currently waiting for input.
pub(crate) fn is_open() -> bool {
    OPEN.load(Ordering::Acquire)
}

/// Install the app-wide keystroke interceptor while a menu is open.
pub(crate) fn set_interceptor(subscription: gpui::Subscription) {
    INTERCEPTOR.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            *slot = Some(subscription);
        }
    });
}

fn clear_interceptor() {
    INTERCEPTOR.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            slot.take();
        }
    });
}

fn with_active<R>(f: impl FnOnce(&OpenMenu) -> R) -> Option<R> {
    ACTIVE.with(|active| active.try_borrow().ok()?.as_ref().map(f))
}

fn with_active_mut<R>(f: impl FnOnce(&mut OpenMenu) -> R) -> Option<R> {
    ACTIVE.with(|active| active.try_borrow_mut().ok()?.as_mut().map(f))
}

/// Resolve a waiting session with `result` and drop the overlay.
fn settle(result: Option<String>) {
    let menu = ACTIVE.with(|active| active.try_borrow_mut().ok().and_then(|mut slot| slot.take()));
    if let Some(menu) = menu {
        OPEN.store(false, Ordering::Release);
        clear_interceptor();
        RESTORE.with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                *slot = menu.previous_focus.clone();
            }
        });
        // The receiver may already be gone if the app is shutting down.
        let _ = menu.sender.send(result);
    }
}

/// Give focus back to the element that had it before the menu opened. Safe to
/// call when no menu closed; the handle may belong to a destroyed element, in
/// which case GPUI simply ends up with no focused element.
pub(crate) fn restore_focus(window: &mut gpui::Window, cx: &mut gpui::App) {
    let focus = RESTORE.with(|slot| slot.try_borrow_mut().ok().and_then(|mut slot| slot.take()));
    if let Some(focus) = focus {
        focus.focus(window, cx);
    }
}

/// Dismiss the open menu, resolving its promise with `null`.
pub(crate) fn dismiss() -> bool {
    let open = with_active(|_| ()).is_some();
    if open {
        settle(None);
    }
    open
}

/// Cancel a specific session; other request IDs are ignored, like macOS.
pub(crate) fn cancel(request_id: u64) -> bool {
    let matches = with_active(|menu| menu.request_id == request_id).unwrap_or(false);
    if matches {
        settle(None);
    }
    matches
}

/// A window is going away or the app is quitting: resolve every pending menu.
pub(crate) fn cancel_all() {
    if with_active(|_| ()).is_some() {
        settle(None);
    }
}

/// Destroyed elements cancel the menu they own.
pub(crate) fn cancel_destroyed(ids: &[u64]) {
    let owned = with_active(|menu| ids.contains(&menu.owner)).unwrap_or(false);
    if owned {
        settle(None);
    }
}

/// Paint the menu, if one is open. `viewport` is the window's content size in
/// logical points, which is the coordinate space the anchor uses.
pub(crate) fn overlay(
    theme: &Theme,
    viewport: gpui::Size<gpui::Pixels>,
    cx: &mut Context<SoloView>,
) -> Option<gpui::AnyElement> {
    let (items, highlighted, x, y, focus) = with_active(|menu| {
        (
            menu.items.clone(),
            menu.highlighted,
            menu.x,
            menu.y,
            menu.focus.clone(),
        )
    })?;
    let layout = layout(
        &items,
        x,
        y,
        f32::from(viewport.width),
        f32::from(viewport.height),
    );
    let mut rows: Vec<gpui::AnyElement> = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        if item.id.is_none() {
            rows.push(
                div()
                    .h(px(1.0))
                    .my(px((SEPARATOR_HEIGHT - 1.0) / 2.0))
                    .mx(px(4.0))
                    .bg(theme.border)
                    .into_any_element(),
            );
            continue;
        }
        let selected = highlighted == Some(index);
        let disabled = item.disabled;
        let row = div()
            .id(("solo-menu-item", index))
            .h(px(ITEM_HEIGHT))
            .px(px(8.0))
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .rounded_sm()
            .text_size(px(13.0))
            .when(disabled, |this| this.text_color(theme.text_faint))
            .when(!disabled, |this| {
                this.text_color(theme.text)
                    .cursor_pointer()
                    .hover(|style| style.bg(neutral(0.18)))
            })
            .when(selected, |this| this.bg(neutral(0.24)))
            .child(
                div()
                    .w(px(12.0))
                    .text_size(px(12.0))
                    .text_color(theme.accent)
                    .when(item.checked, |this| this.child(chrome_text("✓".into(), None))),
            )
            .child(chrome_text(item.label.clone().into(), None));
        let row = if disabled {
            row
        } else {
            row.on_click(cx.listener(
                move |_view, _: &gpui::ClickEvent, window, cx| {
                    if choose(index) {
                        restore_focus(window, cx);
                        cx.notify();
                    }
                },
            ))
        };
        rows.push(row.into_any_element());
    }
    let panel = div()
        .absolute()
        .left(px(layout.x))
        .top(px(layout.y))
        .w(px(MENU_WIDTH))
        .p(px(PANEL_PADDING))
        .flex()
        .flex_col()
        // Stops the backdrop sibling underneath from seeing clicks on the menu.
        .occlude()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.bg)
        .font_family(theme.font_sans.clone())
        .children(rows);
    Some(
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .track_focus(&focus)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    // A menu is modal: the backdrop takes every hit, so the
                    // click that dismisses it cannot also reach the content
                    // underneath, as AppKit's NSMenu does.
                    .occlude()
                    // Any button dismisses, so a right click outside does not
                    // slip past the menu either.
                    .on_any_mouse_down(cx.listener(
                        |_view, _: &MouseDownEvent, window, cx| {
                            if dismiss() {
                                restore_focus(window, cx);
                                cx.notify();
                            }
                        },
                    )),
            )
            .child(panel)
            .into_any_element(),
    )
}

/// Handle a key while the menu is open. Returns whether the key was consumed.
/// Mirrors the macOS helper: arrows skip separators and disabled items and stop
/// at the ends; Enter with nothing highlighted dismisses; Escape dismisses.
pub(crate) fn handle_key(key: &str) -> bool {
    let Some(open) = with_active(|menu| {
        (
            menu.items.clone(),
            menu.highlighted,
        )
    }) else {
        return false;
    };
    let (items, highlighted) = open;
    match key {
        "escape" => {
            settle(None);
            true
        }
        "enter" => {
            let choice = highlighted.and_then(|index| items[index].id.clone());
            settle(choice);
            true
        }
        "up" => {
            set_highlight(move_highlight(&items, highlighted, -1));
            true
        }
        "down" => {
            set_highlight(move_highlight(&items, highlighted, 1));
            true
        }
        "home" => {
            set_highlight(first_selectable(&items));
            true
        }
        "end" => {
            set_highlight(last_selectable(&items));
            true
        }
        _ => false,
    }
}

fn set_highlight(index: Option<usize>) {
    with_active_mut(|menu| menu.highlighted = index);
}

/// Select an item by index (a click). Reports whether a menu was open.
pub(crate) fn choose(index: usize) -> bool {
    let Some(choice) = with_active(|menu| {
        menu.items
            .get(index)
            .filter(|item| !item.disabled)
            .and_then(|item| item.id.clone())
    }) else {
        return with_active(|_| ()).is_some();
    };
    settle(choice);
    true
}

fn selectable(item: &MenuItem) -> bool {
    item.id.is_some() && !item.disabled
}

fn move_highlight(items: &[MenuItem], current: Option<usize>, delta: i32) -> Option<usize> {
    let len = items.len();
    let mut index = match current {
        Some(index) => index as i32,
        None if delta > 0 => -1,
        None => len as i32,
    };
    loop {
        index += delta;
        if index < 0 || index >= len as i32 {
            // Stop at the ends rather than wrapping, as AppKit does.
            return current;
        }
        if selectable(&items[index as usize]) {
            return Some(index as usize);
        }
    }
}

fn first_selectable(items: &[MenuItem]) -> Option<usize> {
    items.iter().position(selectable)
}

fn last_selectable(items: &[MenuItem]) -> Option<usize> {
    items.iter().rposition(selectable)
}

#[derive(Debug, PartialEq)]
pub(crate) struct Layout {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) height: f32,
}

/// Anchor the menu at the pointer or below an element, then keep it inside the
/// window: shift left/up when it would overflow, never past the edge margin.
pub(crate) fn layout(items: &[MenuItem], x: f32, y: f32, width: f32, height: f32) -> Layout {
    let menu_height = menu_height(items);
    let max_x = (width - MENU_WIDTH - EDGE_MARGIN).max(EDGE_MARGIN);
    let max_y = (height - menu_height - EDGE_MARGIN).max(EDGE_MARGIN);
    Layout {
        x: x.max(EDGE_MARGIN).min(max_x),
        y: y.max(EDGE_MARGIN).min(max_y),
        height: menu_height,
    }
}

pub(crate) fn menu_height(items: &[MenuItem]) -> f32 {
    PANEL_PADDING * 2.0
        + items
            .iter()
            .map(|item| {
                if item.id.is_some() {
                    ITEM_HEIGHT
                } else {
                    SEPARATOR_HEIGHT
                }
            })
            .sum::<f32>()
}

/// Parse and validate a `showContextMenu` request. The macOS helper applies
/// the same limits through its protocol module.
pub(crate) fn parse_request(
    json: &str,
    tree: &crate::retained_tree::RetainedTree,
) -> Result<(u64, u64, Vec<MenuItem>, Option<(f32, f32)>)> {
    if json.len() > 1_048_576 {
        return Err(Error::from_reason("Context menu request is too large"));
    }
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::from_reason(e.to_string()))?;
    let request_id = crate::renderer::to_element_id(value["requestId"].as_f64().unwrap_or(f64::NAN))?;
    let owner = crate::renderer::to_element_id(value["elementId"].as_f64().unwrap_or(f64::NAN))?;
    if !tree.elements.contains_key(&owner) {
        return Err(Error::from_reason("Context menu owner element is unmounted"));
    }
    let position = match (value.get("x"), value.get("y")) {
        (None, None) => None,
        (Some(x), Some(y)) => {
            let (x, y) = (
                x.as_f64().unwrap_or(f64::NAN),
                y.as_f64().unwrap_or(f64::NAN),
            );
            if !x.is_finite() || !y.is_finite() {
                return Err(Error::from_reason("Pointer coordinates must be finite"));
            }
            Some((x as f32, y as f32))
        }
        _ => return Err(Error::from_reason("Both pointer coordinates are required")),
    };
    let raw = value["items"]
        .as_array()
        .ok_or_else(|| Error::from_reason("Context menu items are missing"))?;
    if raw.is_empty() {
        return Err(Error::from_reason("Context menu must contain items"));
    }
    if raw.len() > MAX_ITEMS {
        return Err(Error::from_reason(format!(
            "Context menu has {} items; the limit is {MAX_ITEMS}",
            raw.len()
        )));
    }
    let mut items = Vec::with_capacity(raw.len());
    let mut seen = std::collections::HashSet::new();
    for entry in raw {
        if entry.get("type").and_then(|kind| kind.as_str()) == Some("separator") {
            items.push(MenuItem {
                id: None,
                label: String::new(),
                disabled: true,
                checked: false,
            });
            continue;
        }
        let id = entry["id"]
            .as_str()
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| Error::from_reason("Context menu IDs must be non-empty strings"))?;
        if id.len() > MAX_ID_BYTES {
            return Err(Error::from_reason(format!(
                "Context menu ID is {} bytes; the limit is {MAX_ID_BYTES}",
                id.len()
            )));
        }
        let label = entry["label"]
            .as_str()
            .filter(|label| !label.trim().is_empty())
            .ok_or_else(|| Error::from_reason("Context menu labels must be non-empty strings"))?;
        if label.len() > MAX_LABEL_BYTES {
            return Err(Error::from_reason(format!(
                "Context menu label is {} bytes; the limit is {MAX_LABEL_BYTES}",
                label.len()
            )));
        }
        if !seen.insert(id.to_string()) {
            return Err(Error::from_reason(format!("Duplicate context menu ID: {id}")));
        }
        for flag in ["disabled", "checked"] {
            if let Some(value) = entry.get(flag) {
                if !value.is_boolean() {
                    return Err(Error::from_reason("Context menu flags must be boolean"));
                }
            }
        }
        items.push(MenuItem {
            id: Some(id.to_string()),
            label: label.to_string(),
            disabled: entry["disabled"].as_bool().unwrap_or(false),
            checked: entry["checked"].as_bool().unwrap_or(false),
        });
    }
    if !items.iter().any(selectable) {
        return Err(Error::from_reason("Context menu must contain an action"));
    }
    Ok((request_id, owner, items, position))
}

/// Open a menu in the owner's window. Runs on the UI thread; the returned
/// receiver settles when the user chooses, dismisses, or the owner goes away.
pub(crate) fn show(
    json: &str,
    tree: &crate::retained_tree::RetainedTree,
    focus: gpui::FocusHandle,
    previous_focus: Option<gpui::FocusHandle>,
) -> Result<Receiver<Option<String>>> {
    let (request_id, owner, items, position) = parse_request(json, tree)?;
    // Release builds record bounds only where something reads them, so an
    // owner without listeners anchors to its nearest painted ancestor.
    let bounds = std::iter::successors(Some(owner), |id| tree.elements.get(id)?.parent)
        .find_map(crate::automation::get_bounds)
        .ok_or_else(|| Error::from_reason("Context menu owner element has not been painted"))?;
    let (x, y) = position.unwrap_or((bounds.x as f32, (bounds.y + bounds.height) as f32));
    // Opening a second menu cancels the first, as the macOS helper does.
    cancel_all();
    // The cancelled menu's saved focus was not handed back to anyone, so drop
    // it: otherwise the next interceptor keystroke would restore focus to an
    // element behind this menu.
    RESTORE.with(|slot| {
        if let Ok(mut slot) = slot.try_borrow_mut() {
            slot.take();
        }
    });
    let (sender, receiver) = channel();
    ACTIVE.with(|active| {
        if let Ok(mut slot) = active.try_borrow_mut() {
            *slot = Some(OpenMenu {
                request_id,
                owner,
                x,
                y,
                items,
                highlighted: None,
                focus,
                previous_focus,
                sender,
            });
        }
    });
    OPEN.store(true, Ordering::Release);
    Ok(receiver)
}

/// The napi task that resolves once the menu settles.
pub struct MenuTask {
    receiver: Receiver<Option<String>>,
}

impl MenuTask {
    pub(crate) fn new(receiver: Receiver<Option<String>>) -> Self {
        Self { receiver }
    }
}

#[napi]
impl Task for MenuTask {
    type Output = Option<String>;
    type JsValue = Option<String>;

    fn compute(&mut self) -> Result<Self::Output> {
        // A cancelled or dismissed session sends `None`; a dropped sender (app
        // shutdown) also reads as dismissal.
        Ok(self.receiver.recv().unwrap_or(None))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

/// Keep the overlay in step with key input routed through `SoloView`'s capture
/// listener: returns true when the key belonged to the menu.
pub(crate) fn key_down(event: &KeyDownEvent) -> bool {
    handle_key(&event.keystroke.key)
}

/// Re-exported for `renderer` so `show_context_menu` can name the task type.
pub(crate) type MenuAsyncTask = AsyncTask<MenuTask>;

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, disabled: bool) -> MenuItem {
        MenuItem {
            id: Some(id.into()),
            label: id.into(),
            disabled,
            checked: false,
        }
    }

    fn separator() -> MenuItem {
        MenuItem {
            id: None,
            label: String::new(),
            disabled: true,
            checked: false,
        }
    }

    #[test]
    fn arrows_skip_disabled_items_and_separators() {
        let items = vec![
            item("disabled", true),
            separator(),
            item("read", false),
            item("delete", false),
        ];
        assert_eq!(move_highlight(&items, None, 1), Some(2));
        assert_eq!(move_highlight(&items, Some(2), 1), Some(3));
        assert_eq!(move_highlight(&items, Some(3), 1), Some(3));
        assert_eq!(move_highlight(&items, None, -1), Some(3));
        assert_eq!(move_highlight(&items, Some(2), -1), Some(2));
    }

    #[test]
    fn home_and_end_find_selectable_items() {
        let items = vec![item("disabled", true), separator(), item("read", false)];
        assert_eq!(first_selectable(&items), Some(2));
        assert_eq!(last_selectable(&items), Some(2));
    }

    #[test]
    fn layout_keeps_the_menu_inside_the_window() {
        let items = vec![item("a", false), item("b", false)];
        let height = menu_height(&items);
        let inside = layout(&items, 100.0, 100.0, 800.0, 600.0);
        assert_eq!((inside.x, inside.y), (100.0, 100.0));
        assert_eq!(inside.height, height);
        // Overflow on both axes shifts the menu back inside.
        let shifted = layout(&items, 780.0, 590.0, 800.0, 600.0);
        assert_eq!(shifted.x, 800.0 - MENU_WIDTH - EDGE_MARGIN);
        assert_eq!(shifted.y, 600.0 - height - EDGE_MARGIN);
        // A window smaller than the menu still starts at the edge margin.
        let tiny = layout(&items, 0.0, 0.0, 100.0, 20.0);
        assert_eq!((tiny.x, tiny.y), (EDGE_MARGIN, EDGE_MARGIN));
    }

    #[test]
    fn empty_state_is_safe_on_idle_keys() {
        settle(None);
        assert!(!handle_key("down"));
        assert!(!dismiss());
        assert!(!cancel(1));
    }
}
