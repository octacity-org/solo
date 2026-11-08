//! SoloRenderer — napi-rs binding exposed to Node.js.
//!
//! Mutation-based API: Solid's universal renderer sends individual mutations
//! (createElement, appendChild, setStyle, etc.) instead of a full JSON tree.
//! Rust maintains a RetainedTree and rebuilds GPUI elements from it each frame.
//!
//! Lifecycle:
//!   const renderer = new SoloRenderer(eventCallback)
//!   renderer.init({ title: 'My App', width: 800, height: 600 })
//!   renderer.createElement(1, "div")     // mutations from Solid renderer
//!   renderer.appendChild(0, 1)
//!   renderer.commitMutations()           // signal batch complete
//!
//! On macOS the renderer runs in a Worker while the main thread runs GPUI's
//! loop (`runMainLoop`); see the worker-mode section below.
#[cfg(ui_thread)]
use futures::{channel::mpsc, StreamExt as _};
use gpui::AppContext as _;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
#[cfg(ui_thread)]
use std::sync::mpsc::{sync_channel, RecvTimeoutError};
use std::sync::{Arc, Mutex};
#[cfg(ui_thread)]
use std::time::Duration;

use crate::custom_elements::{CustomElementRegistry, CustomRenderContext};
use crate::element_tree::EventPayload;
use crate::retained_tree::{DirtyWindows, RetainedTree};
use crate::style::StyleDesc;
use crate::text::{selectable_text, selection_frame_reset, selection_key, SharedSelection};
use crate::theme::Theme;

mod batch;
mod build;
mod styles;
mod view;
mod virtual_list;

pub(crate) use batch::*;
pub(crate) use build::*;
pub(crate) use styles::*;
pub(crate) use view::*;
use virtual_list::*;

gpui::actions!(solo_focus, [FocusNext, FocusPrevious]);

pub(crate) fn init_key_bindings(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("tab", FocusNext, None),
        gpui::KeyBinding::new("shift-tab", FocusPrevious, None),
    ]);
}

/// Abstracted event callback — both production and test renderers use this.
/// Production: wraps ThreadsafeFunction (async, queued on Node.js event loop).
/// Tests: wraps Arc<Mutex<Vec<EventPayload>>> (synchronous collection).
pub(crate) type EventCallback = Arc<dyn Fn(EventPayload) + Send + Sync>;

/// Validate and convert a JS number (f64) to a u64 element ID.
/// JS numbers are f64 — lossless for integers up to 2^53.
pub(crate) fn to_element_id(id: f64) -> Result<u64> {
    if !id.is_finite() || id < 0.0 || id.fract() != 0.0 || id > 9_007_199_254_740_991.0 {
        return Err(Error::from_reason(format!("Invalid element id: {}", id)));
    }
    Ok(id as u64)
}

#[cfg(test)]
mod system_appearance_tests {
    use super::*;

    #[gpui::test]
    fn registers_replaces_and_disposes_system_appearance(cx: &mut gpui::TestAppContext) {
        let handle = cx.add_window(|_, _| {
            SoloView::new(
                Arc::new(Mutex::new(RetainedTree::new())),
                None,
                "Appearance test".into(),
                SharedSelection::default(),
            )
        });
        handle
            .update(cx, |view, window, cx| {
                assert!(view.system_appearance_subscription.is_none());
                assert_eq!(
                    view.set_system_appearance_subscription(Some("first".into()), window, cx)
                        .unwrap(),
                    "light"
                );
                assert!(view.system_appearance_subscription.is_some());
                assert!(view
                    .set_system_appearance_subscription(Some(String::new()), window, cx)
                    .is_err());
                assert!(view.system_appearance_subscription.is_some());
                assert_eq!(
                    view.set_system_appearance_subscription(Some("second".into()), window, cx)
                        .unwrap(),
                    "light"
                );
                assert!(view.system_appearance_subscription.is_some());
                assert_eq!(
                    view.set_system_appearance_subscription(None, window, cx)
                        .unwrap(),
                    "light"
                );
                assert!(view.system_appearance_subscription.is_none());
            })
            .unwrap();
    }
}

#[cfg(test)]
mod application_lifecycle_tests {
    use super::*;

    #[test]
    fn menu_bar_apps_require_explicit_quit() {
        let ordinary = WindowOptions::default();
        assert_eq!(quit_mode_for_options(&ordinary), gpui::QuitMode::LastWindowClosed);

        let background = WindowOptions {
            menu_bar: Some(MenuBarOptions {
                icon_path: "/tmp/solo-menu-icon.png".into(),
                tooltip: Some("Solo".into()),
            }),
            ..WindowOptions::default()
        };
        assert_eq!(quit_mode_for_options(&background), gpui::QuitMode::Explicit);
    }

    #[test]
    fn menu_bar_icon_path_must_be_absolute() {
        let relative = MenuBarOptions {
            icon_path: "assets/icon.png".into(),
            tooltip: None,
        };
        assert!(validate_menu_bar_options(&relative)
            .unwrap_err()
            .to_string()
            .contains("absolute"));

        let absolute = MenuBarOptions {
            icon_path: "/Applications/Newsprint.app/Contents/Resources/menu.png".into(),
            tooltip: None,
        };
        validate_menu_bar_options(&absolute).unwrap();
    }

    #[test]
    fn window_options_reach_gpui() {
        let defaults = to_gpui_window_options(&WindowOptions::default(), gpui::Bounds::default());
        assert!(defaults.focus && defaults.show && defaults.is_movable && defaults.is_minimizable);
        assert_eq!(defaults.kind, gpui::WindowKind::Normal);
        assert_eq!(defaults.tabbing_identifier, None);
        assert!(!defaults.use_display_p3);

        let options = WindowOptions {
            focus: Some(false),
            show: Some(false),
            kind: Some("floating".into()),
            movable: Some(false),
            minimizable: Some(false),
            tabbing_identifier: Some("documents".into()),
            use_display_p3: Some(true),
            ..WindowOptions::default()
        };
        let gpui_options = to_gpui_window_options(&options, gpui::Bounds::default());
        assert!(!gpui_options.focus && !gpui_options.show);
        assert_eq!(gpui_options.kind, gpui::WindowKind::Floating);
        assert!(!gpui_options.is_movable && !gpui_options.is_minimizable);
        assert_eq!(gpui_options.tabbing_identifier.as_deref(), Some("documents"));
        assert!(gpui_options.use_display_p3);

        let popup = WindowOptions { kind: Some("popup".into()), ..WindowOptions::default() };
        assert_eq!(
            to_gpui_window_options(&popup, gpui::Bounds::default()).kind,
            gpui::WindowKind::PopUp
        );
    }

    #[test]
    fn window_kind_must_be_known() {
        for kind in ["normal", "popup", "floating"] {
            let options = WindowOptions { kind: Some(kind.into()), ..WindowOptions::default() };
            validate_window_options(&options).unwrap();
        }
        let options = WindowOptions { kind: Some("modal".into()), ..WindowOptions::default() };
        assert!(validate_window_options(&options)
            .unwrap_err()
            .to_string()
            .contains("kind"));
    }

    #[test]
    fn only_focused_visible_windows_activate_the_app_on_open() {
        assert!(activates_on_open(&WindowOptions::default()));
        for options in [
            WindowOptions { focus: Some(false), ..WindowOptions::default() },
            WindowOptions { show: Some(false), ..WindowOptions::default() },
        ] {
            assert!(!activates_on_open(&options));
        }
    }

    #[test]
    fn css_cursor_names_map_to_gpui_cursors() {
        use gpui::CursorStyle::*;
        for (name, cursor) in [
            ("default", Arrow),
            ("pointer", PointingHand),
            ("text", IBeam),
            ("vertical-text", IBeamCursorForVerticalLayout),
            ("crosshair", Crosshair),
            ("grab", OpenHand),
            ("grabbing", ClosedHand),
            ("w-resize", ResizeLeft),
            ("e-resize", ResizeRight),
            ("ew-resize", ResizeLeftRight),
            ("n-resize", ResizeUp),
            ("s-resize", ResizeDown),
            ("ns-resize", ResizeUpDown),
            // By the native cursor GPUI draws; its doc comments swap these two.
            ("nwse-resize", ResizeUpLeftDownRight),
            ("nesw-resize", ResizeUpRightDownLeft),
            ("col-resize", ResizeColumn),
            ("row-resize", ResizeRow),
            ("not-allowed", OperationNotAllowed),
            ("alias", DragLink),
            ("copy", DragCopy),
            ("context-menu", ContextualMenu),
        ] {
            assert_eq!(css_cursor_style(name), Some(cursor), "{name}");
        }
        assert_eq!(css_cursor_style("wait"), None);
    }

    #[test]
    fn window_zoom_only_toggles_when_the_requested_state_changes() {
        assert!(should_toggle_window_zoom(false, true));
        assert!(should_toggle_window_zoom(true, false));
        assert!(!should_toggle_window_zoom(true, true));
        assert!(!should_toggle_window_zoom(false, false));
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone)]
struct MacWindowConfig {
    options: WindowOptions,
    tree: Arc<Mutex<RetainedTree>>,
    callback: Option<EventCallback>,
    selection: SharedSelection,
}

#[cfg(target_os = "macos")]
struct MacApplicationState {
    window: MacWindowConfig,
    menu_bar: Option<crate::menu_bar::MenuBar>,
    subscriptions: Vec<gpui::Subscription>,
    appearance_token: Option<String>,
    last_appearance: String,
    window_closed: bool,
}

thread_local! {
    /// The main window: the one `init` opens, painting `tree.root_id`.
    #[cfg(target_os = "macos")]
    static GPUI_WINDOW: RefCell<Option<gpui::WindowHandle<SoloView>>> = const { RefCell::new(None) };
    /// Windows opened with `openWindow`, keyed by root element, in opening order.
    #[cfg(target_os = "macos")]
    static SECONDARY_WINDOWS: RefCell<Vec<(u64, gpui::WindowHandle<SoloView>)>> = const { RefCell::new(Vec::new()) };
    #[cfg(target_os = "macos")]
    static MAC_APPLICATION: RefCell<Option<MacApplicationState>> = const { RefCell::new(None) };
}

/// Drop everything a closed window registered for the napi side.
#[cfg(target_os = "macos")]
pub(crate) fn forget_window_state(window: gpui::WindowId) {
    crate::text::forget_window(window);
    crate::automation::forget_window(window);
}

/// Which window a call acts on.
#[derive(Clone, Copy, Debug)]
enum WindowTarget {
    Main,
    /// A window opened with `openWindow`, by its root element.
    Root(u64),
}

impl WindowTarget {
    /// An explicit `windowId` argument: none means the main window.
    fn from_arg(window_id: Option<f64>) -> Result<Self> {
        window_id
            .map(|id| to_element_id(id).map(Self::Root))
            .transpose()
            .map(|target| target.unwrap_or(Self::Main))
    }

    /// Off macOS there is one window; `openWindow` is macOS-only.
    #[cfg(ui_thread)]
    fn main_only(self) -> Result<()> {
        match self {
            Self::Main => Ok(()),
            Self::Root(_) => Err(Error::from_reason(
                "Multiple windows are supported only on macOS",
            )),
        }
    }

    #[cfg(target_os = "macos")]
    fn handle(self) -> Result<gpui::WindowHandle<SoloView>> {
        match self {
            Self::Main => GPUI_WINDOW
                .with(|window| *window.borrow())
                .ok_or_else(|| Error::from_reason("GPUI window is not initialized")),
            Self::Root(root) => SECONDARY_WINDOWS
                .with(|windows| {
                    windows
                        .borrow()
                        .iter()
                        .find(|(id, _)| *id == root)
                        .map(|(_, window)| *window)
                })
                .ok_or_else(|| Error::from_reason(format!("Window {root} is not open"))),
        }
    }
}

/// Every open window, main first.
#[cfg(target_os = "macos")]
fn all_window_handles() -> Vec<gpui::WindowHandle<SoloView>> {
    GPUI_WINDOW
        .with(|window| *window.borrow())
        .into_iter()
        .chain(SECONDARY_WINDOWS.with(|windows| {
            windows.borrow().iter().map(|(_, window)| *window).collect::<Vec<_>>()
        }))
        .collect()
}

// ── Worker mode (macOS) ─────────────────────────────────────────────
// GPUI/AppKit owns the process main thread through its ordinary blocking loop
// (`runMainLoop`), and the app's JS runs in a Worker. Worker-side renderer
// methods forward closures to the main thread, where the macOS thread-local
// code runs unchanged. The main thread never waits on the Worker; only the
// reverse, so a forwarded call cannot deadlock.
#[cfg(target_os = "macos")]
pub(crate) mod worker_main {
    use futures::channel::mpsc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{LazyLock, Mutex};

    static WORKER_MODE: AtomicBool = AtomicBool::new(false);
    /// Set by `solo dev`: a new app Worker may take over the running window.
    static APP_RELOAD: AtomicBool = AtomicBool::new(false);
    /// Windows to repaint once the queued invalidate task runs.
    pub(crate) static PENDING_INVALIDATE: Mutex<Option<crate::retained_tree::DirtyWindows>> =
        Mutex::new(None);
    pub(crate) type Task = Box<dyn FnOnce() + Send>;
    #[allow(clippy::type_complexity)]
    pub(crate) static TASKS: LazyLock<(
        mpsc::UnboundedSender<Task>,
        Mutex<Option<mpsc::UnboundedReceiver<Task>>>,
    )> = LazyLock::new(|| {
        let (tx, rx) = mpsc::unbounded();
        (tx, Mutex::new(Some(rx)))
    });

    pub(crate) fn enable() {
        WORKER_MODE.store(true, Ordering::SeqCst);
    }
    pub(crate) fn enable_reload() {
        APP_RELOAD.store(true, Ordering::SeqCst);
    }
    pub(crate) fn reload_enabled() -> bool {
        APP_RELOAD.load(Ordering::SeqCst)
    }
    pub(crate) fn enabled() -> bool {
        WORKER_MODE.load(Ordering::SeqCst)
    }
    pub(crate) fn is_main_thread() -> bool {
        // SAFETY: a plain query of the calling thread.
        unsafe { libc::pthread_main_np() == 1 }
    }
    /// True on the Worker side of worker mode: calls must hop to main.
    pub(crate) fn must_forward() -> bool {
        enabled() && !is_main_thread()
    }
}

#[cfg(target_os = "macos")]
thread_local! {
    static MAIN_APP: RefCell<Option<gpui::AsyncApp>> = const { RefCell::new(None) };
    // Never dropped: worker mode ends with the process, not with the loop.
    static MAIN_SHUTDOWN: RefCell<Option<crate::macos_shutdown::ShutdownGuard>> = const { RefCell::new(None) };
}

/// Run `f` on the main thread and wait for its result (worker mode). `f` owns
/// everything it touches: after a timeout the Worker moves on while `f` may
/// still run later.
#[cfg(target_os = "macos")]
pub(crate) fn on_main<R: Send + 'static>(
    f: impl FnOnce() -> Result<R> + Send + 'static,
) -> Result<R> {
    let (tx, rx) = std::sync::mpsc::sync_channel::<std::result::Result<R, String>>(1);
    worker_main::TASKS
        .0
        .unbounded_send(Box::new(move || {
            tx.send(f().map_err(|error| error.reason.clone())).ok();
        }))
        .map_err(|_| Error::from_reason("The main-thread GPUI loop is not running"))?;
    rx.recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| Error::from_reason("Timed out waiting for the main-thread GPUI loop"))?
        .map_err(Error::from_reason)
}

/// Run `f` on the main thread without waiting (worker mode).
#[cfg(target_os = "macos")]
pub(crate) fn on_main_async(f: impl FnOnce() + Send + 'static) -> Result<()> {
    worker_main::TASKS
        .0
        .unbounded_send(Box::new(f))
        .map_err(|_| Error::from_reason("The main-thread GPUI loop is not running"))
}

/// `App` access on the main thread, inside the running main loop.
#[cfg(target_os = "macos")]
pub(crate) fn app_update<R>(f: impl FnOnce(&mut gpui::App) -> R) -> Result<R> {
    let app = MAIN_APP
        .with(|app| app.borrow().clone())
        .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?;
    Ok(app.update(f))
}

/// The app renderer's event callback once `init` has run (main thread).
/// Element-0 application events go here.
#[cfg(target_os = "macos")]
pub(crate) fn app_event_callback() -> Option<EventCallback> {
    MAC_APPLICATION.with(|state| {
        state.borrow().as_ref().and_then(|state| state.window.callback.clone())
    })
}

/// Select worker mode for this process. The generated bootstrap calls this
/// before it starts the app Worker, so the Worker's calls always forward.
#[cfg(target_os = "macos")]
#[napi]
pub fn enable_worker_main_thread() {
    worker_main::enable();
}

/// Dev mode (`solo dev`): a later `init` from a new app Worker replaces the
/// running app's renderer in the same window instead of failing.
#[cfg(target_os = "macos")]
#[napi]
pub fn enable_app_reload() {
    worker_main::enable_reload();
}

/// Element-0 event asking the Worker to exit, so its JS `exit` handlers run.
#[cfg(target_os = "macos")]
const APPLICATION_QUIT_REQUESTED_EVENT: &str = "applicationQuitRequested";

/// How long an AppKit-initiated quit waits for the Worker to exit.
#[cfg(target_os = "macos")]
const WORKER_QUIT_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// End the process from the main thread after the cleanup `on_app_quit` does.
#[cfg(target_os = "macos")]
fn exit_process(code: i32) -> ! {
    crate::context_menu::cancel_all();
    crate::native_view::with_registry(|registry| registry.destroy_all());
    std::process::exit(code)
}

/// Worker mode: end the process with `code`. The Worker calls this from its
/// `exit` handler, after its own JS `exit` handlers have run.
#[cfg(target_os = "macos")]
#[napi]
pub fn request_process_exit(code: i32) -> Result<()> {
    if !worker_main::enabled() {
        return Err(Error::from_reason("requestProcessExit needs worker mode"));
    }
    on_main_async(move || {
        exit_process(code);
    })
}

/// Worker mode: AppKit asked to quit. Ask the Worker to exit through its own
/// `exit` path, and exit anyway when it has not done so within the grace period.
#[cfg(target_os = "macos")]
pub(crate) fn request_worker_quit() {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REQUESTED: AtomicBool = AtomicBool::new(false);
    if REQUESTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let callback = MAC_APPLICATION
        .with(|state| state.borrow().as_ref().and_then(|state| state.window.callback.clone()));
    if callback.is_none() {
        exit_process(0);
    }
    emit_event_full(&callback, 0, APPLICATION_QUIT_REQUESTED_EVENT, |_| {});
    std::thread::spawn(|| {
        std::thread::sleep(WORKER_QUIT_GRACE);
        let _ = on_main_async(|| {
            exit_process(0);
        });
    });
}

/// Run GPUI's ordinary blocking macOS loop on the calling (main) thread.
/// Never returns while the application runs.
#[cfg(target_os = "macos")]
#[napi]
pub fn run_main_loop() -> Result<()> {
    use futures::StreamExt as _;
    use gpui::Platform as _;
    if !worker_main::is_main_thread() {
        return Err(Error::from_reason("runMainLoop must be called on the main thread"));
    }
    worker_main::enable();
    let mut tasks = worker_main::TASKS
        .1
        .lock()
        .unwrap()
        .take()
        .ok_or_else(|| Error::from_reason("runMainLoop was already called"))?;
    // Built here rather than through gpui_platform::application() so Solo can
    // install its menu validator. GPUI's default validator checks only action
    // types; Solo commands share one type but have independent enabled states.
    let platform = Rc::new(gpui_macos::MacPlatform::new(false));
    platform.on_validate_app_menu_command(Box::new(|action| {
        app_update(|cx| crate::application_commands::is_menu_action_available(action, cx))
            .unwrap_or(false)
    }));
    // Accessibility stays off until zed-industries/zed#65186 lands: releasing
    // the AccessKit adapter when a window closes makes AppKit's Touch Bar
    // bookkeeping throw (zed-industries/zed#64819), crashing every
    // openWindow close.
    let application = gpui::Application::new_inaccessible(platform);
    // Deep links and files opened with the app, before or after the app
    // Worker starts; see open_urls.rs.
    application.on_open_urls(crate::open_urls::receive);
    application.run(move |cx| {
        init_key_bindings(cx);
        crate::custom_elements::input::init(cx);
        MAIN_APP.with(|app| *app.borrow_mut() = Some(cx.to_async()));
        match crate::macos_shutdown::install() {
            Ok(guard) => MAIN_SHUTDOWN.with(|slot| *slot.borrow_mut() = Some(guard)),
            Err(error) => log::error!("Failed to install the shutdown delegate: {error}"),
        }
        cx.spawn(async move |_cx| {
            while let Some(task) = tasks.next().await {
                task();
            }
        })
        .detach();
    });
    Ok(())
}

/// Window-close and app-quit cleanup for the main loop.
#[cfg(target_os = "macos")]
fn register_macos_lifecycle(cx: &mut gpui::App) {
    let closed = cx.on_window_closed(|_cx, window_id| {
        forget_window_state(window_id);
        let secondary = SECONDARY_WINDOWS.with(|windows| {
            let mut windows = windows.borrow_mut();
            let index = windows.iter().position(|(_, window)| {
                gpui::AnyWindowHandle::from(*window).window_id() == window_id
            })?;
            Some(windows.remove(index).0)
        });
        if let Some(root) = secondary {
            secondary_window_closed(root);
            return;
        }
        let matches_current = GPUI_WINDOW.with(|stored| {
            stored.borrow().is_some_and(|window| {
                gpui::AnyWindowHandle::from(window).window_id() == window_id
            })
        });
        if matches_current {
            MAC_APPLICATION.with(|state| {
                if let Some(state) = state.borrow_mut().as_mut() {
                    state.window_closed = true;
                }
            });
            GPUI_WINDOW.with(|stored| stored.borrow_mut().take());
            if SECONDARY_WINDOWS.with(|windows| windows.borrow().is_empty()) {
                crate::context_menu::cancel_all();
                crate::native_view::with_registry(|registry| registry.destroy_all());
            } else {
                // Other windows stay open: release only the main window's views.
                let ids = window_config()
                    .map(|config| {
                        let tree = config.tree.lock().unwrap();
                        tree.root_id.map(|root| tree.subtree_ids(root)).unwrap_or_default()
                    })
                    .unwrap_or_default();
                release_window_subtree(&ids);
            }
        }
    });
    let quitting = cx.on_app_quit(|_cx| {
        crate::context_menu::cancel_all();
        MAC_APPLICATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                state.window_closed = true;
            }
        });
        GPUI_WINDOW.with(|window| window.borrow_mut().take());
        SECONDARY_WINDOWS.with(|windows| windows.borrow_mut().clear());
        crate::native_view::with_registry(|registry| registry.destroy_all());
        async {}
    });
    MAC_APPLICATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.subscriptions.extend([closed, quitting]);
        }
    });
}

#[cfg(target_os = "macos")]
fn window_config() -> Option<MacWindowConfig> {
    MAC_APPLICATION.with(|state| state.borrow().as_ref().map(|state| state.window.clone()))
}

/// Cancel the context menus and release the native child views of a closed
/// window's elements. The elements themselves stay until JS destroys them.
#[cfg(target_os = "macos")]
pub(crate) fn release_window_subtree(ids: &[u64]) {
    crate::context_menu::cancel_destroyed(ids);
    crate::native_view::with_registry(|registry| {
        for &id in ids {
            registry.destroy(id);
        }
    });
}

/// A window opened with `openWindow` closed, by the user or by `closeWindow`.
/// Tell JS on its root element, so the app disposes that window's tree.
#[cfg(target_os = "macos")]
fn secondary_window_closed(root: u64) {
    let Some(config) = window_config() else {
        return;
    };
    let ids = {
        let mut tree = config.tree.lock().unwrap();
        tree.window_roots.retain(|id| *id != root);
        tree.subtree_ids(root)
    };
    release_window_subtree(&ids);
    emit_event_full(&config.callback, root, WINDOW_CLOSED_EVENT, |_| {});
}

/// Event on a secondary window's root element after the window closed.
#[cfg(target_os = "macos")]
pub(crate) const WINDOW_CLOSED_EVENT: &str = "windowClosed";

/// Open a window that paints the subtree under `root`, a parentless element
/// already listed in `tree.window_roots`.
#[cfg(target_os = "macos")]
fn open_secondary_window(root: u64, options: WindowOptions) -> Result<()> {
    if worker_main::must_forward() {
        return on_main(move || open_secondary_window(root, options));
    }
    let config = window_config()
        .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?;
    app_update(|cx| -> Result<()> {
        let width = options.width.unwrap_or(800.0);
        let height = options.height.unwrap_or(600.0);
        let title = options.title.clone().unwrap_or_else(|| "Solo".to_string());
        let bounds = gpui::Bounds::centered(
            None,
            gpui::size(gpui::px(width as f32), gpui::px(height as f32)),
            cx,
        );
        // One automation clock for the whole app.
        let clock = GPUI_WINDOW
            .with(|window| *window.borrow())
            .and_then(|window| window.read_with(cx, |view, _| view.clock.clone()).ok());
        let window = cx
            .open_window(to_gpui_window_options(&options, bounds), |_window, cx| {
                cx.new(|_| {
                    SoloView::new(config.tree, config.callback, title, config.selection)
                        .for_window_root(root, clock)
                })
            })
            .map_err(|error| Error::from_reason(format!("Failed to open the GPUI window: {error}")))?;
        SECONDARY_WINDOWS.with(|windows| windows.borrow_mut().push((root, window)));
        if activates_on_open(&options) {
            window
                .update(cx, |_view, window, _cx| window.activate_window())
                .map_err(|error| Error::from_reason(error.to_string()))?;
            cx.activate(true);
        }
        Ok(())
    })?
}

/// Create the status item when the window options ask for a menu-bar app.
#[cfg(target_os = "macos")]
fn create_macos_menu_bar(title: &str) -> Result<()> {
    MAC_APPLICATION.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return Ok(());
        };
        if let Some(menu_options) = state.window.options.menu_bar.as_ref() {
            state.menu_bar = Some(crate::menu_bar::MenuBar::new(menu_options, title)?);
        }
        Ok(())
    })
}

#[cfg(target_os = "macos")]
fn open_macos_window(cx: &mut gpui::App) -> Result<gpui::WindowHandle<SoloView>> {
    let config = MAC_APPLICATION
        .with(|state| state.borrow().as_ref().map(|state| state.window.clone()))
        .ok_or_else(|| Error::from_reason("GPUI application lifecycle is not initialized"))?;
    let width = config.options.width.unwrap_or(800.0);
    let height = config.options.height.unwrap_or(600.0);
    let title = config
        .options
        .title
        .clone()
        .unwrap_or_else(|| "Solo".to_string());
    let bounds = gpui::Bounds::centered(
        None,
        gpui::size(gpui::px(width as f32), gpui::px(height as f32)),
        cx,
    );
    let window_options = config.options.clone();
    let tree = config.tree;
    let callback = config.callback;
    let selection = config.selection;
    let window = cx
        .open_window(
            to_gpui_window_options(&window_options, bounds),
            |_window, cx| {
                cx.new(|_| SoloView::new(tree, callback, title, selection))
            },
        )
        .map_err(|error| Error::from_reason(format!("Failed to open the GPUI window: {error}")))?;
    GPUI_WINDOW.with(|stored| *stored.borrow_mut() = Some(window));
    MAC_APPLICATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.window_closed = false;
        }
    });

    let token = MAC_APPLICATION.with(|state| {
        state
            .borrow()
            .as_ref()
            .and_then(|state| state.appearance_token.clone())
    });
    if token.is_some() {
        let appearance = window
            .update(cx, move |view, window, cx| {
                view.set_system_appearance_subscription(token, window, cx)
            })
            .map_err(|error| Error::from_reason(error.to_string()))??;
        MAC_APPLICATION.with(|state| {
            if let Some(state) = state.borrow_mut().as_mut() {
                state.last_appearance = appearance;
            }
        });
    }
    Ok(window)
}

/// Dev reload: point the running application at a new app Worker's renderer.
/// The process, GPUI and the window survive; everything the previous app
/// created is dropped with its root view or reset here. Window size and
/// `menuBar` options keep their first values until the process restarts.
#[cfg(target_os = "macos")]
fn reattach_macos_application(config: MacWindowConfig, cx: &mut gpui::App) -> Result<()> {
    crate::context_menu::cancel_all();
    crate::context_menu::detach_automation();
    crate::native_view::with_registry(|registry| registry.destroy_all());
    // The previous app's other windows go with it. Taken out of the registry
    // first, so their close never reaches the new app as a windowClosed event.
    for (_, window) in SECONDARY_WINDOWS.with(|windows| std::mem::take(&mut *windows.borrow_mut())) {
        window
            .update(cx, |_view, window, _cx| window.remove_window())
            .ok();
    }
    crate::application_commands::reset_application_commands(config.callback.clone(), cx)?;
    cx.set_quit_mode(quit_mode_for_options(&config.options));
    let callback = config.callback.clone();
    MAC_APPLICATION.with(|state| -> Result<()> {
        let mut state = state.borrow_mut();
        let state = state
            .as_mut()
            .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?;
        if let Some(menu_bar) = state.menu_bar.as_mut() {
            menu_bar.update(None, callback)?;
        }
        state.appearance_token = None;
        state.window = config.clone();
        Ok(())
    })?;
    let title = config
        .options
        .title
        .clone()
        .unwrap_or_else(|| "Solo".to_string());
    let replaced = GPUI_WINDOW.with(|stored| *stored.borrow()).is_some_and(|window| {
        cx.update_window(window.into(), |_root, window, cx| {
            window.set_window_title(&title);
            window.replace_root(cx, |_, _| {
                SoloView::new(config.tree, config.callback, title, config.selection)
            });
        })
        .is_ok()
    });
    if !replaced {
        GPUI_WINDOW.with(|stored| stored.borrow_mut().take());
        open_macos_window(cx)?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn show_macos_window_in_app(cx: &mut gpui::App) -> Result<()> {
    if let Some(window) = GPUI_WINDOW.with(|stored| *stored.borrow()) {
        if window
            .update(cx, |_view, window, _cx| window.activate_window())
            .is_ok()
        {
            cx.activate(true);
            return Ok(());
        }
        GPUI_WINDOW.with(|stored| stored.borrow_mut().take());
    }

    let window = open_macos_window(cx)?;
    window
        .update(cx, |_view, window, _cx| window.activate_window())
        .map_err(|error| Error::from_reason(error.to_string()))?;
    cx.activate(true);
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn show_macos_window() -> Result<()> {
    if worker_main::must_forward() {
        return on_main(show_macos_window);
    }
    app_update(show_macos_window_in_app)?
}

/// Activate a window opened with `openWindow`. Once closed it is gone; the app
/// opens a new one instead.
#[cfg(target_os = "macos")]
fn activate_secondary_window(root: u64) -> Result<()> {
    if worker_main::must_forward() {
        return on_main(move || activate_secondary_window(root));
    }
    let window = WindowTarget::Root(root).handle()?;
    app_update(|cx| {
        window
            .update(cx, |_view, window, _cx| window.activate_window())
            .map_err(|error| Error::from_reason(error.to_string()))?;
        cx.activate(true);
        Ok(())
    })?
}

#[cfg(target_os = "macos")]
fn blur_all_windows() -> Result<()> {
    app_update(|cx| {
        for window in all_window_handles() {
            window.update(cx, |_view, window, cx| window.blur(cx)).ok();
        }
    })
}

#[cfg(target_os = "macos")]
fn close_macos_window(target: WindowTarget) -> Result<()> {
    let window = target.handle()?;
    app_update(|cx| {
        cx.update_window(window.into(), |_view, window, _cx| window.remove_window())
            .map_err(|error| Error::from_reason(error.to_string()))
    })?
}

#[cfg(target_os = "macos")]
pub(crate) fn quit_macos_application() -> Result<()> {
    if worker_main::must_forward() {
        return on_main(quit_macos_application);
    }
    app_update(|cx| cx.quit())
}

#[cfg(target_os = "macos")]
fn set_macos_menu_bar(json: Option<String>) -> Result<()> {
    if worker_main::must_forward() {
        return on_main(move || set_macos_menu_bar(json));
    }
    MAC_APPLICATION.with(|application| {
        let mut application = application.borrow_mut();
        let state = application
            .as_mut()
            .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?;
        let callback = state.window.callback.clone();
        let menu_bar = state.menu_bar.as_mut().ok_or_else(|| {
            Error::from_reason("The application was not created with menuBar options")
        })?;
        menu_bar.update(json.as_deref(), callback)
    })
}

#[cfg(target_os = "macos")]
fn set_macos_system_appearance_subscription(token: Option<String>) -> Result<String> {
    if worker_main::must_forward() {
        return on_main(move || set_macos_system_appearance_subscription(token));
    }
    if token.as_deref().is_some_and(str::is_empty) {
        return Err(Error::from_reason(
            "System appearance subscription token must not be empty",
        ));
    }
    MAC_APPLICATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.appearance_token = token.clone();
        }
    });
    if GPUI_WINDOW.with(|window| window.borrow().is_none()) {
        return MAC_APPLICATION
            .with(|state| {
                state
                    .borrow()
                    .as_ref()
                    .map(|state| state.last_appearance.clone())
            })
            .ok_or_else(|| Error::from_reason("GPUI application is not initialized"));
    }
    let appearance = update_window(move |view, window, cx| {
        view.set_system_appearance_subscription(token, window, cx)
    })??;
    MAC_APPLICATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.last_appearance = appearance.clone();
        }
    });
    Ok(appearance)
}

#[cfg(feature = "profiling")]
pub(crate) fn parse_debug_frame_overlay_mode(mode: &str) -> Result<gpui::DebugFrameOverlayMode> {
    match mode {
        "hidden" => Ok(gpui::DebugFrameOverlayMode::Hidden),
        "minimal" => Ok(gpui::DebugFrameOverlayMode::Minimal),
        "full" => Ok(gpui::DebugFrameOverlayMode::Full),
        other => Err(Error::from_reason(format!(
            "Unknown debug frame overlay mode {other:?}. Use hidden, minimal, or full."
        ))),
    }
}

#[cfg(feature = "profiling")]
pub(crate) fn debug_frame_overlay_mode_name(mode: gpui::DebugFrameOverlayMode) -> &'static str {
    match mode {
        gpui::DebugFrameOverlayMode::Hidden => "hidden",
        gpui::DebugFrameOverlayMode::Minimal => "minimal",
        gpui::DebugFrameOverlayMode::Full => "full",
    }
}

fn record_system_appearance(appearance: &str) {
    #[cfg(target_os = "macos")]
    MAC_APPLICATION.with(|state| {
        if let Some(state) = state.borrow_mut().as_mut() {
            state.last_appearance = appearance.to_string();
        }
    });
    #[cfg(not(target_os = "macos"))]
    let _ = appearance;
}

/// Update the main window.
#[cfg(target_os = "macos")]
fn update_window<R: Send + 'static>(
    update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R
        + Send
        + 'static,
) -> Result<R> {
    update_window_in(WindowTarget::Main, update)
}

#[cfg(target_os = "macos")]
fn update_window_in<R: Send + 'static>(
    target: WindowTarget,
    update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R
        + Send
        + 'static,
) -> Result<R> {
    if worker_main::must_forward() {
        return on_main(move || update_window_local(target, update));
    }
    update_window_local(target, update)
}

/// Non-forwarding body of `update_window_in`.
#[cfg(target_os = "macos")]
fn update_window_local<R>(
    target: WindowTarget,
    update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R,
) -> Result<R> {
    let window = target.handle()?;

    app_update(|cx| {
        window
            .update(cx, update)
            .map_err(|error| Error::from_reason(error.to_string()))
    })?
}

#[cfg(target_os = "macos")]
fn appkit_window(window: &gpui::Window) -> Result<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = HasWindowHandle::window_handle(window)
        .map_err(|error| Error::from_reason(error.to_string()))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(Error::from_reason("Window does not have an AppKit handle"));
    };
    // SAFETY: GPUI owns this NSView for the duration of the synchronous
    // window update, and AppKit retains the returned NSWindow.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    view.window()
        .ok_or_else(|| Error::from_reason("The macOS window is closed"))
}

fn should_toggle_window_zoom(current: bool, requested: bool) -> bool {
    current != requested
}

/// Update only the window, without leasing its root `SoloView` entity.
/// Input dispatch can synchronously draw and render the root, so calling it
/// from `WindowHandle<SoloView>::update` would lease `SoloView` twice.
#[cfg(all(target_os = "macos", feature = "automation"))]
fn update_window_for_input<R: Send + 'static>(
    target: WindowTarget,
    update: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> R + Send + 'static,
) -> Result<R> {
    if worker_main::must_forward() {
        return on_main(move || update_window_for_input(target, update));
    }
    let window = target.handle()?;

    app_update(|cx| {
        cx.update_window(window.into(), move |_view, window, cx| update(window, cx))
            .map_err(|error| Error::from_reason(error.to_string()))
    })?
}

/// Repaint the windows in `dirty`. From the Worker, bursts of calls
/// coalesce into one main-thread task.
#[cfg(target_os = "macos")]
fn invalidate_windows(dirty: DirtyWindows) -> Result<()> {
    if dirty.is_empty() {
        return Ok(());
    }
    if worker_main::must_forward() {
        let mut pending = worker_main::PENDING_INVALIDATE.lock().unwrap();
        if let Some(pending) = pending.as_mut() {
            pending.merge(dirty);
            return Ok(());
        }
        *pending = Some(dirty);
        drop(pending);
        return on_main_async(|| {
            let dirty = worker_main::PENDING_INVALIDATE
                .lock()
                .unwrap()
                .take()
                .unwrap_or_default();
            if let Err(error) = invalidate_windows(dirty) {
                log::error!("worker-mode invalidate failed: {error}");
            }
        });
    }
    let main = GPUI_WINDOW.with(|window| *window.borrow());
    if dirty.main && main.is_none() {
        let closed = MAC_APPLICATION.with(|state| {
            state
                .borrow()
                .as_ref()
                .is_some_and(|state| state.window_closed)
        });
        // Cancellation promises/timers can commit while native cleanup drains.
        // Keep retained state (also needed by background apps), but never try
        // to paint a window that has already closed.
        if !closed {
            return Err(Error::from_reason("GPUI window is not initialized"));
        }
    }
    let windows: Vec<gpui::WindowHandle<SoloView>> = main
        .filter(|_| dirty.main)
        .into_iter()
        .chain(dirty.roots.iter().filter_map(|root| WindowTarget::Root(*root).handle().ok()))
        .collect();
    app_update(|cx| {
        for window in windows {
            // A window may close between the lookup and this update.
            window
                .update(cx, |_view, window, cx| refresh_view(window, cx))
                .ok();
        }
    })
}

/// Draw every window now instead of at its next display frame.
///
/// Automation hit-tests input and reads bounds against the last drawn frame,
/// and focus handles only exist once a render has seen their element. GPUI
/// draws on the display link (or, behind other windows, when AppKit
/// displays), so a click or keystroke that follows a mutation could otherwise
/// act on the previous frame.
#[cfg(all(target_os = "macos", feature = "automation"))]
fn draw_windows_now() -> Result<()> {
    if worker_main::must_forward() {
        return on_main(draw_windows_now);
    }
    app_update(|cx| {
        for window in all_window_handles() {
            // A window may close between the lookup and these updates.
            window
                .update(cx, |_view, window, cx| {
                    cx.notify();
                    window.refresh();
                })
                .ok();
            // Drawn through the untyped handle: rendering the view needs it
            // not to be leased by the update above.
            cx.update_window(window.into(), |_root, window, cx| {
                window.draw(cx).clear(cx);
                // The frame is drawn but not yet shown. A visible window
                // presents it on its next display link tick.
                if !window.is_visible() {
                    request_appkit_display(window);
                }
            })
            .ok();
        }
    })
}

#[cfg(target_os = "macos")]
fn request_appkit_display(window: &gpui::Window) {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return;
    };
    // SAFETY: GPUI owns this NSView for the duration of the synchronous
    // window update.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<objc2_app_kit::NSView>() };
    view.setNeedsDisplay(true);
}

/// Work for the `solo-ui` thread. Closures carry everything else, so a new
/// window operation needs no variant of its own.
#[cfg(ui_thread)]
enum UiCommand {
    /// Repaint after a mutation batch; kept cheap because it is the hot one.
    Invalidate,
    /// Run against the root view.
    View(Box<dyn FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) + Send>),
    /// Run against the window without leasing the root view, as input
    /// dispatch needs: it can synchronously render that view.
    Window(Box<dyn FnOnce(gpui::AnyView, &mut gpui::Window, &mut gpui::App) + Send>),
}

#[cfg(ui_thread)]
async fn run_ui_commands(
    mut commands: mpsc::UnboundedReceiver<UiCommand>,
    window: gpui::WindowHandle<SoloView>,
    cx: &mut gpui::AsyncApp,
) {
    while let Some(command) = commands.next().await {
        let result = match command {
            UiCommand::Invalidate => window.update(cx, |_view, window, cx| refresh_view(window, cx)),
            UiCommand::View(update) => window.update(cx, update),
            UiCommand::Window(update) => cx.update_window(window.into(), update),
        };
        if let Err(error) = result {
            log::error!("Failed to handle GPUI UI command: {error:#}");
        }
    }
    cx.update(|cx| cx.quit());
}

/// Wait for a reply from the `solo-ui` thread. `query` names what was asked
/// in the error messages; none means a general window call.
#[cfg(ui_thread)]
fn wait_ui<T>(receiver: std::sync::mpsc::Receiver<T>, query: Option<&str>) -> Result<T> {
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(value) => Ok(value),
        Err(RecvTimeoutError::Timeout) => Err(Error::from_reason(match query {
            Some(query) => format!("Timed out after 2 seconds waiting for the {query}"),
            None => "Timed out after 2 seconds waiting for the GPUI UI thread".to_string(),
        })),
        // The closure is dropped unrun when the window is gone.
        Err(RecvTimeoutError::Disconnected) => Err(Error::from_reason(match query {
            Some(query) => format!("The GPUI UI thread stopped during the {query}"),
            None => "The GPUI window closed or its UI thread stopped before answering".to_string(),
        })),
    }
}

/// Repaint `window` after a change made outside its render.
fn refresh_view(window: &mut gpui::Window, cx: &mut gpui::Context<SoloView>) {
    cx.notify();
    window.refresh();
    // An occluded window has no display link, so GPUI would not paint this
    // change until it becomes visible. Ask AppKit to display it, as GPUI does
    // before a reveal, so painted bounds and text stay current behind other
    // windows.
    #[cfg(target_os = "macos")]
    if !window.is_visible() {
        request_appkit_display(window);
    }
}

#[cfg(ui_thread)]
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// The main GPUI renderer exposed to Node.js.
#[napi]
pub struct SoloRenderer {
    event_callback: Mutex<Option<Arc<ThreadsafeFunction<EventPayload>>>>,
    tree: Arc<Mutex<RetainedTree>>,
    initialized: Arc<Mutex<bool>>,
    file_dialog_state: Arc<Mutex<crate::file_dialogs::DialogState>>,
    alert_state: Arc<Mutex<crate::file_dialogs::DialogState>>,
    /// Shared with SoloView so napi methods can read the live selection
    /// without an App context. Paint and napi calls can use different threads.
    selection: SharedSelection,
    #[cfg(ui_thread)]
    ui_commands: Mutex<Option<mpsc::UnboundedSender<UiCommand>>>,
}

#[napi]
impl SoloRenderer {
    /// The window `id` is painted in. Resolved here, on the calling thread,
    /// because the tree lives with the renderer, not on the main thread.
    fn window_of(&self, id: u64) -> WindowTarget {
        match self.tree.lock().unwrap().window_root_of(id) {
            Some(root) => WindowTarget::Root(root),
            None => WindowTarget::Main,
        }
    }

    /// `window_of` for an optional element; none means the main window.
    #[cfg(feature = "automation")]
    fn window_of_arg(&self, element_id: Option<f64>) -> Result<WindowTarget> {
        Ok(match element_id {
            Some(id) => self.window_of(to_element_id(id)?),
            None => WindowTarget::Main,
        })
    }

    fn event_callback_for_view(&self) -> Option<EventCallback> {
        self.event_callback.lock().unwrap().clone().map(|tsf| {
            Arc::new(move |payload: EventPayload| {
                tsf.call(Ok(payload), ThreadsafeFunctionCallMode::NonBlocking);
            }) as EventCallback
        })
    }

    #[cfg(ui_thread)]
    fn send_ui_command(&self, command: UiCommand) -> Result<()> {
        self.ui_commands
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?
            .unbounded_send(command)
            .map_err(|_| Error::from_reason("The GPUI UI thread is not running"))
    }

    /// Run `update` against the root view of `target`'s window and return its
    /// result. On macOS this is `update_window_in`, which forwards from the
    /// Worker; elsewhere the closure runs on the `solo-ui` thread. `query`
    /// names the call in the timeout errors there.
    fn query_window<R: Send + 'static>(
        &self,
        target: WindowTarget,
        query: Option<&'static str>,
        update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R
            + Send
            + 'static,
    ) -> Result<R> {
        #[cfg(target_os = "macos")]
        {
            let _ = query;
            return update_window_in(target, update);
        }
        #[cfg(ui_thread)]
        {
            target.main_only()?;
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::View(Box::new(move |view, window, cx| {
                response.send(update(view, window, cx)).ok();
            })))?;
            return wait_ui(receiver, query);
        }
        #[cfg(not(any(target_os = "macos", ui_thread)))]
        {
            let _ = (target, query, update);
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    fn in_window<R: Send + 'static>(
        &self,
        target: WindowTarget,
        update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R
            + Send
            + 'static,
    ) -> Result<R> {
        self.query_window(target, None, update)
    }

    /// `in_window` for a call with no result: off macOS it is queued, and the
    /// JS thread does not wait for the UI thread.
    fn post_to_window(
        &self,
        target: WindowTarget,
        update: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>)
            + Send
            + 'static,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(target, update);

        #[cfg(ui_thread)]
        {
            target.main_only()?;
            return self.send_ui_command(UiCommand::View(Box::new(update)));
        }
        #[cfg(not(any(target_os = "macos", ui_thread)))]
        {
            let _ = (target, update);
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    /// Run `f` against the window on the GPUI UI thread and wait for its
    /// result; the counterpart of macOS `on_main`.
    #[cfg(all(not(target_os = "macos"), feature = "automation"))]
    fn on_ui<T: Send + 'static>(
        &self,
        f: impl FnOnce(gpui::AnyView, &mut gpui::Window, &mut gpui::App) -> T + Send + 'static,
    ) -> Result<T> {
        #[cfg(ui_thread)]
        {
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::Window(Box::new(move |root, window, cx| {
                response.send(f(root, window, cx)).ok();
            })))?;
            return wait_ui(receiver, None);
        }
        #[cfg(not(ui_thread))]
        {
            let _ = f;
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    /// Run `f` on the thread that paints. Paint-time registries (painted
    /// bounds and text) are its thread-locals, so reading them from the JS
    /// thread would see empty copies.
    #[cfg(feature = "automation")]
    fn on_paint_thread<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
        #[cfg(target_os = "macos")]
        {
            if worker_main::must_forward() {
                return on_main(move || Ok(f()));
            }
            return Ok(f());
        }
        #[cfg(not(target_os = "macos"))]
        self.on_ui(move |_, _, _| f())
    }

    /// Dispatch input to `target`'s window and wait. The root view is not
    /// leased: dispatch can synchronously draw and render it.
    #[cfg(feature = "automation")]
    fn input_in_window<R: Send + 'static>(
        &self,
        target: WindowTarget,
        dispatch: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> R + Send + 'static,
    ) -> Result<R> {
        #[cfg(target_os = "macos")]
        return update_window_for_input(target, dispatch);

        #[cfg(not(target_os = "macos"))]
        {
            #[cfg(ui_thread)]
            target.main_only()?;
            self.on_ui(move |_, window, cx| dispatch(window, cx))
        }
    }

    /// `input_in_window` without waiting off macOS.
    #[cfg(feature = "automation")]
    fn post_input(
        &self,
        target: WindowTarget,
        dispatch: impl FnOnce(&mut gpui::Window, &mut gpui::App) + Send + 'static,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_for_input(target, dispatch);

        #[cfg(ui_thread)]
        {
            target.main_only()?;
            return self.send_ui_command(UiCommand::Window(Box::new(move |_, window, cx| {
                dispatch(window, cx)
            })));
        }
        #[cfg(not(any(target_os = "macos", ui_thread)))]
        {
            let _ = (target, dispatch);
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    /// Repaint the windows in `dirty`.
    fn invalidate(&self, dirty: DirtyWindows) -> Result<()> {
        #[cfg(target_os = "macos")]
        return invalidate_windows(dirty);

        // One window off macOS.
        #[cfg(ui_thread)]
        {
            if dirty.is_empty() {
                return Ok(());
            }
            return self.send_ui_command(UiCommand::Invalidate);
        }

        #[cfg(not(any(target_os = "macos", ui_thread)))]
        {
            let _ = dirty;
            Err(Error::from_reason(
                "The production Solo renderer does not support this operating system",
            ))
        }
    }

    /// Repaint every window, for changes outside the tree.
    fn request_invalidate(&self) -> Result<()> {
        let all = self.tree.lock().unwrap().all_windows();
        self.invalidate(all)
    }

    #[napi(constructor)]
    pub fn new(event_callback: Option<ThreadsafeFunction<EventPayload>>) -> Self {
        let _ = env_logger::try_init();
        Self {
            event_callback: Mutex::new(event_callback.map(Arc::new)),
            tree: Arc::new(Mutex::new(RetainedTree::new())),
            initialized: Arc::new(Mutex::new(false)),
            file_dialog_state: Arc::new(Mutex::new(Default::default())),
            alert_state: Arc::new(Mutex::new(Default::default())),
            selection: SharedSelection::default(),
            #[cfg(ui_thread)]
            ui_commands: Mutex::new(None),
        }
    }

    /// Initialize GPUI using the native event-loop architecture for this OS.
    #[napi]
    pub fn init(&self, options: Option<WindowOptions>) -> Result<()> {
        #[cfg(not(any(target_os = "macos", ui_thread)))]
        {
            let _ = options;
            return Err(Error::from_reason(
                "The production Solo renderer does not support this operating system",
            ));
        }

        #[cfg(target_os = "macos")]
        {
            if !worker_main::enabled() {
                return Err(Error::from_reason(
                    "On macOS, Solo apps run in a Worker started by the bootstrap that \
                     @solo/solid/vite generates. Build with Vite and run the built entry with Bun.",
                ));
            }
            return self.init_worker_mode(options);
        }

        #[cfg(ui_thread)]
        return self.init_threaded(options);
    }

    /// Open the window on the worker-mode main loop. Startup errors return
    /// here, so the Worker's `init()` throws.
    #[cfg(target_os = "macos")]
    fn init_worker_mode(&self, options: Option<WindowOptions>) -> Result<()> {
        let options = options.unwrap_or_default();
        validate_window_options(&options)?;
        if *self.initialized.lock().unwrap() {
            return Err(Error::from_reason("Renderer is already initialized"));
        }
        let title = options.title.clone().unwrap_or_else(|| "Solo".to_string());
        let config = MacWindowConfig {
            options,
            tree: self.tree.clone(),
            callback: self.event_callback_for_view(),
            selection: self.selection.clone(),
        };
        on_main(move || {
            if MAC_APPLICATION.with(|state| state.borrow().is_some()) {
                if worker_main::reload_enabled() {
                    return app_update(|cx| reattach_macos_application(config, cx))?;
                }
                return Err(Error::from_reason(
                    "A GPUI application already exists in this process",
                ));
            }
            let quit_mode = quit_mode_for_options(&config.options);
            let activate = activates_on_open(&config.options);
            MAC_APPLICATION.with(|state| {
                *state.borrow_mut() = Some(MacApplicationState {
                    window: config,
                    menu_bar: None,
                    subscriptions: Vec::new(),
                    appearance_token: None,
                    last_appearance: "light".to_string(),
                    window_closed: false,
                });
            });
            let launched = app_update(|cx| -> Result<()> {
                cx.set_quit_mode(quit_mode);
                open_macos_window(cx)?;
                if activate {
                    cx.activate(true);
                }
                register_macos_lifecycle(cx);
                create_macos_menu_bar(&title)
            })
            .and_then(|launched| launched);
            if launched.is_err() {
                MAC_APPLICATION.with(|state| state.borrow_mut().take());
            }
            launched
        })?;
        // URLs that launched the app, or arrived while a `solo dev` reload
        // swapped Workers. Queued behind init, so the JS side has finished
        // `init` before they arrive.
        on_main_async(crate::open_urls::deliver_pending)?;
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }

    #[cfg(ui_thread)]
    fn init_threaded(&self, options: Option<WindowOptions>) -> Result<()> {
        let options = options.unwrap_or_default();
        validate_window_options(&options)?;
        let activate = activates_on_open(&options);
        if *self.initialized.lock().unwrap() {
            return Err(Error::from_reason("Renderer is already initialized"));
        }

        let width = options.width.unwrap_or(800.0);
        let height = options.height.unwrap_or(600.0);
        let title = options.title.clone().unwrap_or_else(|| "Solo".to_string());
        let window_options = options.clone();
        let tree = self.tree.clone();
        let selection = self.selection.clone();
        let callback = self.event_callback_for_view();
        let (command_sender, command_receiver) = mpsc::unbounded();
        let (startup_sender, startup_receiver) = sync_channel(1);
        let exit_startup_sender = startup_sender.clone();

        std::thread::Builder::new()
            .name("solo-ui".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    gpui_platform::application().run(move |cx| {
                        init_key_bindings(cx);
                        crate::custom_elements::input::init(cx);
                        let bounds = gpui::Bounds::centered(
                            None,
                            gpui::size(gpui::px(width as f32), gpui::px(height as f32)),
                            cx,
                        );
                        let window = match cx.open_window(
                            to_gpui_window_options(&window_options, bounds),
                            |_window, cx| {
                                cx.new(|_| SoloView::new(tree, callback, title, selection))
                            },
                        ) {
                            Ok(window) => window,
                            Err(error) => {
                                startup_sender
                                    .send(Err(format!("Failed to open the GPUI window: {error}")))
                                    .ok();
                                cx.quit();
                                return;
                            }
                        };

                        cx.spawn(async move |cx| {
                            run_ui_commands(command_receiver, window, cx).await;
                        })
                        .detach();
                        if activate {
                            cx.activate(true);
                        }
                        startup_sender.send(Ok(())).ok();
                    });
                }));

                let error = match result {
                    Ok(()) => {
                        "The GPUI event loop exited before initialization completed".to_string()
                    }
                    Err(payload) => format!(
                        "The GPUI UI thread panicked during initialization: {}",
                        panic_message(payload)
                    ),
                };
                exit_startup_sender.try_send(Err(error)).ok();
            })
            .map_err(|error| {
                Error::from_reason(format!("Failed to spawn the GPUI UI thread: {error}"))
            })?;

        startup_receiver
            .recv()
            .map_err(|_| Error::from_reason("The GPUI UI thread stopped during initialization"))?
            .map_err(Error::from_reason)?;

        *self.ui_commands.lock().unwrap() = Some(command_sender);
        *self.initialized.lock().unwrap() = true;
        self.event_callback.lock().unwrap().take();
        Ok(())
    }

    // ── Mutation API ─────────────────────────────────────────────────

    /// Signal that a batch of mutations is complete. Triggers re-render.
    #[napi]
    pub fn commit_mutations(&self) -> Result<()> {
        let dirty = self.tree.lock().unwrap().take_dirty_windows();
        self.invalidate(dirty)
    }

    /// Apply a batch of mutations in a single FFI call.
    ///
    /// Accepts a JSON array of mutation tuples. Each tuple is an array where
    /// the first element is the operation name (string) and remaining elements
    /// are the arguments:
    ///
    ///   ["createElement",    id, "type"]
    ///   ["destroyElement",   id]
    ///   ["appendChild",      parentId, childId]
    ///   ["removeChild",      parentId, childId]
    ///   ["insertBefore",     parentId, childId, beforeId]
    ///   ["setStyle",         id, { ...style } | "{styleJson}"]
    ///   ["setText",          id, "content"]
    ///   ["setEventListener", id, "eventType", true|false]
    ///   ["setRoot",          id]
    ///   ["setCustomProp",      id, "key", value | "{valueJson}"]
    ///   ["setCustomPropValue", id, "key", value]
    ///
    /// Returns accumulated destroyed IDs from all destroyElement ops.
    /// Acquires the tree mutex ONCE for the entire batch.
    #[napi]
    pub fn apply_batch(&self, json: String) -> Result<Vec<f64>> {
        let ops = parse_batch(&json)?;
        let (destroyed, dirty) = {
            let mut tree = self.tree.lock().unwrap();
            let destroyed = apply_batch_to_tree(&mut tree, ops);
            (destroyed, tree.take_dirty_windows())
        };
        self.invalidate(dirty)?;
        Ok(destroyed)
    }

    #[napi]
    pub fn is_initialized(&self) -> bool {
        *self.initialized.lock().unwrap()
    }

    /// Open another window painting the subtree under `root_id`, a parentless
    /// element already created through the mutation API. Its `windowId` is
    /// `root_id`. When it closes, `root_id` receives a `windowClosed` event.
    /// macOS only.
    #[napi]
    pub fn open_window(&self, root_id: f64, options: Option<WindowOptions>) -> Result<()> {
        let root = to_element_id(root_id)?;
        #[cfg(target_os = "macos")]
        {
            let options = options.unwrap_or_default();
            if options.menu_bar.is_some() {
                return Err(Error::from_reason(
                    "menuBar is an application option; set it on the main window",
                ));
            }
            validate_window_options(&options)?;
            if !*self.initialized.lock().unwrap() {
                return Err(Error::from_reason(
                    "Renderer is not initialized. Call init() first.",
                ));
            }
            self.tree
                .lock()
                .unwrap()
                .add_window_root(root)
                .map_err(Error::from_reason)?;
            let opened = open_secondary_window(root, options);
            if opened.is_err() {
                self.tree.lock().unwrap().window_roots.retain(|id| *id != root);
            }
            return opened;
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (root, options);
            Err(Error::from_reason("Multiple windows are supported only on macOS"))
        }
    }

    /// Reopen a closed background window or activate the existing window.
    /// With `window_id`, activate that window opened with `openWindow`.
    #[napi]
    pub fn show_window(&self, window_id: Option<f64>) -> Result<()> {
        #[cfg(target_os = "macos")]
        return match WindowTarget::from_arg(window_id)? {
            WindowTarget::Main => show_macos_window(),
            WindowTarget::Root(root) => activate_secondary_window(root),
        };

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window_id;
            Err(Error::from_reason(
                "Background window lifecycle is supported only on macOS",
            ))
        }
    }

    /// Close the main window, or the window `window_id`. Background
    /// applications keep running.
    #[napi]
    pub fn close_window(&self, window_id: Option<f64>) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let target = WindowTarget::from_arg(window_id)?;
            if worker_main::must_forward() {
                return on_main(move || close_macos_window(target));
            }
            return close_macos_window(target);
        }

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window_id;
            Err(Error::from_reason(
                "Background window lifecycle is supported only on macOS",
            ))
        }
    }
    /// Enter or leave the standard macOS zoomed state. AppKit owns the saved
    /// normal frame, so restoring uses exactly the platform's previous frame.
    #[napi]
    pub fn set_window_maximized(&self, maximized: bool, window_id: Option<f64>) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(WindowTarget::from_arg(window_id)?, move |_view, window, _cx| {
            let window = appkit_window(window)?;
            if should_toggle_window_zoom(window.isZoomed(), maximized) {
                window.zoom(None);
            }
            Ok(())
        })?;

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (maximized, window_id);
            Err(Error::from_reason(
                "Maximized window state is supported only on macOS",
            ))
        }
    }

    /// Read AppKit's live zoomed state, including native zoom-button changes.
    #[napi]
    pub fn is_window_maximized(&self, window_id: Option<f64>) -> Result<bool> {
        #[cfg(target_os = "macos")]
        return update_window_in(WindowTarget::from_arg(window_id)?, move |_view, window, _cx| {
            Ok(appkit_window(window)?.isZoomed())
        })?;

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window_id;
            Err(Error::from_reason(
                "Maximized window state is supported only on macOS",
            ))
        }
    }

    /// Explicitly terminate the application, including background apps.
    #[napi]
    pub fn quit_application(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return quit_macos_application();

        #[cfg(not(target_os = "macos"))]
        Err(Error::from_reason(
            "Explicit application quit is supported only on macOS",
        ))
    }

    /// Content size of the main window, or of the window `window_id`, in
    /// logical pixels.
    #[napi]
    pub fn get_window_size(&self, window_id: Option<f64>) -> Result<WindowSize> {
        #[cfg(target_os = "macos")]
        return update_window_in(WindowTarget::from_arg(window_id)?, |_view, window, _cx| {
            window_size(window)
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = window_id;
            Err(Error::from_reason(
                "Reading the window size is supported only on macOS",
            ))
        }
    }

    /// Replace application commands atomically. macOS only for now.
    #[napi]
    pub fn set_application_commands(&self, json: String) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let commands = crate::application_commands::parse_application_commands(&json)?;
            return update_window(move |view, window, cx| {
                // init transfers callback ownership into SoloView and clears
                // the constructor slot. Reuse the live view's callback.
                crate::application_commands::replace_application_commands(
                    commands,
                    view.event_callback.clone(),
                    cx,
                )?;
                window.refresh();
                Ok(())
            })?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = json;
            Err(Error::from_reason(
                "Application commands are supported only on macOS",
            ))
        }
    }

    /// Replace the macOS status-item presentation, or restore the initial
    /// icon, tooltip, and built-in Open/Quit menu when passed null.
    #[napi]
    pub fn set_menu_bar(&self, json: Option<String>) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            return set_macos_menu_bar(json);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = json;
            Err(Error::from_reason(
                "Menu-bar configuration is supported only on macOS",
            ))
        }
    }

    /// Show the macOS file-open dialog without blocking the JavaScript or UI
    /// thread. The returned promise resolves to selected paths, or `null` on
    /// cancellation.
    #[napi]
    pub fn select_files(
        &self,
        options_json: String,
    ) -> Result<AsyncTask<crate::file_dialogs::OpenDialogTask>> {
        #[cfg(target_os = "macos")]
        {
            if !*self.initialized.lock().unwrap() {
                return Err(Error::from_reason(
                    "Renderer is not initialized. Call init() first.",
                ));
            }
            let options = crate::file_dialogs::parse_open_options(&options_json)?;
            let lease = crate::file_dialogs::DialogLease::acquire(self.file_dialog_state.clone())?;
            let receiver = update_window(move |_view, _window, cx| {
                cx.prompt_for_paths(options.to_path_prompt_options())
            })?;
            return Ok(crate::file_dialogs::OpenDialogTask::new(receiver, lease));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = options_json;
            Err(Error::from_reason(
                "File dialogs are supported only on macOS",
            ))
        }
    }

    /// Show a native alert (an `NSAlert` sheet on macOS) on the main window,
    /// or on the `openWindow` window whose root is `window_id`. The promise
    /// resolves to the index of the clicked button. One alert may be open at
    /// a time; a second request rejects instead of queueing.
    #[napi]
    pub fn show_alert(
        &self,
        options_json: String,
        window_id: Option<f64>,
    ) -> Result<AsyncTask<crate::alerts::AlertTask>> {
        #[cfg(target_os = "macos")]
        {
            if !*self.initialized.lock().unwrap() {
                return Err(Error::from_reason(
                    "Renderer is not initialized. Call init() first.",
                ));
            }
            let options = crate::alerts::parse_alert_options(&options_json)?;
            let target = WindowTarget::from_arg(window_id)?;
            let lease = crate::file_dialogs::DialogLease::acquire_as(
                self.alert_state.clone(),
                crate::alerts::ALERT_ALREADY_OPEN,
            )?;
            let receiver =
                update_window_in(target, move |_view, window, cx| options.show(window, cx))?;
            return Ok(crate::alerts::AlertTask::new(receiver, lease));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (options_json, window_id);
            Err(Error::from_reason("Alerts are supported only on macOS"))
        }
    }

    /// Show the macOS save-destination dialog without blocking the JavaScript
    /// or UI thread. The returned promise resolves to a path, or `null` on
    /// cancellation. Solo never creates or writes the selected path.
    #[napi]
    pub fn select_save_path(
        &self,
        options_json: String,
    ) -> Result<AsyncTask<crate::file_dialogs::SaveDialogTask>> {
        #[cfg(target_os = "macos")]
        {
            if !*self.initialized.lock().unwrap() {
                return Err(Error::from_reason(
                    "Renderer is not initialized. Call init() first.",
                ));
            }
            let options = crate::file_dialogs::parse_save_options(&options_json)?;
            let lease = crate::file_dialogs::DialogLease::acquire(self.file_dialog_state.clone())?;
            let receiver = update_window(move |_view, _window, cx| {
                cx.prompt_for_new_path(
                    &options.initial_directory,
                    options.suggested_name.as_deref(),
                )
            })?;
            return Ok(crate::file_dialogs::SaveDialogTask::new(receiver, lease));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = options_json;
            Err(Error::from_reason(
                "File dialogs are supported only on macOS",
            ))
        }
    }

    /// Replace or remove the macOS system-appearance observer and return the
    /// current normalized appearance. The token is echoed in change events so
    /// the Solid layer can discard notifications queued for an old owner.
    #[napi]
    pub fn set_system_appearance_subscription(&self, token: Option<String>) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            set_macos_system_appearance_subscription(token)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = token;
            Err(Error::from_reason(
                "System appearance is supported only on macOS",
            ))
        }
    }

    /// `"hidden"` | `"minimal"` | `"full"`. Paints into the scene after layout.
    #[napi]
    pub fn set_debug_frame_overlay(&self, mode: String) -> Result<String> {
        #[cfg(feature = "profiling")]
        {
            let mode = parse_debug_frame_overlay_mode(&mode)?;
            return self.debug_frame_overlay(move |window| window.set_debug_frame_overlay_mode(mode));
        }
        #[cfg(not(feature = "profiling"))]
        {
            let _ = mode;
            Err(profiling_required())
        }
    }

    /// Hidden → minimal → full → hidden.
    #[napi]
    pub fn cycle_debug_frame_overlay(&self) -> Result<String> {
        #[cfg(feature = "profiling")]
        return self.debug_frame_overlay(|window| window.cycle_debug_frame_overlay_mode());

        #[cfg(not(feature = "profiling"))]
        Err(profiling_required())
    }

    #[napi]
    pub fn get_debug_frame_overlay(&self) -> Result<String> {
        #[cfg(feature = "profiling")]
        return self.debug_frame_overlay(|_| {});

        #[cfg(not(feature = "profiling"))]
        Err(profiling_required())
    }

    /// Clears the last 1000 draw samples. Frame count stays.
    #[napi]
    pub fn reset_debug_frame_overlay_stats(&self) -> Result<()> {
        #[cfg(feature = "profiling")]
        return self
            .debug_frame_overlay(|window| window.reset_debug_frame_overlay_stats())
            .map(drop);

        #[cfg(not(feature = "profiling"))]
        Err(profiling_required())
    }

    /// Apply `change` to the main window's overlay and return its mode.
    #[cfg(feature = "profiling")]
    fn debug_frame_overlay(
        &self,
        change: impl FnOnce(&mut gpui::Window) + Send + 'static,
    ) -> Result<String> {
        self.query_window(
            WindowTarget::Main,
            Some("debug frame overlay query"),
            move |_view, window, _cx| {
                change(window);
                debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
            },
        )
    }

    #[napi]
    pub fn set_window_title(&self, title: String, window_id: Option<f64>) -> Result<()> {
        self.post_to_window(WindowTarget::from_arg(window_id)?, move |view, window, cx| {
            view.window_title = title;
            refresh_view(window, cx);
        })
    }

    #[napi]
    pub fn focus_element(&self, element_id: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        self.post_to_window(self.window_of(id), move |view, window, cx| {
            view.focus_element(id, window, cx);
            refresh_view(window, cx);
        })
    }

    #[napi]
    pub fn blur(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            if worker_main::must_forward() {
                return on_main(blur_all_windows);
            }
            return blur_all_windows();
        }

        #[cfg(ui_thread)]
        return self.send_ui_command(UiCommand::Window(Box::new(|_, window, cx| window.blur(cx))));

        #[cfg(not(any(target_os = "macos", ui_thread)))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    // ── Selection API ────────────────────────────────────────────────

    /// The current text selection joined in document order, or null.
    #[napi]
    pub fn get_selected_text(&self) -> Option<String> {
        self.selection.lock().selected_text()
    }

    /// Drop the current selection and request a repaint.
    #[napi]
    pub fn clear_selection(&self) -> Result<()> {
        self.selection.lock().clear();
        self.request_invalidate()
    }

    // ── Scroll API ───────────────────────────────────────────────────

    /// Set the scroll offset of a scrollable element.
    /// x and y are negative pixel values (scroll down = more negative y).
    #[napi]
    pub fn scroll_to(&self, element_id: f64, x: f64, y: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        self.post_to_window(self.window_of(id), move |view, window, cx| {
            view.scroll_to(id, x as f32, y as f32);
            refresh_view(window, cx);
        })
    }

    /// Scroll a child into view by its index in the children list.
    #[napi]
    pub fn scroll_to_item(&self, element_id: f64, index: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        let index = index as usize;
        self.post_to_window(self.window_of(id), move |view, window, cx| {
            view.scroll_to_item(id, index);
            refresh_view(window, cx);
        })
    }

    /// Get the current scroll offset of a scrollable element.
    /// Returns [x, y] or null if the element has no scroll handle.
    #[napi]
    pub fn get_scroll_offset(&self, element_id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(element_id)?;
        let offset = self.query_window(
            self.window_of(id),
            Some("GPUI scroll query"),
            move |view, _window, _cx| view.scroll_offset(id),
        )?;
        Ok(offset.map(|[x, y]| vec![x, y]))
    }
}

#[cfg(not(feature = "profiling"))]
fn profiling_required() -> Error {
    Error::from_reason("Debug frame overlays require a build with the profiling feature")
}

#[cfg(feature = "automation")]
#[napi]
impl SoloRenderer {
    /// Draw the latest committed tree in every window before returning, so
    /// automation input and reads see it. The live counterpart of
    /// `TestSoloRenderer::flush`. Off macOS the draw runs on the UI thread,
    /// after every command sent before it, such as a focus change.
    #[napi]
    pub fn flush(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return draw_windows_now();

        // The frame is drawn but not yet shown; GPUI presents it on the
        // window's next frame request.
        #[cfg(not(target_os = "macos"))]
        self.on_ui(|root, window, cx| {
            cx.notify(root.entity_id());
            window.refresh();
            window.draw(cx).clear(cx);
        })
    }

    #[napi]
    pub fn get_automation_tree(&self) -> Result<String> {
        self.request_invalidate()?;
        let bounds = self.on_paint_thread(crate::automation::all_bounds)?;
        let json = self.tree.lock().unwrap().to_json(&bounds);
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    /// JSON array of the trees of the windows opened with `openWindow`.
    #[napi]
    pub fn get_window_automation_trees(&self) -> Result<String> {
        let bounds = self.on_paint_thread(crate::automation::all_bounds)?;
        let json = self.tree.lock().unwrap().windows_to_json(&bounds);
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    #[napi]
    pub fn get_element_bounds(&self, id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(id)?;
        let bounds = self.on_paint_thread(move || crate::automation::get_bounds(id))?;
        Ok(bounds.map(|bounds| vec![bounds.x, bounds.y, bounds.width, bounds.height]))
    }

    #[napi]
    pub fn get_all_text(&self) -> Vec<String> {
        crate::automation::collect_text(&self.tree.lock().unwrap())
    }

    #[napi]
    pub fn get_painted_text(&self) -> Result<Vec<String>> {
        self.on_paint_thread(crate::text::painted_text)
    }

    #[napi]
    pub fn simulate_click(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_click(window, cx, x, y, button.unwrap_or(0));
        })
    }

    #[napi]
    pub fn simulate_mouse_down(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_mouse_down(window, cx, x, y, button.unwrap_or(0));
        })
    }

    #[napi]
    pub fn simulate_mouse_up(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_mouse_up(window, cx, x, y, button.unwrap_or(0));
        })
    }

    #[napi]
    pub fn simulate_mouse_move(
        &self,
        x: f64,
        y: f64,
        pressed_button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_mouse_move(window, cx, x, y, pressed_button);
        })
    }

    #[napi]
    pub fn simulate_scroll_wheel(
        &self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.post_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_scroll_wheel(window, cx, x, y, delta_x, delta_y);
        })
    }

    /// Drag files from outside the app over `(x, y)` without dropping them.
    #[napi]
    pub fn simulate_file_drag_over(
        &self,
        paths: Vec<String>,
        x: f64,
        y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_file_drag_over(window, cx, paths, x, y);
        })
    }

    /// Drop files from outside the app at `(x, y)`, as a Finder drop would.
    #[napi]
    pub fn simulate_file_drop(
        &self,
        paths: Vec<String>,
        x: f64,
        y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        self.input_in_window(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_file_drop(window, cx, paths, x, y);
        })
    }

    /// Type a space-separated sequence of keystrokes, e.g. `"a enter cmd-s"`.
    ///
    /// Text is inserted through GPUI's input pipeline, so it lands in whatever
    /// element has focus. That is the point: this is a real key event, not a
    /// JS-bound value write, so `onChange` and `onKeyDown` fire as they would
    /// for a human. Focus the target first — see `focus_element`.
    #[napi]
    pub fn simulate_keystrokes(&self, keystrokes: String, element_id: Option<f64>) -> Result<()> {
        // Parsed here, on the calling thread, so a malformed key is reported
        // to the caller even where the dispatch is queued without waiting.
        let keys = crate::automation::parse_keystrokes(&keystrokes).map_err(Error::from_reason)?;

        #[cfg(target_os = "macos")]
        if crate::context_menu::dispatch_keys(&keys) {
            return Ok(());
        }

        self.post_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_keystrokes(window, cx, &keys);
        })
    }

    /// A key down with no matching key up.
    ///
    /// Fires `onKeyDown` but does **not** insert text — use
    /// [`Self::simulate_keystrokes`] to type. Matches
    /// `TestSoloRenderer::simulate_key_down`.
    #[napi]
    pub fn simulate_key_down(
        &self,
        keystroke: String,
        is_held: Option<bool>,
        element_id: Option<f64>,
    ) -> Result<()> {
        let key = crate::automation::parse_keystroke(&keystroke).map_err(Error::from_reason)?;
        let is_held = is_held.unwrap_or(false);

        #[cfg(target_os = "macos")]
        if crate::context_menu::dispatch_keys(std::slice::from_ref(&key)) {
            return Ok(());
        }

        self.post_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_key_down(window, cx, &key, is_held);
        })
    }

    /// A key up, paired with [`Self::simulate_key_down`].
    #[napi]
    pub fn simulate_key_up(&self, keystroke: String, element_id: Option<f64>) -> Result<()> {
        let key = crate::automation::parse_keystroke(&keystroke).map_err(Error::from_reason)?;
        self.post_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_key_up(window, cx, &key);
        })
    }

    #[napi]
    pub fn clock_pause(&self) -> Result<f64> {
        self.with_clock(|clock| clock.pause())
    }

    #[napi]
    pub fn clock_set(&self, now_ms: f64) -> Result<f64> {
        self.with_clock(move |clock| clock.set_ms(now_ms))
    }

    #[napi]
    pub fn clock_fast_forward(&self, delta_ms: f64) -> Result<f64> {
        self.with_clock(move |clock| clock.fast_forward_ms(delta_ms))
    }

    #[napi]
    pub fn clock_resume(&self) -> Result<f64> {
        self.with_clock(|clock| clock.resume())
    }

    /// Change the clock every window shares, then repaint the main window.
    fn with_clock(
        &self,
        change: impl FnOnce(&crate::automation::AutomationClock) -> f64 + Send + 'static,
    ) -> Result<f64> {
        self.in_window(WindowTarget::Main, move |view, _window, cx| {
            let now_ms = change(&view.clock);
            cx.notify();
            now_ms
        })
    }

    #[napi]
    pub fn capture_screenshot(&self, path: String) -> Result<()> {
        #[cfg(all(target_os = "macos", feature = "test-support"))]
        {
            let image = update_window(move |_view, window, cx| {
                cx.notify();
                window.refresh();
                window.render_to_image()
            })?
            .map_err(|e| Error::from_reason(format!("Screenshot capture failed: {}", e)))?;
            image
                .save(&path)
                .map_err(|e| Error::from_reason(format!("Failed to save screenshot: {}", e)))?;
            Ok(())
        }

        // The capture reads back what the X server shows, so it waits for the
        // latest tree to be presented. Next-frame callbacks run before that
        // frame draws and presents: frame N shows the tree, frame N+1 reads.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::Window(Box::new(move |root, window, cx| {
                cx.notify(root.entity_id());
                window.refresh();
                window.on_next_frame(move |window, _cx| {
                    window.on_next_frame(move |window, _cx| {
                        response.send(capture_x11_window(window, &path)).ok();
                    });
                });
            })))?;
            return match receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(result) => result.map_err(Error::from_reason),
                Err(RecvTimeoutError::Timeout) => Err(Error::from_reason(
                    "Timed out after 2 seconds waiting for a frame to capture",
                )),
                Err(RecvTimeoutError::Disconnected) => Err(Error::from_reason(
                    "The GPUI window closed or its UI thread stopped before the capture",
                )),
            };
        }

        #[cfg(not(any(
            all(target_os = "macos", feature = "test-support"),
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            let _ = path;
            Err(Error::from_reason(
                "captureScreenshot needs the test-support build on macOS",
            ))
        }
    }
}

/// Save the window's pixels as a PNG, read back from the X server. GPUI's
/// `render_to_image` exists only on macOS and its test platform.
#[cfg(all(any(target_os = "linux", target_os = "freebsd"), feature = "automation"))]
fn capture_x11_window(window: &gpui::Window, path: &str) -> std::result::Result<(), String> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, ImageOrder};

    let handle = HasWindowHandle::window_handle(window).map_err(|e| e.to_string())?;
    let xid = match handle.as_raw() {
        RawWindowHandle::Xcb(handle) => handle.window.get(),
        RawWindowHandle::Xlib(handle) => handle.window as u32,
        _ => return Err("captureScreenshot on Linux needs an X11 window; Wayland is not supported".into()),
    };
    let x11_error = |e: &dyn std::fmt::Display| format!("X11 screenshot failed: {e}");
    let (conn, _) = x11rb::connect(None).map_err(|e| x11_error(&e))?;
    let geometry = conn
        .get_geometry(xid)
        .map_err(|e| x11_error(&e))?
        .reply()
        .map_err(|e| x11_error(&e))?;
    let (width, height) = (geometry.width, geometry.height);
    let reply = conn
        .get_image(ImageFormat::Z_PIXMAP, xid, 0, 0, width, height, !0)
        .map_err(|e| x11_error(&e))?
        .reply()
        .map_err(|e| x11_error(&e))?;

    // TrueColor at 24 or 32 bits of depth is stored as 32-bit B, G, R, X
    // pixels; at 32 bits per pixel scanlines carry no padding.
    let setup = conn.setup();
    let bits_per_pixel = setup
        .pixmap_formats
        .iter()
        .find(|format| format.depth == reply.depth)
        .map(|format| format.bits_per_pixel);
    if bits_per_pixel != Some(32) || setup.image_byte_order != ImageOrder::LSB_FIRST {
        return Err(format!(
            "X11 screenshot failed: unsupported pixel format (depth {}, {:?} bits per pixel)",
            reply.depth, bits_per_pixel
        ));
    }
    let byte_len = usize::from(width) * usize::from(height) * 4;
    let Some(data) = reply.data.get(..byte_len) else {
        return Err("X11 screenshot failed: the server returned a short image".into());
    };
    let rgba = data
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], 255])
        .collect();
    image::RgbaImage::from_raw(u32::from(width), u32::from(height), rgba)
        .ok_or_else(|| "X11 screenshot failed: image size mismatch".to_string())?
        .save(path)
        .map_err(|e| format!("Failed to save screenshot: {e}"))
}

#[cfg(ui_thread)]
impl Drop for SoloRenderer {
    fn drop(&mut self) {
        self.ui_commands.lock().unwrap().take();
    }
}

/// The tree mutation methods `SoloRenderer` and `TestSoloRenderer` share.
/// Each expands to its own `#[napi] impl` block over the type's `tree`, so
/// the two renderers cannot drift apart on the protocol. Callers need
/// `napi_derive::napi` in scope.
macro_rules! tree_mutation_api {
    ($renderer:ty) => {
        #[napi]
        impl $renderer {
            #[napi]
            pub fn create_element(&self, id: f64, element_type: String) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.create_element(id, element_type);
                Ok(())
            }

            /// Destroy an element and all descendants. Returns array of destroyed IDs
            /// so JS can clean up event handlers for the entire subtree.
            #[napi]
            pub fn destroy_element(&self, id: f64) -> napi::Result<Vec<f64>> {
                let id = $crate::renderer::to_element_id(id)?;
                let mut tree = self.tree.lock().unwrap();
                let destroyed = tree.destroy_element(id);
                Ok(destroyed.iter().map(|&id| id as f64).collect())
            }

            #[napi]
            pub fn append_child(&self, parent_id: f64, child_id: f64) -> napi::Result<()> {
                let parent_id = $crate::renderer::to_element_id(parent_id)?;
                let child_id = $crate::renderer::to_element_id(child_id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.append_child(parent_id, child_id);
                Ok(())
            }

            #[napi]
            pub fn remove_child(&self, parent_id: f64, child_id: f64) -> napi::Result<()> {
                let parent_id = $crate::renderer::to_element_id(parent_id)?;
                let child_id = $crate::renderer::to_element_id(child_id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.remove_child(parent_id, child_id);
                Ok(())
            }

            #[napi]
            pub fn insert_before(&self, parent_id: f64, child_id: f64, before_id: f64) -> napi::Result<()> {
                let parent_id = $crate::renderer::to_element_id(parent_id)?;
                let child_id = $crate::renderer::to_element_id(child_id)?;
                let before_id = $crate::renderer::to_element_id(before_id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.insert_before(parent_id, child_id, before_id);
                Ok(())
            }

            #[napi]
            pub fn set_style(&self, id: f64, style_json: String) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let style: $crate::style::StyleDesc = serde_json::from_str(&style_json)
                    .map_err(|e| napi::Error::from_reason(format!("Failed to parse style: {}", e)))?;
                let mut tree = self.tree.lock().unwrap();
                tree.set_style(id, style);
                Ok(())
            }

            #[napi]
            pub fn set_text(&self, id: f64, content: String) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.set_text(id, content);
                Ok(())
            }

            #[napi]
            pub fn set_event_listener(&self, id: f64, event_type: String, has_handler: bool) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.set_event_listener(id, event_type, has_handler);
                Ok(())
            }

            /// Set the root element (called from appendChildToContainer).
            #[napi]
            pub fn set_root(&self, id: f64) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let mut tree = self.tree.lock().unwrap();
                tree.set_root(id);
                Ok(())
            }

            /// Set a custom prop on an element (for non-div/text elements like input, editor, diff).
            /// Key is the prop name, value is JSON-encoded.
            #[napi]
            pub fn set_custom_prop(&self, id: f64, key: String, value_json: String) -> napi::Result<()> {
                let id = $crate::renderer::to_element_id(id)?;
                let value: serde_json::Value = serde_json::from_str(&value_json)
                    .map_err(|e| napi::Error::from_reason(format!("Failed to parse custom prop value: {}", e)))?;
                let mut tree = self.tree.lock().unwrap();
                tree.set_custom_prop(id, key, value);
                Ok(())
            }

            /// Get a custom prop value from an element. Returns JSON string or null.
            #[napi]
            pub fn get_custom_prop(&self, id: f64, key: String) -> napi::Result<Option<String>> {
                let id = $crate::renderer::to_element_id(id)?;
                let tree = self.tree.lock().unwrap();
                Ok(tree
                    .get_custom_prop(id, &key)
                    .map(|v| serde_json::to_string(v).unwrap_or_default()))
            }
        }
    };
}
#[cfg(all(feature = "test-support", target_os = "macos"))]
pub(crate) use tree_mutation_api;

tree_mutation_api!(SoloRenderer);

// ── Event emission ───────────────────────────────────────────────────

/// Helper to convert a GPUI Point<Pixels> to (f64, f64).
pub(crate) fn point_to_xy(p: gpui::Point<gpui::Pixels>) -> (f64, f64) {
    (f64::from(f32::from(p.x)), f64::from(f32::from(p.y)))
}

/// Convert GPUI MouseButton to our u32 encoding: 0=left, 1=middle, 2=right.
pub(crate) fn mouse_button_to_u32(button: gpui::MouseButton) -> u32 {
    match button {
        gpui::MouseButton::Left => 0,
        gpui::MouseButton::Middle => 1,
        gpui::MouseButton::Right => 2,
        gpui::MouseButton::Navigate(_) => 3,
    }
}

/// General-purpose event emitter. Builds a default EventPayload, lets the
/// caller customize it via a closure, then sends it through the callback.
/// Production: queues on Node.js event loop via ThreadsafeFunction.
/// Tests: pushes to a synchronous Vec for drainEvents().
pub(crate) fn emit_event_full(
    callback: &Option<EventCallback>,
    element_id: u64,
    event_type: &str,
    build: impl FnOnce(&mut EventPayload),
) {
    if let Some(cb) = callback {
        let mut payload = EventPayload {
            element_id: element_id as f64,
            event_type: event_type.to_string(),
            ..Default::default()
        };
        build(&mut payload);
        cb(payload);
    }
}

// ── Types ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
#[napi(object)]
pub struct WindowSize {
    pub width: f64,
    pub height: f64,
}

pub(crate) fn window_size(window: &gpui::Window) -> WindowSize {
    let size = window.viewport_size();
    WindowSize {
        width: f64::from(f32::from(size.width)),
        height: f64::from(f32::from(size.height)),
    }
}

#[derive(Debug, Clone)]
#[napi(object)]
pub struct WindowOptions {
    pub title: Option<String>,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub min_width: Option<f64>,
    pub min_height: Option<f64>,
    pub resizable: Option<bool>,
    pub fullscreen: Option<bool>,
    /// Plain alpha transparency. Prefer `window_background` when you need blur.
    pub transparent: Option<bool>,
    /// Hide the native titlebar so the app can draw chrome under the traffic lights.
    pub titlebar_transparent: Option<bool>,
    /// `"opaque"` | `"transparent"` | `"blurred"`. `transparent: true` is the
    /// same as `"transparent"` when this is unset.
    pub window_background: Option<String>,
    pub traffic_light_x: Option<f64>,
    pub traffic_light_y: Option<f64>,
    /// Keep the application alive after its window closes and install a macOS
    /// status item with built-in Open and Quit actions.
    pub menu_bar: Option<MenuBarOptions>,
    /// Make the window key when it opens. Defaults to true. `false` also
    /// leaves the application where it is instead of bringing it forward.
    pub focus: Option<bool>,
    /// Show the window when it opens. Defaults to true. A hidden window
    /// appears on `showWindow`.
    pub show: Option<bool>,
    /// `"normal"` (default) | `"popup"`, above all other windows without
    /// activating the app | `"floating"`, above normal windows.
    pub kind: Option<String>,
    /// Whether the user can drag the window. Defaults to true.
    pub movable: Option<bool>,
    /// Whether the user can minimize the window. Defaults to true.
    pub minimizable: Option<bool>,
    /// Windows sharing an identifier can join as native tabs (macOS).
    pub tabbing_identifier: Option<String>,
    /// Render in the Display P3 color space (macOS). Defaults to false.
    pub use_display_p3: Option<bool>,
}

#[derive(Debug, Clone)]
#[napi(object)]
pub struct MenuBarOptions {
    /// Absolute path to a template image displayed in the macOS menu bar.
    pub icon_path: String,
    pub tooltip: Option<String>,
}

fn quit_mode_for_options(options: &WindowOptions) -> gpui::QuitMode {
    if options.menu_bar.is_some() {
        gpui::QuitMode::Explicit
    } else {
        gpui::QuitMode::LastWindowClosed
    }
}

fn validate_menu_bar_options(options: &MenuBarOptions) -> Result<()> {
    if !std::path::Path::new(&options.icon_path).is_absolute() {
        return Err(Error::from_reason(
            "Menu-bar iconPath must be an absolute file path",
        ));
    }
    Ok(())
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: Some("Solo".to_string()),
            width: Some(800.0),
            height: Some(600.0),
            min_width: None,
            min_height: None,
            resizable: Some(true),
            fullscreen: Some(false),
            transparent: Some(false),
            titlebar_transparent: Some(false),
            window_background: None,
            traffic_light_x: None,
            traffic_light_y: None,
            menu_bar: None,
            focus: None,
            show: None,
            kind: None,
            movable: None,
            minimizable: None,
            tabbing_identifier: None,
            use_display_p3: None,
        }
    }
}

/// Checks the window options a window cannot open with.
pub(crate) fn validate_window_options(options: &WindowOptions) -> Result<()> {
    if let Some(kind) = options.kind.as_deref() {
        if window_kind(kind).is_none() {
            return Err(Error::from_reason(format!(
                "Window kind must be \"normal\", \"popup\" or \"floating\", not {kind:?}"
            )));
        }
    }
    if let Some(menu_bar) = options.menu_bar.as_ref() {
        validate_menu_bar_options(menu_bar)?;
    }
    Ok(())
}

fn window_kind(kind: &str) -> Option<gpui::WindowKind> {
    match kind {
        "normal" => Some(gpui::WindowKind::Normal),
        "popup" => Some(gpui::WindowKind::PopUp),
        "floating" => Some(gpui::WindowKind::Floating),
        _ => None,
    }
}

/// Whether opening this window should bring the application forward. A window
/// opened unfocused or hidden leaves the frontmost application alone.
fn activates_on_open(options: &WindowOptions) -> bool {
    options.focus.unwrap_or(true) && options.show.unwrap_or(true)
}

fn to_gpui_window_options(
    options: &WindowOptions,
    bounds: gpui::Bounds<gpui::Pixels>,
) -> gpui::WindowOptions {
    let title = options.title.clone().unwrap_or_else(|| "Solo".to_string());
    let titlebar_transparent = options.titlebar_transparent.unwrap_or(false);
    let traffic_light_position = match (options.traffic_light_x, options.traffic_light_y) {
        (Some(x), Some(y)) => Some(gpui::point(gpui::px(x as f32), gpui::px(y as f32))),
        _ => None,
    };
    let window_background = match options.window_background.as_deref() {
        Some("transparent") => gpui::WindowBackgroundAppearance::Transparent,
        Some("blurred") => gpui::WindowBackgroundAppearance::Blurred,
        Some("opaque") => gpui::WindowBackgroundAppearance::Opaque,
        _ if options.transparent.unwrap_or(false) => gpui::WindowBackgroundAppearance::Transparent,
        _ => gpui::WindowBackgroundAppearance::Opaque,
    };
    let window_min_size = match (options.min_width, options.min_height) {
        (Some(width), Some(height)) => {
            Some(gpui::size(gpui::px(width as f32), gpui::px(height as f32)))
        }
        _ => None,
    };
    let window_bounds = if options.fullscreen.unwrap_or(false) {
        gpui::WindowBounds::Fullscreen(bounds)
    } else {
        gpui::WindowBounds::Windowed(bounds)
    };
    gpui::WindowOptions {
        window_bounds: Some(window_bounds),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: titlebar_transparent,
            traffic_light_position,
        }),
        focus: options.focus.unwrap_or(true),
        show: options.show.unwrap_or(true),
        // Validated where the window is requested.
        kind: options
            .kind
            .as_deref()
            .and_then(window_kind)
            .unwrap_or(gpui::WindowKind::Normal),
        is_movable: options.movable.unwrap_or(true),
        is_resizable: options.resizable.unwrap_or(true),
        is_minimizable: options.minimizable.unwrap_or(true),
        window_background,
        window_min_size,
        tabbing_identifier: options.tabbing_identifier.clone(),
        use_display_p3: options.use_display_p3.unwrap_or(false),
        ..Default::default()
    }
}

#[cfg(target_os = "macos")]
#[napi]
impl SoloRenderer {
    /// Present a lifecycle-managed native menu without blocking the app loop.
    #[napi]
    pub fn show_context_menu(
        &self,
        json: String,
    ) -> Result<AsyncTask<crate::context_menu::ContextMenuTask>> {
        let tree = self.tree.clone();
        // The menu opens in the owner element's window; `show` validates the id.
        let owner = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|value| value["elementId"].as_f64())
            .and_then(|id| to_element_id(id).ok());
        let target = owner.map_or(WindowTarget::Main, |id| self.window_of(id));
        let scripted = crate::context_menu::automation_attached();
        let task = update_window_in(target, move |_view, window, _cx| {
            crate::context_menu::show(&json, &tree.lock().unwrap(), window, scripted)
        })??;
        Ok(AsyncTask::new(task))
    }

    #[napi]
    pub fn cancel_context_menu(&self, request_id: f64) -> Result<()> {
        crate::context_menu::cancel(to_element_id(request_id)?);
        Ok(())
    }

    /// An automation client attached. Context menus opened from now on are
    /// scripted: they take keys only from automation and never open on screen,
    /// so they stay independent of the user's own input and of app focus.
    #[napi]
    pub fn attach_automation(&self) {
        crate::context_menu::attach_automation();
    }

    /// Evaluate an expression in a mounted macOS WebView without blocking JS.
    #[napi(js_name = "evaluateWebviewJavaScript")]
    pub fn evaluate_webview_javascript(
        &self,
        element_id: f64,
        script: String,
    ) -> Result<AsyncTask<crate::native_view::webview::EvaluateJavaScriptTask>> {
        let element_id = to_element_id(element_id)?;
        if !*self.initialized.lock().unwrap() {
            return Err(Error::from_reason(
                "Renderer is not initialized. Call init() first.",
            ));
        }
        let receiver = update_window_in(self.window_of(element_id), move |_view, _window, _cx| {
            crate::native_view::with_registry(|registry| {
                registry
                    .evaluate_javascript(element_id, &script)
                    .map_err(|error| Error::from_reason(error.to_string()))
            })
        })??;
        Ok(crate::native_view::webview::EvaluateJavaScriptTask::new(receiver))
    }

    /// Resolve when the intended macOS WebView document has finished loading.
    #[napi]
    pub fn wait_webview_ready(
        &self,
        element_id: f64,
    ) -> Result<AsyncTask<crate::native_view::webview::WebviewReadyTask>> {
        let element_id = to_element_id(element_id)?;
        if !*self.initialized.lock().unwrap() {
            return Err(Error::from_reason(
                "Renderer is not initialized. Call init() first.",
            ));
        }
        let receiver = update_window_in(self.window_of(element_id), move |_view, _window, _cx| {
            crate::native_view::with_registry(|registry| {
                registry
                    .wait_for_ready(element_id)
                    .map_err(|error| Error::from_reason(error.to_string()))
            })
        })??;
        Ok(crate::native_view::webview::WebviewReadyTask::new(receiver))
    }

    /// Resolve a pending native WebView navigation request.
    #[napi]
    pub fn allow_webview_navigation(&self, element_id: f64, navigation_id: f64) -> Result<()> {
        self.decide_webview_navigation(element_id, navigation_id, true)
    }

    /// Cancel a pending native WebView navigation request.
    #[napi]
    pub fn cancel_webview_navigation(&self, element_id: f64, navigation_id: f64) -> Result<()> {
        self.decide_webview_navigation(element_id, navigation_id, false)
    }

    fn decide_webview_navigation(
        &self,
        element_id: f64,
        navigation_id: f64,
        allow: bool,
    ) -> Result<()> {
        let element_id = to_element_id(element_id)?;
        let navigation_id = to_element_id(navigation_id)?;
        update_window_in(self.window_of(element_id), move |_view, _window, _cx| {
            let resolved = crate::native_view::with_registry(|registry| {
                registry.decide_navigation(element_id, navigation_id, allow)
            });
            if resolved {
                Ok(())
            } else {
                Err(Error::from_reason(format!(
                    "WebView navigation request {navigation_id} is no longer pending"
                )))
            }
        })?
    }
}
