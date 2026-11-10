//! Linux status item: a StatusNotifierItem on the session bus.
//!
//! Neither X11 nor Wayland has a menu bar, so Linux desktops show an icon in a
//! status-notifier host instead: KDE Plasma natively, XFCE through its status
//! notifier plugin, GNOME through the (extensively packaged) AppIndicator and
//! KStatusNotifierItem extensions. The item's menu travels over
//! `com.canonical.dbusmenu`.
//!
//! `ksni` implements all of that over D-Bus without GTK, which suits Solo's
//! GTK-free windows, so the tray is one `spawn()` plus a menu description. The
//! service runs on its own thread; every callback routes through the renderer,
//! which owns the UI thread and the event callback.

use ksni::blocking::{Handle, TrayMethods};
use ksni::menu::{CheckmarkItem, MenuItem, StandardItem};
use ksni::{Icon, ToolTip, Tray};
use napi::bindgen_prelude::{Error, Result};

use super::{
    parse_update, MenuBarItem, APPLICATION_REOPEN_EVENT, MENU_BAR_ACTION_EVENT,
};
use crate::renderer::{self, EventCallback};

/// Owns the status item. The item stays registered until the process exits.
pub(crate) struct MenuBar {
    handle: Handle<SoloTray>,
    application_title: String,
    initial_icon_path: String,
    initial_tooltip: Option<String>,
}

impl MenuBar {
    /// The icon click emits `applicationReopen` through `callback` without
    /// waiting for a `configureMenuBar` call, as the macOS status item does.
    pub(crate) fn new(
        options: &crate::renderer::MenuBarOptions,
        application_title: &str,
        callback: Option<EventCallback>,
    ) -> Result<Self> {
        let icon = load_icon(&options.icon_path)?;
        let tray = SoloTray {
            id: tray_id(application_title),
            application_title: application_title.to_string(),
            icon,
            tooltip: options.tooltip.clone(),
            items: Vec::new(),
            callback,
        };
        // Without a host the icon would be invisible, and a background app
        // with no window and no icon is unreachable; fail where the app can
        // still report it instead of running hidden.
        let handle = tray.spawn().map_err(|error| {
            Error::from_reason(format!(
                "No status notifier is listening on the session bus ({error}), so the menu-bar \
                 icon cannot appear. Install one: KDE Plasma, XFCE with the status notifier \
                 plugin, or GNOME with the AppIndicator and KStatusNotifierItem extensions."
            ))
        })?;
        Ok(Self {
            handle,
            application_title: application_title.to_string(),
            initial_icon_path: options.icon_path.clone(),
            initial_tooltip: options.tooltip.clone(),
        })
    }

    pub(crate) fn update(
        &mut self,
        json: Option<&str>,
        callback: Option<EventCallback>,
    ) -> Result<()> {
        let update = json.map(parse_update).transpose()?;
        let icon_path = update
            .as_ref()
            .and_then(|update| update.icon_path.as_deref())
            .unwrap_or(&self.initial_icon_path);
        let icon = load_icon(icon_path)?;
        let tooltip = update
            .as_ref()
            .and_then(|update| update.tooltip.clone())
            .unwrap_or_else(|| self.initial_tooltip.clone());
        let items = update.map(|update| update.items).unwrap_or_default();
        let title = self.application_title.clone();
        self.handle.update(move |tray: &mut SoloTray| {
            tray.application_title = title;
            tray.icon = icon;
            tray.tooltip = tooltip;
            tray.items = items;
            // Keep the callback an earlier update installed when this call
            // does not carry one: `init` sets it from the renderer, and a
            // later update must not disable the reopen event.
            if callback.is_some() {
                tray.callback = callback;
            }
        });
        Ok(())
    }
}

/// What choosing a row does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TrayClick {
    /// A menu item the app defined; its token goes to `menuBarAction`.
    Token(String),
    /// Solo's built-in item: bring the main window back.
    Open,
    /// Solo's built-in item: end the application.
    Quit,
}

/// One row of the item's menu, in painting order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum TrayRow {
    Action {
        token: String,
        label: String,
        enabled: bool,
        checked: bool,
    },
    Separator,
    /// Non-actionable text (`type: "status"`).
    Status {
        label: String,
    },
    Builtin {
        click: TrayClick,
        label: String,
    },
}

/// The app's items, then Solo's built-in Open and Quit — the same order as the
/// macOS menu.
pub(super) fn rows(items: &[MenuBarItem], application_title: &str) -> Vec<TrayRow> {
    let mut rows: Vec<TrayRow> = items
        .iter()
        .map(|item| match item {
            MenuBarItem::Action {
                token,
                label,
                enabled,
                checked,
            } => TrayRow::Action {
                token: token.clone(),
                label: label.clone(),
                enabled: *enabled,
                checked: *checked,
            },
            MenuBarItem::Separator => TrayRow::Separator,
            MenuBarItem::Status { label } => TrayRow::Status {
                label: label.clone(),
            },
        })
        .collect();
    if !rows.is_empty() {
        rows.push(TrayRow::Separator);
    }
    rows.push(TrayRow::Builtin {
        click: TrayClick::Open,
        label: format!("Open {application_title}"),
    });
    rows.push(TrayRow::Separator);
    rows.push(TrayRow::Builtin {
        click: TrayClick::Quit,
        label: format!("Quit {application_title}"),
    });
    rows
}

/// `com.canonical.dbusmenu` reads `_` as the access-key marker, so an app's
/// underscores are doubled to show literally.
fn escape_label(label: &str) -> String {
    label.replace('_', "__")
}

/// A stable, D-Bus-safe item name. It should survive restarts, so it comes
/// from the application title rather than the process.
fn tray_id(application_title: &str) -> String {
    let id: String = application_title
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let id = id.trim_matches('-').to_ascii_lowercase();
    if id.is_empty() {
        "solo-app".to_string()
    } else {
        id
    }
}

/// The `iconPath` PNG as an ARGB32 pixmap, the only icon form the item carries
/// (`StatusNotifierItem` has no template images).
fn load_icon(path: &str) -> Result<Icon> {
    let bytes = std::fs::read(path)
        .map_err(|error| Error::from_reason(format!("Failed to read menu-bar icon {path:?}: {error}")))?;
    let image = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png)
        .map_err(|error| Error::from_reason(format!("Menu-bar icon {path:?} is not a PNG: {error}")))?;
    let (width, height) = (image.width(), image.height());
    let mut data = image.into_rgba8().into_vec();
    for pixel in data.chunks_exact_mut(4) {
        // RGBA to the ARGB the specification's network byte order asks for.
        pixel.rotate_right(1);
    }
    Ok(Icon {
        width: width as i32,
        height: height as i32,
        data,
    })
}

/// The item `ksni` publishes: what the host reads and what the callbacks do.
struct SoloTray {
    id: String,
    application_title: String,
    icon: Icon,
    tooltip: Option<String>,
    items: Vec<MenuBarItem>,
    callback: Option<EventCallback>,
}

impl SoloTray {
    fn run(&mut self, click: TrayClick) {
        match click {
            TrayClick::Token(token) => renderer::emit_event_full(
                &self.callback,
                0,
                MENU_BAR_ACTION_EVENT,
                |event| event.value = Some(token),
            ),
            TrayClick::Open => {
                if let Err(error) = renderer::show_main_window_from_menu() {
                    log::error!("Failed to reopen the Solo window: {error}");
                }
            }
            TrayClick::Quit => {
                if let Err(error) = renderer::quit_from_menu() {
                    log::error!("Failed to quit the Solo application: {error}");
                }
            }
        }
    }

    fn item(&self, row: TrayRow) -> MenuItem<Self> {
        match row {
            TrayRow::Separator => MenuItem::Separator,
            TrayRow::Status { label } => StandardItem {
                label: escape_label(&label),
                // Status rows are text, not commands.
                enabled: false,
                ..Default::default()
            }
            .into(),
            TrayRow::Action {
                token,
                label,
                enabled,
                checked,
            } => CheckmarkItem {
                label: escape_label(&label),
                enabled,
                checked,
                activate: Box::new(move |tray: &mut Self| tray.run(TrayClick::Token(token.clone()))),
                ..Default::default()
            }
            .into(),
            TrayRow::Builtin { click, label } => StandardItem {
                label: escape_label(&label),
                enabled: true,
                activate: Box::new(move |tray: &mut Self| tray.run(click.clone())),
                ..Default::default()
            }
            .into(),
        }
    }
}

impl Tray for SoloTray {
    const MENU_ON_ACTIVATE: bool = false;

    fn id(&self) -> String {
        self.id.clone()
    }

    fn title(&self) -> String {
        self.application_title.clone()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        vec![self.icon.clone()]
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: self.tooltip.clone().unwrap_or_default(),
            description: self.tooltip.clone().unwrap_or_default(),
            ..Default::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        rows(&self.items, &self.application_title)
            .into_iter()
            .map(|row| self.item(row))
            .collect()
    }

    /// A click on the icon is the Dock click of Linux: the app decides whether
    /// to bring its window back (`onApplicationReopen`).
    fn activate(&mut self, _x: i32, _y: i32) {
        renderer::emit_event_full(&self.callback, 0, APPLICATION_REOPEN_EVENT, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn action(token: &str, enabled: bool, checked: bool) -> MenuBarItem {
        MenuBarItem::Action {
            token: token.into(),
            label: format!("Item {token}"),
            enabled,
            checked,
        }
    }

    #[test]
    fn menu_is_app_items_then_open_and_quit() {
        let items = vec![
            action("1", true, false),
            MenuBarItem::Separator,
            MenuBarItem::Status {
                label: "Idle".into(),
            },
        ];
        assert_eq!(
            rows(&items, "Newsprint"),
            vec![
                TrayRow::Action {
                    token: "1".into(),
                    label: "Item 1".into(),
                    enabled: true,
                    checked: false,
                },
                TrayRow::Separator,
                TrayRow::Status {
                    label: "Idle".into()
                },
                TrayRow::Separator,
                TrayRow::Builtin {
                    click: TrayClick::Open,
                    label: "Open Newsprint".into(),
                },
                TrayRow::Separator,
                TrayRow::Builtin {
                    click: TrayClick::Quit,
                    label: "Quit Newsprint".into(),
                },
            ]
        );
    }

    #[test]
    fn a_menu_without_app_items_starts_at_open() {
        assert_eq!(
            rows(&[], "Solo"),
            vec![
                TrayRow::Builtin {
                    click: TrayClick::Open,
                    label: "Open Solo".into(),
                },
                TrayRow::Separator,
                TrayRow::Builtin {
                    click: TrayClick::Quit,
                    label: "Quit Solo".into(),
                },
            ]
        );
    }

    #[test]
    fn labels_survive_the_access_key_marker() {
        assert_eq!(escape_label("Refresh_now"), "Refresh__now");
    }

    #[test]
    fn ids_are_stable_and_dbus_safe() {
        assert_eq!(tray_id("Newsprint Daily"), "newsprint-daily");
        assert_eq!(tray_id("Ünïcode"), "n-code");
        assert_eq!(tray_id("///"), "solo-app");
    }
}
