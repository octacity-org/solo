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

/// Add an existing file to the app's recent documents. macOS records it with
/// AppKit (Dock menu and `File > Open Recent`); Linux appends a bookmark to the
/// freedesktop `recently-used.xbel`, which GTK file choosers and desktop menus
/// read. Either way the app later receives a `file://` URL through the
/// open-URLs event when the user picks it.
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
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        recent_documents::add(&path)
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "linux",
        target_os = "freebsd"
    )))]
    {
        let _ = path;
        Err(Error::from_reason(
            "Recent documents are not supported on this operating system",
        ))
    }
}

/// The freedesktop recent-documents list (`recently-used.xbel`).
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod recent_documents {
    use super::*;
    use std::path::{Path, PathBuf};

    const EMPTY: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<xbel version=\"1.0\" ",
        "xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\" ",
        "xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\">\n",
        "</xbel>\n"
    );

    pub(super) fn add(path: &str) -> Result<()> {
        let data_home = std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share"))
            })
            .ok_or_else(|| {
                Error::from_reason("Cannot locate XDG_DATA_HOME or HOME for recent documents")
            })?;
        std::fs::create_dir_all(&data_home).map_err(|error| {
            Error::from_reason(format!(
                "Cannot create {}: {error}",
                data_home.display()
            ))
        })?;
        let file = data_home.join("recently-used.xbel");
        let existing = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => EMPTY.to_string(),
            Err(error) => {
                return Err(Error::from_reason(format!(
                    "Cannot read {}: {error}",
                    file.display()
                )))
            }
        };
        let url = url::Url::from_file_path(Path::new(path))
            .map_err(|_| Error::from_reason(format!("Cannot represent {path:?} as a file URL")))?;
        let updated = upsert(&existing, url.as_str(), Path::new(path))?;
        // Replace atomically: a desktop menu may read this file at any time.
        // A per-process suffix keeps two Solo apps adding documents at once
        // from writing each other's half-finished file.
        let temporary = file.with_extension(format!("xbel.solo-tmp-{}", std::process::id()));
        std::fs::write(&temporary, updated).map_err(|error| {
            Error::from_reason(format!("Cannot write {}: {error}", temporary.display()))
        })?;
        std::fs::rename(&temporary, &file).map_err(|error| {
            let _ = std::fs::remove_file(&temporary);
            Error::from_reason(format!("Cannot replace {}: {error}", file.display()))
        })
    }

    /// Insert one bookmark before `</xbel>`, or touch the earlier entry for the
    /// same URL in place. Refuses a file that is not XBEL instead of
    /// truncating it.
    ///
    /// An existing entry is kept whole: its `<info>` carries the MIME type and
    /// the `bookmark:applications` list of every app that opened the file, so
    /// replacing the element would erase other applications' history. Only
    /// `modified` and `visited` are bumped, which is how the recent-files spec
    /// describes a repeated open. New entries carry Solo's own application
    /// metadata.
    fn upsert(xbel: &str, href: &str, path: &Path) -> Result<String> {
        let end = xbel
            .rfind("</xbel>")
            .ok_or_else(|| Error::from_reason("recently-used.xbel has no closing </xbel>"))?;
        let now = timestamp();
        let escaped_href = xml_escape(href);
        let marker = format!("<bookmark href=\"{escaped_href}\"");
        if let Some(start) = xbel[..end].find(&marker) {
            // An entry is either self-closing or ends with `</bookmark>`; both
            // are written by different desktop tools.
            let open_end = start
                + xbel[start..]
                    .find('>')
                    .ok_or_else(|| Error::from_reason("recently-used.xbel has an unclosed bookmark"))?;
            let self_closing = xbel[..open_end].ends_with('/');
            let element_end = if self_closing {
                open_end + 1
            } else {
                open_end
                    + 1
                    + xbel[open_end + 1..]
                        .find("</bookmark>")
                        .ok_or_else(|| {
                            Error::from_reason("recently-used.xbel has an unclosed bookmark")
                        })?
                    + "</bookmark>".len()
            };
            let mut bookmark = String::from(&xbel[start..element_end]);
            if self_closing {
                // Nothing to preserve: expand it so the new entry has metadata.
                let open_end = bookmark.find('>').unwrap();
                let tag = set_attribute(&bookmark[..open_end], "modified", &now);
                let tag = set_attribute(&tag, "visited", &now);
                let tag = tag.trim_end_matches('/').trim_end().to_string();
                bookmark = format!("{tag}>{}</bookmark>", metadata(path));
            } else {
                let open_end = bookmark.find('>').unwrap();
                let tag = set_attribute(&bookmark[..open_end], "modified", &now);
                let tag = set_attribute(&tag, "visited", &now);
                bookmark.replace_range(..=open_end, &format!("{tag}>"));
            }
            return Ok(format!(
                "{}{}{}",
                &xbel[..start],
                bookmark,
                &xbel[element_end..]
            ));
        }
        let head = &xbel[..end];
        let entry = format!(
            "  <bookmark href=\"{escaped_href}\" added=\"{now}\" modified=\"{now}\" visited=\"{now}\">{}\n  </bookmark>\n",
            metadata(path)
        );
        Ok(format!("{head}{entry}</xbel>\n"))
    }

    /// Solo's `<info>` for a new bookmark: its MIME type and application entry.
    fn metadata(path: &Path) -> String {
        let now = timestamp();
        let exec = std::env::current_exe()
            .ok()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| "solo".to_string());
        let mime = mime_type_for_extension(path);
        let mime = mime
            .map(|mime| format!("\n        <mime:mime-type type=\"{mime}\"/>"))
            .unwrap_or_default();
        format!(
            "\n    <info>\n      <metadata owner=\"http://freedesktop.org\">{mime}\n        <bookmark:applications>\n          <bookmark:application name=\"Solo\" exec=\"{}\" count=\"1\" modified=\"{now}\"/>\n        </bookmark:applications>\n      </metadata>\n    </info>\n",
            xml_escape(&exec)
        )
    }

    /// Set an attribute on an opening tag, replacing or appending it.
    fn set_attribute(tag: &str, name: &str, value: &str) -> String {
        let needle = format!("{name}=\"");
        if let Some(start) = tag.find(&needle) {
            let value_start = start + needle.len();
            if let Some(length) = tag[value_start..].find('"') {
                return format!(
                    "{}{}{}",
                    &tag[..value_start],
                    xml_escape(value),
                    &tag[value_start + length..]
                );
            }
        }
        let insert = tag.rfind('>').unwrap_or(tag.len());
        let mut at = insert;
        if tag[..insert].ends_with('/') {
            at = insert - 1;
        }
        format!(
            "{} {name}=\"{}\"{}",
            tag[..at].trim_end(),
            xml_escape(value),
            &tag[at..]
        )
    }

    /// A rough MIME type from the file name. The recent-files metadata may
    /// omit it; GTK only uses it to filter and label entries.
    fn mime_type_for_extension(path: &Path) -> Option<&'static str> {
        let extension = path
            .extension()?
            .to_str()?
            .to_ascii_lowercase();
        Some(match extension.as_str() {
            "txt" | "log" => "text/plain",
            "md" | "markdown" => "text/markdown",
            "html" | "htm" => "text/html",
            "json" => "application/json",
            "xml" => "application/xml",
            "pdf" => "application/pdf",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            "gif" => "image/gif",
            "svg" => "image/svg+xml",
            "mp3" => "audio/mpeg",
            "mp4" => "video/mp4",
            _ => return None,
        })
    }

    fn xml_escape(text: &str) -> String {
        text.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    /// UTC, as the recent-documents spec asks (`1999-12-08T08:12:33Z`).
    fn timestamp() -> String {
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let days = (seconds / 86_400) as i64;
        let time = seconds % 86_400;
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            time / 3600,
            time % 3600 / 60,
            time % 60
        )
    }

    /// Howard Hinnant's civil-date conversion; days since 1970-01-01.
    fn civil_from_days(days: i64) -> (i64, u32, u32) {
        let days = days + 719_468;
        let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
        let day_of_era = (days - era * 146_097) as u64;
        let year_of_era =
            (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let year = year_of_era as i64 + era * 400;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let month_prime = (5 * day_of_year + 2) / 153;
        let day = (day_of_year - (153 * month_prime + 2) / 5 + 1) as u32;
        let month = if month_prime < 10 {
            month_prime + 3
        } else {
            month_prime - 9
        } as u32;
        (if month <= 2 { year + 1 } else { year }, month, day)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        use std::path::Path;

        #[test]
        fn writes_a_bookmark_into_a_fresh_xbel() {
            let updated = upsert(
                EMPTY,
                "file:///tmp/Notes%20%26%20More.md",
                Path::new("/tmp/Notes%20%26%20More.md"),
            )
            .unwrap();
            assert!(updated.contains("<bookmark href=\"file:///tmp/Notes%20%26%20More.md\""));
            assert!(updated.ends_with("</xbel>\n"));
            assert_eq!(updated.matches("</xbel>").count(), 1);
            // GTK's recent view filters and labels by the metadata.
            assert!(updated.contains("<mime:mime-type type=\"text/markdown\"/>"));
            assert!(updated.contains("<bookmark:application name=\"Solo\""));
        }

        #[test]
        fn touches_an_existing_bookmark_without_dropping_its_metadata() {
            let once = upsert(EMPTY, "file:///tmp/a.txt", Path::new("/tmp/a.txt")).unwrap();
            let twice = upsert(&once, "file:///tmp/a.txt", Path::new("/tmp/a.txt")).unwrap();
            assert_eq!(twice.matches("href=\"file:///tmp/a.txt\"").count(), 1);
            assert_eq!(twice.matches("<bookmark ").count(), 1);
            assert!(twice.contains("file:///tmp/a.txt") && twice.contains("<xbel"));
            // Another application's metadata is preserved, not rewritten.
            let hand_written = format!(
                "{}\n  <bookmark href=\"file:///tmp/a.txt\" added=\"2020-01-01T00:00:00Z\" modified=\"2020-01-01T00:00:00Z\" visited=\"2020-01-01T00:00:00Z\">\n    <info>\n      <metadata owner=\"http://freedesktop.org\">\n        <mime:mime-type type=\"text/plain\"/>\n        <bookmark:applications>\n          <bookmark:application name=\"gedit\" exec=\"gedit %u\" count=\"3\" modified=\"2020-01-01T00:00:00Z\"/>\n        </bookmark:applications>\n      </metadata>\n    </info>\n  </bookmark>\n</xbel>\n",
                &EMPTY[..EMPTY.len() - "</xbel>\n".len()]
            );
            let updated = upsert(&hand_written, "file:///tmp/a.txt", Path::new("/tmp/a.txt"))
                .unwrap();
            assert_eq!(updated.matches("href=\"file:///tmp/a.txt\"").count(), 1);
            assert!(updated.contains("<bookmark:application name=\"gedit\""));
            assert!(updated.contains("<mime:mime-type type=\"text/plain\"/>"));
            // `added` and gedit's own application record keep their timestamps;
            // only the bookmark's `modified` and `visited` are bumped.
            assert_eq!(updated.matches("2020-01-01T00:00:00Z").count(), 2);
            assert!(updated.contains("added=\"2020-01-01T00:00:00Z\""));
            assert!(updated.contains("name=\"gedit\" exec=\"gedit %u\" count=\"3\" modified=\"2020-01-01T00:00:00Z\""));
        }

        #[test]
        fn keeps_unrelated_bookmarks_and_refuses_non_xbel() {
            let once = upsert(EMPTY, "file:///tmp/a.txt", Path::new("/tmp/a.txt")).unwrap();
            let updated = upsert(&once, "file:///tmp/b&c.txt", Path::new("/tmp/b&c.txt"))
                .unwrap();
            assert!(updated.contains("file:///tmp/a.txt"));
            assert!(updated.contains("file:///tmp/b&amp;c.txt"));
            assert!(
                upsert("not xbel", "file:///tmp/a.txt", Path::new("/tmp/a.txt")).is_err()
            );
        }

        #[test]
        fn formats_utc_timestamps() {
            assert_eq!(civil_from_days(0), (1970, 1, 1));
            assert_eq!(civil_from_days(19_723), (2024, 1, 1));
            let stamp = timestamp();
            assert!(stamp.ends_with('Z'));
            assert_eq!(stamp.len(), 20);
        }
    }
}
