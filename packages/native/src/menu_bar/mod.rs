//! The status item the `menuBar` window option installs.
//!
//! macOS shows it in the menu bar (an `NSStatusItem`); Linux shows it through
//! the StatusNotifierItem specification, which KDE Plasma, XFCE's status
//! notifier plugin and GNOME's AppIndicator extension host. Both platforms
//! read the same JSON the JS side sends from `setMenuBar`, so the parsing and
//! validation live here and each platform only draws the result.

use std::collections::HashSet;

use napi::bindgen_prelude::{Error, Result};
use serde::{Deserialize, Deserializer};

/// The element-0 event a chosen menu-bar action sends, carrying its token.
pub(crate) const MENU_BAR_ACTION_EVENT: &str = "menuBarAction";

/// The element-0 event the status item sends when the user clicks it, the
/// counterpart of clicking the Dock icon on macOS.
pub(crate) const APPLICATION_REOPEN_EVENT: &str = "applicationReopen";

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::MenuBar;

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) use linux::MenuBar;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(crate) enum MenuBarItem {
    Action {
        token: String,
        label: String,
        enabled: bool,
        checked: bool,
    },
    Separator,
    Status {
        label: String,
    },
}

#[derive(Debug, Deserialize)]
pub(crate) struct MenuBarUpdate {
    #[serde(rename = "iconPath")]
    pub icon_path: Option<String>,
    #[serde(default, deserialize_with = "nullable_string")]
    pub tooltip: Option<Option<String>>,
    #[serde(default)]
    pub items: Vec<MenuBarItem>,
}

fn nullable_string<'de, D>(deserializer: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(deserializer)?))
}

pub(crate) fn parse_update(json: &str) -> Result<MenuBarUpdate> {
    let update: MenuBarUpdate = serde_json::from_str(json)
        .map_err(|error| Error::from_reason(format!("Invalid menu-bar JSON: {error}")))?;
    if update
        .icon_path
        .as_ref()
        .is_some_and(|path| !std::path::Path::new(path).is_absolute())
    {
        return Err(Error::from_reason(
            "Menu-bar iconPath must be an absolute file path",
        ));
    }
    if update
        .tooltip
        .as_ref()
        .and_then(|tooltip| tooltip.as_ref())
        .is_some_and(|tooltip| tooltip.trim().is_empty())
    {
        return Err(Error::from_reason("Menu-bar tooltip must not be empty"));
    }
    let mut tokens = HashSet::new();
    for item in &update.items {
        match item {
            MenuBarItem::Action { token, label, .. } => {
                if token.trim().is_empty() || label.trim().is_empty() {
                    return Err(Error::from_reason(
                        "Menu-bar action token and label must not be empty",
                    ));
                }
                if !tokens.insert(token) {
                    return Err(Error::from_reason("Duplicate menu-bar action token"));
                }
            }
            MenuBarItem::Status { label } if label.trim().is_empty() => {
                return Err(Error::from_reason(
                    "Menu-bar status label must not be empty",
                ));
            }
            _ => {}
        }
    }
    Ok(update)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_complete_dynamic_menu_snapshot() {
        let update = parse_update(
            r#"{
          "iconPath":"/tmp/busy.png",
          "tooltip":"Refreshing",
          "items":[
            {"type":"action","token":"17","label":"Refresh","enabled":false,"checked":true},
            {"type":"separator"},
            {"type":"status","label":"Last refresh: 10:42"}
          ]
        }"#,
        )
        .unwrap();

        assert_eq!(update.icon_path.as_deref(), Some("/tmp/busy.png"));
        assert_eq!(update.tooltip, Some(Some("Refreshing".into())));
        assert!(matches!(
            update.items[0],
            MenuBarItem::Action {
                enabled: false,
                checked: true,
                ..
            }
        ));
        assert!(matches!(update.items[1], MenuBarItem::Separator));
        assert!(matches!(update.items[2], MenuBarItem::Status { .. }));
    }

    #[test]
    fn rejects_invalid_dynamic_menu_before_appkit_mutation() {
        for json in [
            r#"{"iconPath":"relative.png","items":[]}"#,
            r#"{"items":[{"type":"status","label":" "}]}"#,
            r#"{"items":[{"type":"action","token":"1","label":"One","enabled":true,"checked":false},{"type":"action","token":"1","label":"Two","enabled":true,"checked":false}]}"#,
        ] {
            assert!(parse_update(json).is_err(), "accepted {json}");
        }
    }
}
