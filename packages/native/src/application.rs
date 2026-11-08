//! Application-level controls: hiding, thermal state, the keyboard layout,
//! the scroll-bar preference and restarting.
//!
//! Linux reports the keyboard layout and the scroll-bar preference, and can
//! restart a compiled app. Hiding other apps and thermal state are macOS-only,
//! because GPUI has no equivalent there; Solo throws instead of pretending.
//!
//! Thermal-state and keyboard-layout changes reach JS as element-0
//! `thermalStateChange` and `keyboardLayoutChange` events. Their observers are
//! installed once in `run_main_loop`, so a `solo dev` reload keeps them and
//! the new app receives them through the current event callback.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use napi::{Error, Result};
use napi_derive::napi;

/// The keyboard layout (input source) currently selected.
#[derive(Debug, Clone, PartialEq)]
#[napi(object)]
pub struct KeyboardLayout {
    /// Unique identifier, such as `com.apple.keylayout.US`.
    pub id: String,
    /// Name to show, such as `U.S.`.
    pub name: String,
}

fn thermal_state_name(state: gpui::ThermalState) -> &'static str {
    match state {
        gpui::ThermalState::Nominal => "nominal",
        gpui::ThermalState::Fair => "fair",
        gpui::ThermalState::Serious => "serious",
        gpui::ThermalState::Critical => "critical",
    }
}

fn keyboard_layout(cx: &gpui::App) -> KeyboardLayout {
    let layout = cx.keyboard_layout();
    KeyboardLayout {
        id: layout.id().to_string(),
        name: layout.name().to_string(),
    }
}

/// Element-0 event: the thermal state changed; `value` is the new state.
#[cfg(target_os = "macos")]
const THERMAL_STATE_CHANGE_EVENT: &str = "thermalStateChange";
/// Element-0 event: the keyboard layout changed; `value` is its JSON.
const KEYBOARD_LAYOUT_CHANGE_EVENT: &str = "keyboardLayoutChange";

/// Report thermal-state and keyboard-layout changes to the current app
/// renderer for the life of the process. Thermal state never changes on
/// Linux, so only the keyboard layout is observed there.
#[cfg(any(target_os = "macos", ui_thread))]
pub(crate) fn observe(cx: &mut gpui::App) {
    #[cfg(target_os = "macos")]
    cx.on_thermal_state_change(|cx| {
        let state = thermal_state_name(cx.thermal_state());
        crate::renderer::emit_event_full(
            &crate::renderer::app_event_callback(),
            0,
            THERMAL_STATE_CHANGE_EVENT,
            |payload| payload.value = Some(state.to_string()),
        );
    })
    .detach();
    cx.on_keyboard_layout_change(|cx| {
        let KeyboardLayout { id, name } = keyboard_layout(cx);
        let value = serde_json::json!({ "id": id, "name": name }).to_string();
        crate::renderer::emit_event_full(
            &crate::renderer::app_event_callback(),
            0,
            KEYBOARD_LAYOUT_CHANGE_EVENT,
            |payload| payload.value = Some(value),
        );
    })
    .detach();
}

/// Run `f` with the running app: the live app's main thread, or the
/// `TestSoloRenderer` app on this thread.
#[cfg(target_os = "macos")]
fn with_app<R: Send + 'static>(
    action: &str,
    f: impl FnOnce(&mut gpui::App) -> R + Send + 'static,
) -> Result<R> {
    if crate::renderer::worker_main::must_forward() {
        return crate::renderer::on_main(move || crate::renderer::app_update(f));
    }
    #[cfg(feature = "test-support")]
    if let Some(result) = crate::test_renderer::with_app(f) {
        return Ok(result);
    }
    Err(Error::from_reason(format!("{action} needs a running Solo app")))
}

#[cfg(ui_thread)]
fn with_app<R: Send + 'static>(
    action: &'static str,
    f: impl FnOnce(&mut gpui::App) -> R + Send + 'static,
) -> Result<R> {
    crate::renderer::with_ui_app(action, f)
}

#[cfg(not(any(target_os = "macos", ui_thread)))]
fn with_app<R>(action: &str, _f: impl FnOnce(&mut gpui::App) -> R) -> Result<R> {
    Err(Error::from_reason(format!(
        "{action} is not supported on this operating system"
    )))
}

/// Hide the application, as its Hide menu command does (macOS).
#[napi]
pub fn hide_application() -> Result<()> {
    with_app("hideApplication", |cx| cx.hide())
}

/// Hide every other application, as Hide Others does (macOS).
#[napi]
pub fn hide_other_applications() -> Result<()> {
    with_app("hideOtherApplications", |cx| cx.hide_other_apps())
}

/// Show the applications hidden by Hide Others, as Show All does (macOS).
#[napi]
pub fn show_all_applications() -> Result<()> {
    with_app("showAllApplications", |cx| cx.unhide_other_apps())
}

/// `"nominal"`, `"fair"`, `"serious"` or `"critical"` (macOS).
#[napi]
pub fn get_thermal_state() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        with_app("getThermalState", |cx| {
            thermal_state_name(cx.thermal_state()).to_string()
        })
    }
    // GPUI always reports `Nominal` on Linux, so a constant would look like a
    // reading while no thermal feed exists. Say so instead.
    #[cfg(not(target_os = "macos"))]
    {
        Err(Error::from_reason(
            "Thermal state is reported only on macOS",
        ))
    }
}

/// The keyboard layout currently selected.
#[napi]
pub fn get_keyboard_layout() -> Result<KeyboardLayout> {
    with_app("getKeyboardLayout", |cx| keyboard_layout(cx))
}

/// Whether the system shows scroll bars only while scrolling.
#[napi]
pub fn should_auto_hide_scrollbars() -> Result<bool> {
    with_app("shouldAutoHideScrollbars", |cx| cx.should_auto_hide_scrollbars())
}

/// Quit, then open the app bundle again once this process has exited
/// (macOS). Only a packaged `.app` can restart: run any other way, the
/// executable is `bun` itself, which would reopen without the app.
#[napi]
pub fn restart_application() -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if !crate::renderer::worker_main::must_forward() {
            return Err(Error::from_reason("restartApplication needs a running Solo app"));
        }
        crate::renderer::on_main(|| {
            crate::renderer::app_update(|cx| {
                let bundle = cx.app_path().ok().filter(|path| {
                    path.extension().is_some_and(|extension| extension == "app")
                });
                if bundle.is_none() {
                    return Err(Error::from_reason(
                        "restartApplication works only in a packaged .app bundle",
                    ));
                }
                cx.restart();
                Ok(())
            })?
        })
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        use std::path::Path;

        /// Whether `path` is a compiled Solo app rather than the `bun` or
        /// `node` runtime executing a script.
        fn is_compiled_app(path: &Path) -> bool {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| !matches!(stem, "bun" | "node" | "nodejs"))
        }

        crate::renderer::with_ui_app("restartApplication", |cx| {
            let executable = std::env::current_exe().map_err(|error| {
                Error::from_reason(format!("Unable to determine the running executable: {error}"))
            })?;
            if !is_compiled_app(&executable) {
                return Err(Error::from_reason(
                    "restartApplication works only in a compiled app binary, not under bun or node",
                ));
            }
            cx.restart();
            Ok(())
        })?
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "freebsd")))]
    {
        Err(Error::from_reason(
            "Restarting is not supported on this operating system",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_every_thermal_state() {
        assert_eq!(thermal_state_name(gpui::ThermalState::Nominal), "nominal");
        assert_eq!(thermal_state_name(gpui::ThermalState::Fair), "fair");
        assert_eq!(thermal_state_name(gpui::ThermalState::Serious), "serious");
        assert_eq!(thermal_state_name(gpui::ThermalState::Critical), "critical");
    }

    /// `restartApplication` re-executes the running binary. Under a JS runtime
    /// that would reopen `bun`, not the app, so only compiled apps qualify.
    #[test]
    fn only_compiled_apps_can_restart() {
        use std::path::Path;
        fn is_compiled_app(path: &Path) -> bool {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| !matches!(stem, "bun" | "node" | "nodejs"))
        }
        assert!(!is_compiled_app(Path::new("/home/me/.bun/bin/bun")));
        assert!(!is_compiled_app(Path::new("/usr/bin/node")));
        assert!(is_compiled_app(Path::new("/opt/solo/SoloTasks")));
    }
}
