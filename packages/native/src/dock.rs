//! macOS Dock integration: app-defined Dock menu items, the Dock-click
//! reopen event, and recent documents.
//!
//! Dock items dispatch one GPUI action type carrying the JS item token.
//! Choosing one emits an element-0 `dockMenuAction` event on the current app
//! renderer's callback, so a `solo dev` reload needs only `reset`.

use napi::{Error, Result};
use napi_derive::napi;

/// Element-0 event whose `value` is the chosen Dock item's token.
#[cfg(target_os = "macos")]
const DOCK_MENU_ACTION_EVENT: &str = "dockMenuAction";
/// Element-0 event: the Dock icon was clicked while no window was visible.
#[cfg(target_os = "macos")]
const APPLICATION_REOPEN_EVENT: &str = "applicationReopen";

#[cfg(target_os = "macos")]
#[derive(Clone, PartialEq, gpui::Action)]
#[action(namespace = solo_dock, no_json)]
struct InvokeDockItem {
    token: String,
}

/// Tokens of the enabled items in the current Dock menu.
#[cfg(target_os = "macos")]
#[derive(Default)]
struct DockState {
    enabled: std::collections::HashSet<String>,
}

#[cfg(target_os = "macos")]
impl gpui::Global for DockState {}

/// AppKit validates Dock items through the app menu validator, which would
/// enable every item sharing `InvokeDockItem`. `None` for other actions.
#[cfg(target_os = "macos")]
pub(crate) fn is_dock_action_available(action: &dyn gpui::Action, cx: &gpui::App) -> Option<bool> {
    let action = action.as_any().downcast_ref::<InvokeDockItem>()?;
    Some(
        cx.try_global::<DockState>()
            .is_some_and(|state| state.enabled.contains(&action.token)),
    )
}

#[cfg(target_os = "macos")]
fn replace_dock_menu(items: Vec<crate::menu_bar::MenuBarItem>, cx: &mut gpui::App) {
    use crate::menu_bar::MenuBarItem;
    if !cx.has_global::<DockState>() {
        cx.set_global(DockState::default());
        cx.on_action(|action: &InvokeDockItem, cx| {
            if !cx.global::<DockState>().enabled.contains(&action.token) {
                return;
            }
            crate::renderer::emit_event_full(
                &crate::renderer::app_event_callback(),
                0,
                DOCK_MENU_ACTION_EVENT,
                |event| event.value = Some(action.token.clone()),
            );
        });
    }
    let mut enabled = std::collections::HashSet::new();
    let menu = items
        .into_iter()
        .map(|item| match item {
            MenuBarItem::Separator => gpui::MenuItem::separator(),
            // A label nobody can choose: its empty token is never enabled.
            MenuBarItem::Status { label } => {
                gpui::MenuItem::action(label, InvokeDockItem { token: String::new() }).disabled(true)
            }
            MenuBarItem::Action { token, label, enabled: is_enabled, checked } => {
                if is_enabled {
                    enabled.insert(token.clone());
                }
                gpui::MenuItem::action(label, InvokeDockItem { token })
                    .checked(checked)
                    .disabled(!is_enabled)
            }
        })
        .collect();
    cx.global_mut::<DockState>().enabled = enabled;
    cx.set_dock_menu(menu);
}

/// Dev reload: drop the previous app's Dock items. A no-op until an app has
/// set a Dock menu.
#[cfg(target_os = "macos")]
pub(crate) fn reset(cx: &mut gpui::App) {
    if cx.has_global::<DockState>() {
        replace_dock_menu(Vec::new(), cx);
    }
}

/// `Application::on_reopen`: GPUI calls it only when no window is visible.
#[cfg(target_os = "macos")]
pub(crate) fn reopen(_cx: &mut gpui::App) {
    crate::renderer::emit_event_full(
        &crate::renderer::app_event_callback(),
        0,
        APPLICATION_REOPEN_EVENT,
        |_| {},
    );
}

/// Replace the app-defined items of the Dock icon's menu (macOS). `items_json`
/// is a JSON array of menu-bar items: `{type:"action",token,label,enabled,
/// checked}`, `{type:"separator"}` or `{type:"status",label}`.
#[napi]
pub fn set_dock_menu(items_json: String) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        let items: Vec<crate::menu_bar::MenuBarItem> = serde_json::from_str(&items_json)
            .map_err(|error| Error::from_reason(format!("Invalid Dock menu: {error}")))?;
        if let Some(label) = items.iter().find_map(|item| match item {
            crate::menu_bar::MenuBarItem::Action { label, .. }
            | crate::menu_bar::MenuBarItem::Status { label } => {
                label.trim().is_empty().then_some(label)
            }
            crate::menu_bar::MenuBarItem::Separator => None,
        }) {
            return Err(Error::from_reason(format!("Invalid Dock menu label {label:?}")));
        }
        if !crate::renderer::worker_main::must_forward() {
            return Err(Error::from_reason("setDockMenu needs a running Solo app (worker mode)"));
        }
        crate::renderer::on_main(move || crate::renderer::app_update(|cx| replace_dock_menu(items, cx)))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = items_json;
        Err(Error::from_reason("The Dock menu is supported only on macOS"))
    }
}

/// Add an existing file to the app's recent documents (macOS), shown in the
/// Dock menu and in `File > Open Recent`. Choosing one sends the app a
/// `file://` URL through the open-URLs event.
#[napi]
pub fn add_recent_document(path: String) -> Result<()> {
    crate::desktop::validate_existing_path(&path)?;
    #[cfg(target_os = "macos")]
    {
        if !crate::renderer::worker_main::must_forward() {
            return Err(Error::from_reason(
                "addRecentDocument needs a running Solo app (worker mode)",
            ));
        }
        crate::renderer::on_main(move || {
            crate::renderer::app_update(|cx| cx.add_recent_document(std::path::Path::new(&path)))
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::from_reason("Recent documents are supported only on macOS"))
    }
}
