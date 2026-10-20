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
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use futures::{channel::mpsc, StreamExt as _};
use gpui::AppContext as _;
use napi::bindgen_prelude::*;
use napi::threadsafe_function::{ThreadsafeFunction, ThreadsafeFunctionCallMode};
use napi_derive::napi;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use std::sync::mpsc::{sync_channel, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
use std::time::Duration;

use crate::custom_elements::{CustomElementRegistry, CustomRenderContext};
use crate::element_tree::EventPayload;
use crate::retained_tree::RetainedTree;
use crate::style::{parse_color_hex, StyleDesc};
use crate::text::{selectable_text, selection_frame_reset, selection_key, SharedSelection};
use crate::theme::Theme;

gpui::actions!(solo_focus, [FocusNext, FocusPrevious]);

pub(crate) fn init_key_bindings(cx: &mut gpui::App) {
    cx.bind_keys([
        gpui::KeyBinding::new("tab", FocusNext, None),
        gpui::KeyBinding::new("shift-tab", FocusPrevious, None),
    ]);
}

/// Parse a CSS font-weight value (string or number) into a GPUI FontWeight.
/// Accepts named keywords ("bold", "semibold"), numeric strings ("700"),
/// and raw numbers (700). Falls back to 400 (normal) for unrecognized values.
fn parse_font_weight(value: &crate::style::FontWeightValue) -> gpui::FontWeight {
    match value {
        crate::style::FontWeightValue::Num(n) => gpui::FontWeight((*n as f32).clamp(1.0, 1000.0)),
        crate::style::FontWeightValue::Str(s) => {
            let lower = s.trim().to_ascii_lowercase();
            match lower.as_str() {
                "100" | "thin" => gpui::FontWeight(100.0),
                "200" | "extralight" | "extra-light" => gpui::FontWeight(200.0),
                "300" | "light" => gpui::FontWeight(300.0),
                "400" | "normal" => gpui::FontWeight(400.0),
                "500" | "medium" => gpui::FontWeight(500.0),
                "600" | "semibold" | "semi-bold" => gpui::FontWeight(600.0),
                "700" | "bold" => gpui::FontWeight(700.0),
                "800" | "extrabold" | "extra-bold" => gpui::FontWeight(800.0),
                "900" | "black" => gpui::FontWeight(900.0),
                _ => lower
                    .parse::<f32>()
                    .map(|n| gpui::FontWeight(n.clamp(1.0, 1000.0)))
                    .unwrap_or(gpui::FontWeight(400.0)),
            }
        }
    }
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
    /// Shared scroll handles — each SoloView writes its own during render(),
    /// platform-local handlers read from here for programmatic scroll control.
    /// ScrollHandle is Rc<RefCell<...>> so its methods (set_offset, offset,
    /// scroll_to_item) work without an App context. Keyed by window; element
    /// IDs are unique across windows, so lookups search them all.
    static SCROLL_HANDLES: RefCell<HashMap<gpui::WindowId, HashMap<u64, gpui::ScrollHandle>>> = RefCell::new(HashMap::new());
    static VIRTUAL_LIST_STATES: RefCell<HashMap<gpui::WindowId, HashMap<u64, gpui::ListState>>> = RefCell::new(HashMap::new());
}

fn scroll_handle(id: u64) -> Option<gpui::ScrollHandle> {
    SCROLL_HANDLES.with(|cell| cell.borrow().values().find_map(|handles| handles.get(&id).cloned()))
}

fn virtual_list_state(id: u64) -> Option<gpui::ListState> {
    VIRTUAL_LIST_STATES.with(|cell| cell.borrow().values().find_map(|states| states.get(&id).cloned()))
}

/// Drop everything a closed window registered for the napi side.
#[cfg(target_os = "macos")]
pub(crate) fn forget_window_state(window: gpui::WindowId) {
    SCROLL_HANDLES.with(|handles| handles.borrow_mut().remove(&window));
    VIRTUAL_LIST_STATES.with(|states| states.borrow_mut().remove(&window));
    crate::text::forget_window(window);
    crate::automation::forget_window(window);
}

/// Which window a call acts on.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug)]
enum WindowTarget {
    Main,
    /// A window opened with `openWindow`, by its root element.
    Root(u64),
}

#[cfg(target_os = "macos")]
impl WindowTarget {
    /// An explicit `windowId` argument: none means the main window.
    fn from_arg(window_id: Option<f64>) -> Result<Self> {
        window_id
            .map(|id| to_element_id(id).map(Self::Root))
            .transpose()
            .map(|target| target.unwrap_or(Self::Main))
    }

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
    pub(crate) static INVALIDATE_PENDING: AtomicBool = AtomicBool::new(false);
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
fn app_update<R>(f: impl FnOnce(&mut gpui::App) -> R) -> Result<R> {
    let app = MAIN_APP
        .with(|app| app.borrow().clone())
        .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?;
    Ok(app.update(f))
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
    gpui::Application::new_inaccessible(platform).run(move |cx| {
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
    crate::native_view::with_registry(|registry| registry.destroy_all());
    // The previous app's other windows go with it. Taken out of the registry
    // first, so their close never reaches the new app as a windowClosed event.
    for (_, window) in SECONDARY_WINDOWS.with(|windows| std::mem::take(&mut *windows.borrow_mut())) {
        window
            .update(cx, |_view, window, _cx| window.remove_window())
            .ok();
    }
    SCROLL_HANDLES.with(|handles| handles.borrow_mut().clear());
    VIRTUAL_LIST_STATES.with(|states| states.borrow_mut().clear());
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

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
#[cfg(feature = "profiling")]
fn recv_debug_frame_overlay_mode(
    receiver: std::sync::mpsc::Receiver<String>,
) -> Result<String> {
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(mode) => Ok(mode),
        Err(RecvTimeoutError::Timeout) => Err(Error::from_reason(
            "Timed out after 2 seconds waiting for the debug frame overlay query",
        )),
        Err(RecvTimeoutError::Disconnected) => Err(Error::from_reason(
            "The GPUI UI thread stopped during the debug frame overlay query",
        )),
    }
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

#[cfg(target_os = "macos")]
fn invalidate_window() -> Result<()> {
    if worker_main::must_forward() {
        use std::sync::atomic::Ordering;
        if worker_main::INVALIDATE_PENDING.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        return on_main_async(|| {
            worker_main::INVALIDATE_PENDING.store(false, Ordering::Release);
            if let Err(error) = invalidate_window() {
                log::error!("worker-mode invalidate failed: {error}");
            }
        });
    }
    let windows = all_window_handles();
    if windows.is_empty() {
        let closed = MAC_APPLICATION.with(|state| {
            state
                .borrow()
                .as_ref()
                .is_some_and(|state| state.window_closed)
        });
        // Cancellation promises/timers can commit while native cleanup drains.
        // Keep retained state (also needed by background apps), but never try
        // to paint a window that has already closed.
        return if closed {
            Ok(())
        } else {
            Err(Error::from_reason("GPUI window is not initialized"))
        };
    }
    app_update(|cx| {
        for window in windows {
            // A window may close between the lookup and this update.
            window
                .update(cx, |_view, window, cx| {
                    cx.notify();
                    window.refresh();
                    // An occluded window has no display link, so GPUI would
                    // not paint this change until it becomes visible. Ask
                    // AppKit to display it, as GPUI does before a reveal, so
                    // painted bounds and text stay current behind other windows.
                    if !window.is_visible() {
                        request_appkit_display(window);
                    }
                })
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
#[cfg(target_os = "macos")]
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

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
enum UiCommand {
    Invalidate,
    SetWindowTitle(String),
    #[cfg(feature = "profiling")]
    SetDebugFrameOverlay(gpui::DebugFrameOverlayMode),
    #[cfg(feature = "profiling")]
    CycleDebugFrameOverlay {
        response: SyncSender<String>,
    },
    #[cfg(feature = "profiling")]
    GetDebugFrameOverlay {
        response: SyncSender<String>,
    },
    #[cfg(feature = "profiling")]
    ResetDebugFrameOverlayStats,
    ScrollTo {
        id: u64,
        x: f32,
        y: f32,
    },
    ScrollToItem {
        id: u64,
        index: usize,
    },
    /// Inject a scroll-wheel event at the GPUI event boundary (automation).
    #[cfg(feature = "automation")]
    SimulateScrollWheel {
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    },
    /// Inject keystrokes at the GPUI event boundary (automation). Carries
    /// parsed keystrokes, not a string: parsing happens on the calling thread
    /// so a malformed key is reported to JS instead of being swallowed by the
    /// fire-and-forget channel.
    #[cfg(feature = "automation")]
    SimulateKeystrokes {
        keys: Vec<gpui::Keystroke>,
    },
    #[cfg(feature = "automation")]
    SimulateKeyDown {
        key: gpui::Keystroke,
        is_held: bool,
    },
    #[cfg(feature = "automation")]
    SimulateKeyUp {
        key: gpui::Keystroke,
    },
    GetScrollOffset {
        id: u64,
        response: SyncSender<Option<[f64; 2]>>,
    },
    FocusElement(u64),
    Blur,
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
fn refresh_ui_window(
    window: gpui::WindowHandle<SoloView>,
    cx: &mut gpui::AsyncApp,
) -> anyhow::Result<()> {
    window.update(cx, |_view, window, cx| {
        cx.notify();
        window.refresh();
    })
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
async fn run_ui_commands(
    mut commands: mpsc::UnboundedReceiver<UiCommand>,
    window: gpui::WindowHandle<SoloView>,
    cx: &mut gpui::AsyncApp,
) {
    while let Some(command) = commands.next().await {
        let result = match command {
            UiCommand::Invalidate => refresh_ui_window(window, cx),
            UiCommand::SetWindowTitle(title) => window.update(cx, move |view, window, cx| {
                view.window_title = title;
                cx.notify();
                window.refresh();
            }),
            #[cfg(feature = "profiling")]
            UiCommand::SetDebugFrameOverlay(mode) => window.update(cx, move |_view, window, _cx| {
                window.set_debug_frame_overlay_mode(mode);
            }),
            #[cfg(feature = "profiling")]
            UiCommand::CycleDebugFrameOverlay { response } => {
                window.update(cx, move |_view, window, _cx| {
                    window.cycle_debug_frame_overlay_mode();
                    response
                        .send(debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).into())
                        .ok();
                })
            }
            #[cfg(feature = "profiling")]
            UiCommand::GetDebugFrameOverlay { response } => {
                window.update(cx, move |_view, window, _cx| {
                    response
                        .send(debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).into())
                        .ok();
                })
            }
            #[cfg(feature = "profiling")]
            UiCommand::ResetDebugFrameOverlayStats => {
                window.update(cx, |_view, window, _cx| {
                    window.reset_debug_frame_overlay_stats();
                })
            },
            UiCommand::ScrollTo { id, x, y } => {
                let offset = gpui::point(gpui::px(x), gpui::px(y));
                if let Some(state) = virtual_list_state(id) {
                    state.set_offset_from_scrollbar(offset);
                } else if let Some(handle) = scroll_handle(id) {
                    handle.set_offset(offset);
                }
                refresh_ui_window(window, cx)
            }
            #[cfg(feature = "automation")]
            UiCommand::SimulateScrollWheel { x, y, delta_x, delta_y } => {
                window.update(cx, move |_view, window, cx| {
                    crate::automation::dispatch_scroll_wheel(window, cx, x as f64, y as f64, delta_x as f64, delta_y as f64);
                })
            }
            #[cfg(feature = "automation")]
            UiCommand::SimulateKeystrokes { keys } => {
                cx.update_window(window.into(), move |_view, window, cx| {
                    crate::automation::dispatch_keystrokes(window, cx, &keys);
                })
            }
            #[cfg(feature = "automation")]
            UiCommand::SimulateKeyDown { key, is_held } => {
                cx.update_window(window.into(), move |_view, window, cx| {
                    crate::automation::dispatch_key_down(window, cx, &key, is_held);
                })
            }
            #[cfg(feature = "automation")]
            UiCommand::SimulateKeyUp { key } => {
                cx.update_window(window.into(), move |_view, window, cx| {
                    crate::automation::dispatch_key_up(window, cx, &key);
                })
            }
            UiCommand::ScrollToItem { id, index } => window.update(cx, move |view, window, cx| {
                if let Some(state) = virtual_list_state(id) {
                    state.scroll_to(gpui::ListOffset {
                        item_ix: index,
                        offset_in_item: gpui::px(0.0),
                    });
                } else if let Some(handle) = scroll_handle(id) {
                    if !scroll_retained_child_into_view(&view.tree, &handle, id, index) {
                        handle.scroll_to_item(index);
                    }
                }
                cx.notify();
                window.refresh();
            }),
            UiCommand::GetScrollOffset { id, response } => {
                let offset = virtual_list_state(id)
                    .map(|state| state.scroll_px_offset_for_scrollbar())
                    .or_else(|| scroll_handle(id).map(|handle| handle.offset()))
                    .map(|offset| {
                        [
                            f64::from(f32::from(offset.x)),
                            f64::from(f32::from(offset.y)),
                        ]
                    });
                response.send(offset).ok();
                Ok(())
            }
            UiCommand::FocusElement(id) => window.update(cx, move |view, window, cx| {
                view.reveal_virtual_list_ancestor(id);
                if let Some(handle) = view.focus_handles.get(&id) {
                    handle.focus(window, cx);
                }
                cx.notify();
                window.refresh();
            }),
            UiCommand::Blur => window.update(cx, |_view, window, _cx| window.blur()),
        };
        if let Err(error) = result {
            log::error!("Failed to handle GPUI UI command: {error:#}");
        }
    }
    cx.update(|cx| cx.quit());
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
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
    /// Shared with SoloView so napi methods can read the live selection
    /// without an App context. Paint and napi calls can use different threads.
    selection: SharedSelection,
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    ui_commands: Mutex<Option<mpsc::UnboundedSender<UiCommand>>>,
}

#[napi]
impl SoloRenderer {
    /// The window `id` is painted in. Resolved here, on the calling thread,
    /// because the tree lives with the renderer, not on the main thread.
    #[cfg(target_os = "macos")]
    fn window_of(&self, id: u64) -> WindowTarget {
        match self.tree.lock().unwrap().window_root_of(id) {
            Some(root) => WindowTarget::Root(root),
            None => WindowTarget::Main,
        }
    }

    /// `window_of` for an optional element; none means the main window.
    #[cfg(all(target_os = "macos", feature = "automation"))]
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

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    fn send_ui_command(&self, command: UiCommand) -> Result<()> {
        self.ui_commands
            .lock()
            .unwrap()
            .as_ref()
            .ok_or_else(|| Error::from_reason("GPUI application is not initialized"))?
            .unbounded_send(command)
            .map_err(|_| Error::from_reason("The GPUI UI thread is not running"))
    }

    fn request_invalidate(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return invalidate_window();

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::Invalidate);

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason(
            "The production Solo renderer does not support this operating system",
        ))
    }

    #[napi(constructor)]
    pub fn new(event_callback: Option<ThreadsafeFunction<EventPayload>>) -> Self {
        let _ = env_logger::try_init();
        Self {
            event_callback: Mutex::new(event_callback.map(Arc::new)),
            tree: Arc::new(Mutex::new(RetainedTree::new())),
            initialized: Arc::new(Mutex::new(false)),
            file_dialog_state: Arc::new(Mutex::new(Default::default())),
            selection: SharedSelection::default(),
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
            ui_commands: Mutex::new(None),
        }
    }

    /// Initialize GPUI using the native event-loop architecture for this OS.
    #[napi]
    pub fn init(&self, options: Option<WindowOptions>) -> Result<()> {
        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
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

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
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
        *self.initialized.lock().unwrap() = true;
        Ok(())
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
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

    #[napi]
    pub fn create_element(&self, id: f64, element_type: String) -> Result<()> {
        let id = to_element_id(id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.create_element(id, element_type);
        Ok(())
    }

    /// Destroy an element and all descendants. Returns array of destroyed IDs
    /// so JS can clean up event handlers for the entire subtree.
    #[napi]
    pub fn destroy_element(&self, id: f64) -> Result<Vec<f64>> {
        let id = to_element_id(id)?;
        let mut tree = self.tree.lock().unwrap();
        let destroyed = tree.destroy_element(id);
        Ok(destroyed.iter().map(|&id| id as f64).collect())
    }

    #[napi]
    pub fn append_child(&self, parent_id: f64, child_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.append_child(parent_id, child_id);
        Ok(())
    }

    #[napi]
    pub fn remove_child(&self, parent_id: f64, child_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.remove_child(parent_id, child_id);
        Ok(())
    }

    #[napi]
    pub fn insert_before(&self, parent_id: f64, child_id: f64, before_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        let before_id = to_element_id(before_id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.insert_before(parent_id, child_id, before_id);
        Ok(())
    }

    #[napi]
    pub fn set_style(&self, id: f64, style_json: String) -> Result<()> {
        let id = to_element_id(id)?;
        let style: StyleDesc = serde_json::from_str(&style_json)
            .map_err(|e| Error::from_reason(format!("Failed to parse style: {}", e)))?;
        let mut tree = self.tree.lock().unwrap();
        tree.set_style(id, style);
        Ok(())
    }

    #[napi]
    pub fn set_text(&self, id: f64, content: String) -> Result<()> {
        let id = to_element_id(id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.set_text(id, content);
        Ok(())
    }

    #[napi]
    pub fn set_event_listener(&self, id: f64, event_type: String, has_handler: bool) -> Result<()> {
        let id = to_element_id(id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.set_event_listener(id, event_type, has_handler);
        Ok(())
    }

    /// Set the root element (called from appendChildToContainer).
    #[napi]
    pub fn set_root(&self, id: f64) -> Result<()> {
        let id = to_element_id(id)?;
        let mut tree = self.tree.lock().unwrap();
        tree.root_id = Some(id);
        Ok(())
    }

    /// Set a custom prop on an element (for non-div/text elements like input, editor, diff).
    /// Key is the prop name, value is JSON-encoded.
    #[napi]
    pub fn set_custom_prop(&self, id: f64, key: String, value_json: String) -> Result<()> {
        let id = to_element_id(id)?;
        let value: serde_json::Value = serde_json::from_str(&value_json)
            .map_err(|e| Error::from_reason(format!("Failed to parse custom prop value: {}", e)))?;
        let mut tree = self.tree.lock().unwrap();
        tree.set_custom_prop(id, key, value);
        Ok(())
    }

    /// Get a custom prop value from an element. Returns JSON string or null.
    #[napi]
    pub fn get_custom_prop(&self, id: f64, key: String) -> Result<Option<String>> {
        let id = to_element_id(id)?;
        let tree = self.tree.lock().unwrap();
        Ok(tree
            .get_custom_prop(id, &key)
            .map(|v| serde_json::to_string(v).unwrap_or_default()))
    }

    /// Signal that a batch of mutations is complete. Triggers re-render.
    #[napi]
    pub fn commit_mutations(&self) -> Result<()> {
        self.request_invalidate()
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
        let ops: Vec<serde_json::Value> = serde_json::from_str(&json)
            .map_err(|e| Error::from_reason(format!("Failed to parse batch: {}", e)))?;
        let mut tree = self.tree.lock().unwrap();
        let destroyed = apply_batch_to_tree(&mut tree, &ops)?;
        drop(tree);
        self.request_invalidate()?;
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
        let mode = parse_debug_frame_overlay_mode(&mode)?;
        #[cfg(all(feature = "profiling", target_os = "macos"))]
        return update_window(move |_view, window, _cx| {
            window.set_debug_frame_overlay_mode(mode);
            debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
        });

        #[cfg(all(
            feature = "profiling",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        {
            self.send_ui_command(UiCommand::SetDebugFrameOverlay(mode))?;
            return self.debug_frame_overlay_mode();
        }

        #[cfg(not(feature = "profiling"))]
        {
            let _ = mode;
            Err(Error::from_reason(
                "Debug frame overlays require a build with the profiling feature",
            ))
        }

        #[cfg(all(
            feature = "profiling",
            not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "freebsd"
            ))
        ))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    /// Hidden → minimal → full → hidden.
    #[napi]
    pub fn cycle_debug_frame_overlay(&self) -> Result<String> {
        #[cfg(all(feature = "profiling", target_os = "macos"))]
        return update_window(move |_view, window, _cx| {
            window.cycle_debug_frame_overlay_mode();
            debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
        });

        #[cfg(all(
            feature = "profiling",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        {
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::CycleDebugFrameOverlay { response })?;
            return recv_debug_frame_overlay_mode(receiver);
        }

        #[cfg(not(feature = "profiling"))]
        return Err(Error::from_reason(
            "Debug frame overlays require a build with the profiling feature",
        ));

        #[cfg(all(
            feature = "profiling",
            not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "freebsd"
            ))
        ))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    #[napi]
    pub fn get_debug_frame_overlay(&self) -> Result<String> {
        self.debug_frame_overlay_mode()
    }

    fn debug_frame_overlay_mode(&self) -> Result<String> {
        #[cfg(all(feature = "profiling", target_os = "macos"))]
        return update_window(|_view, window, _cx| {
            debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
        });

        #[cfg(all(
            feature = "profiling",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        {
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::GetDebugFrameOverlay { response })?;
            recv_debug_frame_overlay_mode(receiver)
        }

        #[cfg(not(feature = "profiling"))]
        return Err(Error::from_reason(
            "Debug frame overlays require a build with the profiling feature",
        ));

        #[cfg(all(
            feature = "profiling",
            not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "freebsd"
            ))
        ))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    /// Clears the last 1000 draw samples. Frame count stays.
    #[napi]
    pub fn reset_debug_frame_overlay_stats(&self) -> Result<()> {
        #[cfg(all(feature = "profiling", target_os = "macos"))]
        return update_window(|_view, window, _cx| {
            window.reset_debug_frame_overlay_stats();
        });

        #[cfg(all(
            feature = "profiling",
            any(target_os = "windows", target_os = "linux", target_os = "freebsd")
        ))]
        return self.send_ui_command(UiCommand::ResetDebugFrameOverlayStats);

        #[cfg(not(feature = "profiling"))]
        return Err(Error::from_reason(
            "Debug frame overlays require a build with the profiling feature",
        ));

        #[cfg(all(
            feature = "profiling",
            not(any(
                target_os = "macos",
                target_os = "windows",
                target_os = "linux",
                target_os = "freebsd"
            ))
        ))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    #[napi]
    pub fn set_window_title(&self, title: String, window_id: Option<f64>) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(WindowTarget::from_arg(window_id)?, move |view, window, cx| {
            view.window_title = title;
            cx.notify();
            window.refresh();
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        {
            if window_id.is_some() {
                return Err(Error::from_reason("Multiple windows are supported only on macOS"));
            }
            return self.send_ui_command(UiCommand::SetWindowTitle(title));
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason(
            "The production Solo renderer does not support this operating system",
        ))
    }

    /// Draw the latest committed tree in every window before returning, so
    /// automation input and reads see it. The live counterpart of
    /// `TestSoloRenderer::flush`. Off macOS frames arrive asynchronously
    /// through the UI thread, and this does nothing.
    #[cfg(feature = "automation")]
    #[napi]
    pub fn flush(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        return draw_windows_now();

        #[cfg(not(target_os = "macos"))]
        Ok(())
    }

    #[napi]
    pub fn focus_element(&self, element_id: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of(id), move |view, window, cx| {
            view.reveal_virtual_list_ancestor(id);
            if let Some(handle) = view.focus_handles.get(&id) {
                handle.focus(window, cx);
            }
            cx.notify();
            window.refresh();
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::FocusElement(id));

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason("Unsupported operating system"))
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

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::Blur);

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
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
    // SoloView syncs scroll handles and virtual list states to thread-local maps.

    /// Set the scroll offset of a scrollable element.
    /// x and y are negative pixel values (scroll down = more negative y).
    #[napi]
    pub fn scroll_to(&self, element_id: f64, x: f64, y: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        #[cfg(target_os = "macos")]
        if worker_main::must_forward() {
            return on_main(move || macos_scroll_to(id, x, y));
        }
        #[cfg(target_os = "macos")]
        return macos_scroll_to(id, x, y);

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::ScrollTo {
            id,
            x: x as f32,
            y: y as f32,
        });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    /// Scroll a child into view by its index in the children list.
    #[napi]
    pub fn scroll_to_item(&self, element_id: f64, index: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        let index = index as usize;
        #[cfg(target_os = "macos")]
        {
            let tree = self.tree.clone();
            if worker_main::must_forward() {
                return on_main(move || macos_scroll_to_item(&tree, id, index));
            }
            return macos_scroll_to_item(&tree, id, index);
        }

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::ScrollToItem { id, index });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    /// Get the current scroll offset of a scrollable element.
    /// Returns [x, y] or null if the element has no scroll handle.
    #[napi]
    pub fn get_scroll_offset(&self, element_id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(element_id)?;
        #[cfg(target_os = "macos")]
        if worker_main::must_forward() {
            return on_main(move || Ok(macos_scroll_offset(id)));
        }
        #[cfg(target_os = "macos")]
        return Ok(macos_scroll_offset(id));

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        {
            let (response, receiver) = sync_channel(1);
            self.send_ui_command(UiCommand::GetScrollOffset { id, response })?;
            return match receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(offset) => Ok(offset.map(|[x, y]| vec![x, y])),
                Err(RecvTimeoutError::Timeout) => Err(Error::from_reason(
                    "Timed out after 2 seconds waiting for the GPUI scroll query",
                )),
                Err(RecvTimeoutError::Disconnected) => Err(Error::from_reason(
                    "The GPUI UI thread stopped during the scroll query",
                )),
            };
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason("Unsupported operating system"))
    }

}

#[cfg(target_os = "macos")]
fn macos_scroll_to(id: u64, x: f64, y: f64) -> Result<()> {
    let offset = gpui::point(gpui::px(x as f32), gpui::px(y as f32));
    if let Some(state) = virtual_list_state(id) {
        state.set_offset_from_scrollbar(offset);
    } else if let Some(handle) = scroll_handle(id) {
        handle.set_offset(offset);
    }
    invalidate_window()
}

#[cfg(target_os = "macos")]
fn macos_scroll_to_item(tree: &Arc<Mutex<RetainedTree>>, id: u64, index: usize) -> Result<()> {
    if let Some(state) = virtual_list_state(id) {
        state.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
    } else if let Some(handle) = scroll_handle(id) {
        if !scroll_retained_child_into_view(tree, &handle, id, index) {
            handle.scroll_to_item(index);
        }
    }
    invalidate_window()
}

#[cfg(target_os = "macos")]
fn macos_scroll_offset(id: u64) -> Option<Vec<f64>> {
    let offset = virtual_list_state(id)
        .map(|state| state.scroll_px_offset_for_scrollbar())
        .or_else(|| scroll_handle(id).map(|handle| handle.offset()))?;
    Some(vec![f64::from(f32::from(offset.x)), f64::from(f32::from(offset.y))])
}

#[cfg(feature = "automation")]
#[napi]
impl SoloRenderer {
    #[cfg(feature = "automation")]
    #[napi]
    pub fn get_automation_tree(&self) -> Result<String> {
        self.request_invalidate()?;
        #[cfg(target_os = "macos")]
        let bounds = if worker_main::must_forward() {
            on_main(|| Ok(crate::automation::all_bounds()))?
        } else {
            crate::automation::all_bounds()
        };
        #[cfg(not(target_os = "macos"))]
        let bounds = crate::automation::all_bounds();
        let tree = self.tree.lock().unwrap();
        let json = tree.to_json(&bounds);
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    /// JSON array of the trees of the windows opened with `openWindow`.
    #[cfg(feature = "automation")]
    #[napi]
    pub fn get_window_automation_trees(&self) -> Result<String> {
        #[cfg(target_os = "macos")]
        let bounds = if worker_main::must_forward() {
            on_main(|| Ok(crate::automation::all_bounds()))?
        } else {
            crate::automation::all_bounds()
        };
        #[cfg(not(target_os = "macos"))]
        let bounds = crate::automation::all_bounds();
        let json = self.tree.lock().unwrap().windows_to_json(&bounds);
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn get_element_bounds(&self, id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(id)?;
        #[cfg(target_os = "macos")]
        if worker_main::must_forward() {
            return on_main(move || Ok(crate::automation::get_bounds(id)))
                .map(|b| b.map(|bounds| vec![bounds.x, bounds.y, bounds.width, bounds.height]));
        }
        Ok(crate::automation::get_bounds(id).map(|bounds| {
            vec![bounds.x, bounds.y, bounds.width, bounds.height]
        }))
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn get_all_text(&self) -> Vec<String> {
        let tree = self.tree.lock().unwrap();
        let mut texts = Vec::new();
        if let Some(root_id) = tree.root_id {
            collect_text(root_id, &tree, &mut texts);
        }
        texts
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn get_painted_text(&self) -> Result<Vec<String>> {
        #[cfg(target_os = "macos")]
        if worker_main::must_forward() {
            return on_main(|| Ok(crate::text::painted_text()));
        }
        Ok(crate::text::painted_text())
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_click(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of_arg(element_id)?, move |_view, window, cx| {
            crate::automation::dispatch_click(window, cx, x, y, button.unwrap_or(0));
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (x, y, button, element_id);
            Err(Error::from_reason("simulateClick is only implemented on macOS"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_mouse_down(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of_arg(element_id)?, move |_view, window, cx| {
            crate::automation::dispatch_mouse_down(window, cx, x, y, button.unwrap_or(0));
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (x, y, button, element_id);
            Err(Error::from_reason("simulateMouseDown is only implemented on macOS"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_mouse_up(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of_arg(element_id)?, move |_view, window, cx| {
            crate::automation::dispatch_mouse_up(window, cx, x, y, button.unwrap_or(0));
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (x, y, button, element_id);
            Err(Error::from_reason("simulateMouseUp is only implemented on macOS"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_mouse_move(
        &self,
        x: f64,
        y: f64,
        pressed_button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of_arg(element_id)?, move |_view, window, cx| {
            crate::automation::dispatch_mouse_move(window, cx, x, y, pressed_button);
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = (x, y, pressed_button, element_id);
            Err(Error::from_reason("simulateMouseMove is only implemented on macOS"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_scroll_wheel(
        &self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        return update_window_in(self.window_of_arg(element_id)?, move |_view, window, cx| {
            crate::automation::dispatch_scroll_wheel(window, cx, x, y, delta_x, delta_y);
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        let _ = element_id;
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::SimulateScrollWheel {
            x: x as f32,
            y: y as f32,
            delta_x: delta_x as f32,
            delta_y: delta_y as f32,
        });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        Err(Error::from_reason("Unsupported operating system"))
    }

    /// Type a space-separated sequence of keystrokes, e.g. `"a enter cmd-s"`.
    ///
    /// Text is inserted through GPUI's input pipeline, so it lands in whatever
    /// element has focus. That is the point: this is a real key event, not a
    /// JS-bound value write, so `onChange` and `onKeyDown` fire as they would
    /// for a human. Focus the target first — see `focus_element`.
    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_keystrokes(&self, keystrokes: String, element_id: Option<f64>) -> Result<()> {
        // Parsed here rather than inside the platform branches so a malformed
        // key is reported to the caller instead of being swallowed by the
        // fire-and-forget UiCommand channel.
        let keys = crate::automation::parse_keystrokes(&keystrokes).map_err(Error::from_reason)?;

        #[cfg(target_os = "macos")]
        if crate::context_menu::dispatch_keys(&keys) {
            return Ok(());
        }

        #[cfg(target_os = "macos")]
        return update_window_for_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_keystrokes(window, cx, &keys);
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        let _ = element_id;
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::SimulateKeystrokes { keys });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            let _ = keys;
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    /// A key down with no matching key up.
    ///
    /// Fires `onKeyDown` but does **not** insert text — use
    /// [`Self::simulate_keystrokes`] to type. Matches
    /// `TestSoloRenderer::simulate_key_down`.
    #[cfg(feature = "automation")]
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

        #[cfg(target_os = "macos")]
        return update_window_for_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_key_down(window, cx, &key, is_held);
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        let _ = element_id;
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::SimulateKeyDown { key, is_held });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            let _ = (key, is_held);
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    /// A key up, paired with [`Self::simulate_key_down`].
    #[cfg(feature = "automation")]
    #[napi]
    pub fn simulate_key_up(&self, keystroke: String, element_id: Option<f64>) -> Result<()> {
        let key = crate::automation::parse_keystroke(&keystroke).map_err(Error::from_reason)?;

        #[cfg(target_os = "macos")]
        return update_window_for_input(self.window_of_arg(element_id)?, move |window, cx| {
            crate::automation::dispatch_key_up(window, cx, &key);
        });

        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        let _ = element_id;
        #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
        return self.send_ui_command(UiCommand::SimulateKeyUp { key });

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            let _ = key;
            Err(Error::from_reason("Unsupported operating system"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn clock_pause(&self) -> Result<f64> {
        #[cfg(target_os = "macos")]
        return update_window(move |view, _window, cx| {
            let now_ms = view.clock.pause();
            cx.notify();
            now_ms
        });

        #[cfg(not(target_os = "macos"))]
        Err(Error::from_reason("clockPause is only implemented on macOS"))
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn clock_set(&self, now_ms: f64) -> Result<f64> {
        #[cfg(target_os = "macos")]
        return update_window(move |view, _window, cx| {
            let now_ms = view.clock.set_ms(now_ms);
            cx.notify();
            now_ms
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = now_ms;
            Err(Error::from_reason("clockSet is only implemented on macOS"))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn clock_fast_forward(&self, delta_ms: f64) -> Result<f64> {
        #[cfg(target_os = "macos")]
        return update_window(move |view, _window, cx| {
            let now_ms = view.clock.fast_forward_ms(delta_ms);
            cx.notify();
            now_ms
        });

        #[cfg(not(target_os = "macos"))]
        {
            let _ = delta_ms;
            Err(Error::from_reason(
                "clockFastForward is only implemented on macOS",
            ))
        }
    }

    #[cfg(feature = "automation")]
    #[napi]
    pub fn clock_resume(&self) -> Result<f64> {
        #[cfg(target_os = "macos")]
        return update_window(move |view, _window, cx| {
            let now_ms = view.clock.resume();
            cx.notify();
            now_ms
        });

        #[cfg(not(target_os = "macos"))]
        Err(Error::from_reason("clockResume is only implemented on macOS"))
    }

    #[cfg(feature = "automation")]
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

        #[cfg(not(all(target_os = "macos", feature = "test-support")))]
        {
            let _ = path;
            Err(Error::from_reason(
                "captureScreenshot needs the test-support build on macOS",
            ))
        }
    }
}

#[cfg(feature = "automation")]
fn collect_text(id: u64, tree: &RetainedTree, texts: &mut Vec<String>) {
    if let Some(element) = tree.elements.get(&id) {
        if let Some(ref content) = element.content {
            texts.push(content.clone());
        }
        for &child_id in &element.children {
            collect_text(child_id, tree, texts);
        }
    }
}

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
impl Drop for SoloRenderer {
    fn drop(&mut self) {
        self.ui_commands.lock().unwrap().take();
    }
}

// ── GPUI View ────────────────────────────────────────────────────────

/// Scroll a retained child into view using its painted bounds.
///
/// GPUI's `ScrollHandle::scroll_to_item` only tracks layout children that its
/// div records internally. Solo also injects absolute automation canvases, so
/// retained child indexes are not guaranteed to match that internal list.
pub(crate) fn scroll_retained_child_into_view(
    tree: &Arc<Mutex<RetainedTree>>,
    handle: &gpui::ScrollHandle,
    parent_id: u64,
    index: usize,
) -> bool {
    let child_id = {
        let tree = tree.lock().unwrap();
        tree.elements
            .get(&parent_id)
            .and_then(|parent| parent.children.get(index))
            .copied()
    };
    let Some(child_id) = child_id else {
        return false;
    };
    let Some(parent) = crate::automation::get_bounds(parent_id) else {
        return false;
    };
    let Some(child) = crate::automation::get_bounds(child_id) else {
        return false;
    };

    let offset = handle.offset();
    let mut x = f64::from(f32::from(offset.x));
    let mut y = f64::from(f32::from(offset.y));
    if child.x < parent.x {
        x += parent.x - child.x;
    } else if child.x + child.width > parent.x + parent.width {
        x += parent.x + parent.width - child.x - child.width;
    }
    if child.y < parent.y {
        y += parent.y - child.y;
    } else if child.y + child.height > parent.y + parent.height {
        y += parent.y + parent.height - child.y - child.height;
    }
    handle.set_offset(gpui::point(gpui::px(x as f32), gpui::px(y as f32)));
    true
}

pub(crate) struct SoloView {
    pub(crate) tree: Arc<Mutex<RetainedTree>>,
    pub(crate) event_callback: Option<EventCallback>,
    pub(crate) window_title: String,
    /// Persistent FocusHandles keyed by element ID.
    /// Created lazily for elements with keyboard or focus/blur listeners.
    /// Handles persist across renders so GPUI maintains focus state.
    pub(crate) focus_handles: HashMap<u64, gpui::FocusHandle>,
    /// Active focus/blur subscriptions keyed by element and event type.
    pub(crate) focus_subscriptions: HashMap<(u64, String), gpui::Subscription>,
    /// Registry for custom element types (input, editor, diff, etc.).
    /// Stores factories (one per type) and live instances (one per element ID).
    pub(crate) custom_registry: CustomElementRegistry,
    /// Persistent ScrollHandles keyed by element ID.
    /// Created lazily for elements with overflow: "scroll" (or per-axis scroll).
    /// Handles persist across renders so GPUI maintains scroll offset state.
    pub(crate) scroll_handles: HashMap<u64, gpui::ScrollHandle>,
    /// Native animation clocks keyed by retained element ID.
    pub(crate) motion_states: HashMap<u64, crate::motion::MotionState>,
    /// Live text selection, shared with the paint closures and the napi methods.
    pub(crate) selection: SharedSelection,
    /// Persistent measurement and scroll state for Solid-backed virtual lists.
    virtual_lists: HashMap<u64, VirtualListEntry>,
    /// Motion / review clock. Live wall time unless automation freezes it.
    pub(crate) clock: crate::automation::AutomationClock,
    /// The single live system-appearance observer, when subscribed by Solid.
    /// Replacing this subscription drops the previous observer first.
    system_appearance_subscription: Option<gpui::Subscription>,
    /// Releases custom-element entities on app quit. Without this, gpui's
    /// exit leak check panics because quitting does not drop root views.
    quit_hook: Option<gpui::Subscription>,
    /// The element this view paints, for a window opened with `openWindow`.
    /// None paints `tree.root_id`, the main window.
    root: Option<u64>,
}

impl SoloView {
    pub(crate) fn new(
        tree: Arc<Mutex<RetainedTree>>,
        event_callback: Option<EventCallback>,
        window_title: String,
        selection: SharedSelection,
    ) -> Self {
        Self {
            tree,
            event_callback,
            window_title,
            focus_handles: HashMap::new(),
            focus_subscriptions: HashMap::new(),
            custom_registry: CustomElementRegistry::with_defaults(),
            scroll_handles: HashMap::new(),
            motion_states: HashMap::new(),
            selection,
            virtual_lists: HashMap::new(),
            clock: crate::automation::AutomationClock::new(),
            system_appearance_subscription: None,
            quit_hook: None,
            root: None,
        }
    }

    /// Paint `root` instead of the tree's main root, sharing `clock`.
    #[cfg(target_os = "macos")]
    pub(crate) fn for_window_root(
        mut self,
        root: u64,
        clock: Option<crate::automation::AutomationClock>,
    ) -> Self {
        self.root = Some(root);
        if let Some(clock) = clock {
            self.clock = clock;
        }
        self
    }

    fn root_id(&self, tree: &RetainedTree) -> Option<u64> {
        self.root.or(tree.root_id)
    }

    pub(crate) fn set_system_appearance_subscription(
        &mut self,
        token: Option<String>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Result<String> {
        if token.as_deref().is_some_and(str::is_empty) {
            return Err(Error::from_reason(
                "System appearance subscription token must not be empty",
            ));
        }

        self.system_appearance_subscription.take();
        let current = window.appearance();

        if let Some(token) = token {
            self.system_appearance_subscription = Some(cx.observe_window_appearance(
                window,
                move |view, window, _cx| {
                    let value = crate::system_appearance::appearance_event_value(
                        &token,
                        window.appearance(),
                    );
                    let appearance = crate::system_appearance::normalized_appearance(
                        window.appearance(),
                    )
                    .to_string();
                    record_system_appearance(&appearance);
                    emit_event_full(
                        &view.event_callback,
                        0,
                        "systemAppearanceChange",
                        |payload| payload.value = Some(value),
                    );
                },
            ));
        }

        Ok(crate::system_appearance::normalized_appearance(current).to_string())
    }

    fn build_virtual_child(
        &mut self,
        list_id: u64,
        index: usize,
        expected_child_id: u64,
        inherited: Inherited,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::prelude::*;

        let row_focus_handle = self.virtual_lists.get_mut(&list_id).and_then(|entry| {
            entry.seen_rows.insert(expected_child_id);
            (entry.child_ids.get(index) == Some(&expected_child_id))
                .then(|| entry.row_focus_handles.get(index).cloned())
                .flatten()
                .flatten()
        });

        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        // Rows can move between this list's snapshot and its layout. Build the
        // snapshot's row while it is still in the list: an empty stand-in is
        // measured as zero height and keeps it until the row is laid out again.
        let child_in_list = tree
            .elements
            .get(&expected_child_id)
            .is_some_and(|child| child.parent == Some(list_id));
        if !child_in_list {
            return gpui::Empty.into_any_element();
        }

        let callback = self.event_callback.clone();
        let now = self.clock.now();
        let mut motion_active = false;
        let mut build_ctx = BuildCtx {
            tree: &tree,
            event_callback: &callback,
            focus_handles: &self.focus_handles,
            scroll_handles: &mut self.scroll_handles,
            custom_registry: &mut self.custom_registry,
            virtual_lists: &mut self.virtual_lists,
            motion_states: &mut self.motion_states,
            now,
            motion_active: &mut motion_active,
            selection: self.selection.clone(),
            inherited,
            root_id: None,
        };
        let child = build_element(expected_child_id, &mut build_ctx, window, cx);
        if motion_active {
            window.request_animation_frame();
        }
        let Some(focus_handle) = row_focus_handle else {
            return child;
        };
        gpui::div()
            .id(gpui::SharedString::from(format!(
                "__solo_virtual_row_{}_{}",
                list_id, expected_child_id
            )))
            .w_full()
            .track_focus(&focus_handle)
            .child(child)
            .into_any_element()
    }

    pub(crate) fn scroll_virtual_list_to_item(&self, id: u64, index: usize) -> bool {
        let Some(entry) = self.virtual_lists.get(&id) else {
            return false;
        };
        entry.state.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
        true
    }

    pub(crate) fn set_virtual_list_offset(&self, id: u64, x: f32, y: f32) -> bool {
        let Some(entry) = self.virtual_lists.get(&id) else {
            return false;
        };
        entry
            .state
            .set_offset_from_scrollbar(gpui::point(gpui::px(x), gpui::px(y)));
        true
    }

    pub(crate) fn virtual_list_offset(&self, id: u64) -> Option<[f64; 2]> {
        let offset = self
            .virtual_lists
            .get(&id)?
            .state
            .scroll_px_offset_for_scrollbar();
        Some([
            f64::from(f32::from(offset.x)),
            f64::from(f32::from(offset.y)),
        ])
    }

    pub(crate) fn reveal_virtual_list_ancestor(&self, id: u64) -> bool {
        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        let mut current = id;
        let location = loop {
            let Some(parent_id) = tree
                .elements
                .get(&current)
                .and_then(|element| element.parent)
            else {
                break None;
            };
            if self.virtual_lists.contains_key(&parent_id) {
                let index = tree
                    .elements
                    .get(&parent_id)
                    .and_then(|parent| parent.children.iter().position(|child| *child == current));
                break index.map(|index| (parent_id, index));
            }
            current = parent_id;
        };
        drop(tree);

        let Some((list_id, index)) = location else {
            return false;
        };
        self.scroll_virtual_list_to_item(list_id, index)
    }
}

/// Everything `build_element` threads through the tree.
///
/// Split into a struct because the recursion needs eight-plus shared references
/// and adding one more to every call site is how this file rots. `window` and
/// `cx` stay separate parameters: they are `&mut` and gpui reborrows them.
pub(crate) struct BuildCtx<'a> {
    pub tree: &'a RetainedTree,
    pub event_callback: &'a Option<EventCallback>,
    pub focus_handles: &'a HashMap<u64, gpui::FocusHandle>,
    pub scroll_handles: &'a mut HashMap<u64, gpui::ScrollHandle>,
    pub custom_registry: &'a mut CustomElementRegistry,
    virtual_lists: &'a mut HashMap<u64, VirtualListEntry>,
    pub motion_states: &'a mut HashMap<u64, crate::motion::MotionState>,
    pub now: std::time::Instant,
    pub motion_active: &'a mut bool,
    pub selection: SharedSelection,
    /// Inherited text state, resolved the way CSS inherits it. The renderer's
    /// own theme only seeds the root selection wash; custom elements resolve
    /// their own theme from their `theme` prop.
    pub inherited: Inherited,
    /// The retained-tree root element, when this build renders from it. The
    /// root div is created without styles by the JS side, so it must be sized
    /// to the window here — otherwise every percentage height below it
    /// resolves against an auto-height parent and collapses to content size,
    /// which makes scroll containers non-scrollable (scroll_max = 0).
    pub root_id: Option<u64>,
}

/// Style properties that cascade into descendants.
#[derive(Clone, Copy)]
pub(crate) struct Inherited {
    /// False once an ancestor sets `userSelect: "none"`.
    pub selectable: bool,
    /// Selection wash colour for this subtree.
    pub selection_wash: gpui::Hsla,
}

impl Inherited {
    fn root(theme: &Theme) -> Self {
        let mut wash = theme.accent;
        wash.a = 0.35;
        Self {
            selectable: true,
            selection_wash: wash,
        }
    }

    /// Apply the inheritable parts of `style` for the subtree below it.
    fn descend(mut self, style: Option<&StyleDesc>) -> Self {
        let Some(style) = style else { return self };
        match style.user_select.as_deref() {
            Some("none") => self.selectable = false,
            Some("text") | Some("auto") => self.selectable = true,
            _ => {}
        }
        if let Some(hex) = style
            .selection_color
            .as_deref()
            .and_then(crate::style::parse_color_hex)
        {
            self.selection_wash = gpui::rgba(hex).into();
        }
        self
    }
}

#[derive(Clone, Copy, PartialEq)]
struct VirtualListConfig {
    alignment: gpui::ListAlignment,
    follow_tail: bool,
    overdraw: f32,
    estimated_item_height: Option<f32>,
}

impl VirtualListConfig {
    fn from_element(element: &crate::retained_tree::RetainedElement) -> Self {
        let prop = |key: &str| element.custom_props.get(key);
        let alignment = match prop("alignment").and_then(serde_json::Value::as_str) {
            Some("bottom") => gpui::ListAlignment::Bottom,
            _ => gpui::ListAlignment::Top,
        };
        let follow_tail = prop("followTail")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let overdraw = prop("overdraw")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(512.0)
            .max(0.0) as f32;
        let estimated_item_height = prop("estimatedItemHeight")
            .and_then(serde_json::Value::as_f64)
            .filter(|height| *height > 0.0)
            .map(|height| height as f32);
        Self {
            alignment,
            follow_tail,
            overdraw,
            estimated_item_height,
        }
    }

    fn make_state(self, focus_handles: &[Option<gpui::FocusHandle>]) -> gpui::ListState {
        let item_count = focus_handles.len();
        let mut state = gpui::ListState::new(item_count, self.alignment, gpui::px(self.overdraw));
        state.splice_focusable(0..item_count, focus_handles.iter().cloned());
        if let Some(height) = self.estimated_item_height {
            state = state.with_uniform_item_height(gpui::px(height));
        }
        if self.follow_tail {
            state.set_follow_mode(gpui::FollowMode::Tail);
        }
        state
    }
}

struct VirtualListEntry {
    state: gpui::ListState,
    config: VirtualListConfig,
    child_ids: Vec<u64>,
    child_revisions: Vec<u64>,
    row_focus_handles: Vec<Option<gpui::FocusHandle>>,
    seen_rows: HashSet<u64>,
}

impl VirtualListEntry {
    fn new(
        config: VirtualListConfig,
        child_ids: Vec<u64>,
        child_revisions: Vec<u64>,
        row_focus_handles: Vec<Option<gpui::FocusHandle>>,
    ) -> Self {
        Self {
            state: config.make_state(&row_focus_handles),
            config,
            child_ids,
            child_revisions,
            row_focus_handles,
            seen_rows: HashSet::new(),
        }
    }

    fn sync(
        &mut self,
        config: VirtualListConfig,
        child_ids: Vec<u64>,
        child_revisions: Vec<u64>,
        focusable_rows: &HashSet<u64>,
        cx: &mut gpui::Context<SoloView>,
    ) {
        let old_rows: HashMap<u64, (u64, Option<gpui::FocusHandle>)> = self
            .child_ids
            .iter()
            .copied()
            .zip(self.child_revisions.iter().copied())
            .zip(self.row_focus_handles.iter().cloned())
            .map(|((id, revision), focus_handle)| (id, (revision, focus_handle)))
            .collect();
        let row_focus_handles: Vec<Option<gpui::FocusHandle>> = child_ids
            .iter()
            .map(|id| {
                focusable_rows.contains(id).then(|| {
                    old_rows
                        .get(id)
                        .and_then(|(_, focus_handle)| focus_handle.clone())
                        .unwrap_or_else(|| cx.focus_handle())
                })
            })
            .collect();
        if self.config != config {
            let scroll_top = self.state.logical_scroll_top();
            let should_follow =
                config.follow_tail && (!self.config.follow_tail || self.state.is_following_tail());
            let mut replacement = Self::new(config, child_ids, child_revisions, row_focus_handles);
            replacement.seen_rows = std::mem::take(&mut self.seen_rows);
            replacement
                .seen_rows
                .retain(|id| replacement.child_ids.contains(id));
            if !should_follow {
                replacement.state.scroll_to(scroll_top);
            }
            *self = replacement;
            return;
        }

        let mut spliced = self.child_ids != child_ids;
        if spliced {
            let prefix = self
                .child_ids
                .iter()
                .zip(&child_ids)
                .take_while(|(old, new)| old == new)
                .count();
            let suffix = self.child_ids[prefix..]
                .iter()
                .rev()
                .zip(child_ids[prefix..].iter().rev())
                .take_while(|(old, new)| old == new)
                .count();
            self.state.splice_focusable(
                prefix..self.child_ids.len().saturating_sub(suffix),
                row_focus_handles[prefix..row_focus_handles.len().saturating_sub(suffix)]
                    .iter()
                    .cloned(),
            );
        }

        for (index, (&id, focus_handle)) in child_ids.iter().zip(&row_focus_handles).enumerate() {
            let focusability_changed = old_rows
                .get(&id)
                .is_some_and(|(_, old_handle)| old_handle.is_some() != focus_handle.is_some());
            if focusability_changed {
                self.state
                    .splice_focusable(index..index + 1, std::iter::once(focus_handle.clone()));
                spliced = true;
            }
        }
        // Spliced rows start without a size hint.
        if spliced {
            if let Some(height) = config.estimated_item_height {
                self.state = self
                    .state
                    .clone()
                    .with_uniform_item_height(gpui::px(height));
            }
        }

        let mut changed_start = None;
        for (index, (&id, &revision)) in child_ids.iter().zip(&child_revisions).enumerate() {
            let changed = old_rows
                .get(&id)
                .is_some_and(|(old_revision, _)| *old_revision != revision);
            match (changed_start, changed) {
                (None, true) => changed_start = Some(index),
                (Some(start), false) => {
                    self.state.remeasure_items(start..index);
                    changed_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = changed_start {
            self.state.remeasure_items(start..child_ids.len());
        }

        self.child_ids = child_ids;
        self.child_revisions = child_revisions;
        self.row_focus_handles = row_focus_handles;
    }
}

impl SoloView {
    /// Sync focus handles with the current element tree.
    /// Creates handles for new focusable elements, subscribes on_focus/on_blur,
    /// and cleans up handles for destroyed elements.
    fn sync_focus_handles(
        &mut self,
        tree: &RetainedTree,
        callback: &Option<EventCallback>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let tab_index = |element: &crate::retained_tree::RetainedElement| {
            element
                .custom_props
                .get("tabIndex")
                .and_then(|value| value.as_i64())
                .and_then(|index| isize::try_from(index).ok())
        };
        let has_commands = crate::application_commands::has_commands(cx);
        let root_id = self.root_id(tree);
        // With several windows on one tree, each view owns only its subtree;
        // otherwise an autoFocus in one window would focus into another.
        let members: Option<std::collections::HashSet<u64>> = (!tree.window_roots.is_empty())
            .then(|| root_id.map(|root| tree.subtree_ids(root)).unwrap_or_default().into_iter().collect());
        let is_member = |id: u64| members.as_ref().is_none_or(|members| members.contains(&id));
        let needs_focus = |element: &crate::retained_tree::RetainedElement| {
            is_member(element.id)
                && ((has_commands && root_id == Some(element.id))
                || matches!(element.element_type.as_str(), "input" | "textarea")
                || tab_index(element).is_some()
                || element.events.contains("keyDown")
                || element.events.contains("keyUp")
                || element.events.contains("focus")
                || element.events.contains("blur"))
        };
        // Create handles for elements that need focus but don't have one yet.
        for (&id, element) in &tree.elements {
            let tab_index = tab_index(element).or_else(|| {
                matches!(element.element_type.as_str(), "input" | "textarea").then_some(0)
            });

            if needs_focus(element) && !self.focus_handles.contains_key(&id) {
                let handle = match tab_index {
                    Some(index) => cx.focus_handle().tab_index(index).tab_stop(index >= 0),
                    None => cx.focus_handle(),
                };
                // Focus once, at creation. Re-focusing every frame would
                // steal focus back from whatever the user clicked next.
                if element.auto_focus {
                    handle.focus(window, cx);
                }
                self.focus_handles.insert(id, handle);
            } else if let (Some(handle), Some(index)) =
                (self.focus_handles.get(&id).cloned(), tab_index)
            {
                self.focus_handles
                    .insert(id, handle.tab_index(index).tab_stop(index >= 0));
            } else if let Some(handle) = self.focus_handles.get(&id).cloned() {
                self.focus_handles.insert(id, handle.tab_stop(false));
            }
        }

        self.focus_subscriptions.retain(|(id, event), _| {
            tree.elements
                .get(id)
                .is_some_and(|element| element.events.contains(event))
        });
        for (&id, element) in &tree.elements {
            let Some(handle) = self.focus_handles.get(&id).cloned() else {
                continue;
            };
            let focus_key = (id, "focus".to_string());
            if element.events.contains("focus")
                && !self.focus_subscriptions.contains_key(&focus_key)
            {
                let callback = callback.clone();
                let subscription = cx.on_focus(&handle, window, move |_this, _window, _cx| {
                    emit_event_full(&callback, id, "focus", |_| {});
                });
                self.focus_subscriptions.insert(focus_key, subscription);
            }
            let blur_key = (id, "blur".to_string());
            if element.events.contains("blur") && !self.focus_subscriptions.contains_key(&blur_key)
            {
                let callback = callback.clone();
                let subscription = cx.on_blur(&handle, window, move |_this, _window, _cx| {
                    emit_event_full(&callback, id, "blur", |_| {});
                });
                self.focus_subscriptions.insert(blur_key, subscription);
            }
        }

        // Clean up handles for elements that no longer exist.
        self.focus_handles
            .retain(|id, _| tree.elements.get(id).is_some_and(&needs_focus));
        // Route global shortcuts through the root capture listener before the
        // first control is focused, without stealing focus from native views.
        if has_commands
            && window.focused(cx).is_none()
            && !crate::application_commands::native_child_has_focus(window)
        {
            if let Some(handle) = root_id.and_then(|id| self.focus_handles.get(&id)) {
                handle.focus(window, cx);
            }
        }
    }
}

impl gpui::Render for SoloView {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::IntoElement;

        if self.quit_hook.is_none() {
            self.quit_hook = Some(cx.on_app_quit(|view, _cx| {
                view.system_appearance_subscription.take();
                view.custom_registry.destroy_all();
                // Native child views (WKWebView) outlive the retained tree, so
                // they need their own teardown or AppKit keeps the web process
                // alive past the window.
                crate::native_view::with_registry(|registry| registry.destroy_all());
                async {}
            }));
        }

        window.set_window_title(&self.window_title);

        // Clone Arc so we don't borrow self.tree — frees self for focus_handles access.
        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        let callback = self.event_callback.clone();

        // Sync focus handles before building elements.
        self.sync_focus_handles(&tree, &callback, window, cx);

        // Ensure custom element instances are destroyed when their IDs disappear.
        self.custom_registry
            .prune_missing(|id| tree.elements.contains_key(&id));

        // Clean up scroll handles for destroyed elements (IDs removed from tree).
        // Scrollability-based cleanup (element still exists but style changed
        // from scroll to non-scroll) is handled inside build_div().
        self.scroll_handles
            .retain(|id, _| tree.elements.contains_key(id));
        self.virtual_lists
            .retain(|id, _| tree.elements.contains_key(id));
        self.motion_states
            .retain(|id, _| tree.elements.contains_key(id));

        // Build the element tree. custom_registry, focus_handles, and scroll_handles
        // are different fields of self, so Rust allows borrowing all simultaneously.
        let theme = Theme::dark();
        let now = self.clock.now();
        let mut motion_active = false;
        let result = match self.root_id(&tree) {
            Some(root_id) => {
                let mut ctx = BuildCtx {
                    tree: &tree,
                    event_callback: &callback,
                    focus_handles: &self.focus_handles,
                    scroll_handles: &mut self.scroll_handles,
                    custom_registry: &mut self.custom_registry,
                    virtual_lists: &mut self.virtual_lists,
                    motion_states: &mut self.motion_states,
                    now,
                    motion_active: &mut motion_active,
                    selection: self.selection.clone(),
                    inherited: Inherited::root(&theme),
                    root_id: Some(root_id),
                };
                build_element(root_id, &mut ctx, window, cx)
            }
            None => gpui::Empty.into_any_element(),
        };

        // The frame reset must paint BEFORE any text, so it is the first child of
        // the root wrapper. Without it the selection registry accumulates stale
        // entries across frames and a drag resolves against elements that are no
        // longer on screen.
        let result = {
            use gpui::prelude::*;
            gpui::div()
                .size_full()
                .capture_key_down(cx.listener(|view, event, window, cx| {
                    crate::application_commands::dispatch_shortcut(event, view, window, cx);
                }))
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .on_action(|_: &FocusPrevious, window, cx| window.focus_prev(cx))
                .child(selection_frame_reset(self.selection.clone()))
                .child(crate::automation::bounds_frame_reset())
                .child(result)
                .into_any_element()
        };

        // Sync scroll handles to thread_local so napi methods (scrollTo,
        // getScrollOffset) can access them without an App context.
        let window_id = window.window_handle().window_id();
        SCROLL_HANDLES.with(|cell| {
            let handles = self
                .scroll_handles
                .iter()
                .map(|(&id, handle)| (id, handle.clone()))
                .collect();
            cell.borrow_mut().insert(window_id, handles);
        });
        VIRTUAL_LIST_STATES.with(|cell| {
            let states = self
                .virtual_lists
                .iter()
                .map(|(&id, entry)| (id, entry.state.clone()))
                .collect();
            cell.borrow_mut().insert(window_id, states);
        });

        if motion_active {
            window.request_animation_frame();
        }

        result
    }
}

// ── Element builders ─────────────────────────────────────────────────

pub(crate) fn build_element(
    id: u64,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::IntoElement;

    let Some(element) = ctx.tree.elements.get(&id) else {
        return gpui::Empty.into_any_element();
    };

    let animated_style = if let Some(source) = element.custom_props.get("motion") {
        let state = match ctx.motion_states.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                match crate::motion::MotionState::new(source, ctx.now) {
                    Ok(state) => entry.insert(state),
                    Err(error) => {
                        log::warn!("Invalid motion description for element {id}: {error}");
                        entry.insert(crate::motion::MotionState::invalid(source, ctx.now))
                    }
                }
            }
        };
        if let Err(error) = state.sync(source, ctx.now) {
            log::warn!("Invalid motion update for element {id}: {error}");
        }
        state.is_valid().then(|| {
            let frame = state.frame(ctx.now);
            *ctx.motion_active |= frame.active;
            let mut resolved = element.style.clone().unwrap_or_default();
            frame.style.apply_to(&mut resolved);
            resolved
        })
    } else {
        ctx.motion_states.remove(&id);
        None
    };
    let style = animated_style.as_ref().or(element.style.as_ref());

    // Inheritable style resolves once here so both built-ins and custom
    // elements see the same cascade.
    let parent_inherited = ctx.inherited;
    ctx.inherited = parent_inherited.descend(style);

    let built = match element.element_type.as_str() {
        "div" => {
            ctx.custom_registry.destroy(id);
            build_div(element, style, ctx, window, cx)
        }
        "text" => {
            ctx.custom_registry.destroy(id);
            build_text(element, style, ctx, window, cx)
        }
        "virtual-list" => {
            ctx.custom_registry.destroy(id);
            build_virtual_list(element, ctx, window, cx)
        }

        // Polymorphic dispatch for all custom elements.
        custom_type => {
            let child_ids: Vec<u64> = element
                .children
                .iter()
                .copied()
                .filter(|child_id| ctx.tree.elements.contains_key(child_id))
                .collect();
            let custom_children: Vec<gpui::AnyElement> = child_ids
                .into_iter()
                .map(|child_id| build_element(child_id, ctx, window, cx))
                .collect();

            let selection = ctx.selection.clone();
            let inherited = ctx.inherited;
            let custom_registry = &mut *ctx.custom_registry;
            if let Some(instance) = custom_registry.get_or_create(id, custom_type) {
                // Sync known props from RetainedElement to the CustomElement instance.
                // Missing keys are explicitly reset with null to avoid stale state.
                let supported_props: Vec<String> = instance
                    .supported_props()
                    .iter()
                    .map(|key| (*key).to_string())
                    .collect();

                for key in &supported_props {
                    let value = element
                        .custom_props
                        .get(key)
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    instance.set_prop(key, value);
                }

                // Also pass through unknown props for forward compatibility.
                for (key, value) in &element.custom_props {
                    if !supported_props.iter().any(|known| known == key) {
                        instance.set_prop(key, value.clone());
                    }
                }

                // Only pass events the custom element declares support for.
                let supported_events: Vec<String> = instance
                    .supported_events()
                    .iter()
                    .map(|event| (*event).to_string())
                    .collect();
                let filtered_events: HashSet<String> = element
                    .events
                    .iter()
                    .filter(|event| supported_events.iter().any(|supported| supported == *event))
                    .cloned()
                    .collect();

                let render_ctx = CustomRenderContext {
                    id,
                    events: &filtered_events,
                    event_callback: ctx.event_callback,
                    focus_handle: ctx.focus_handles.get(&id),
                    style,
                    children: custom_children,
                    selection,
                    selectable: inherited.selectable,
                    selection_wash: inherited.selection_wash,
                };

                instance.render(render_ctx, window, cx)
            } else {
                log::warn!("Unknown element type: {}", custom_type);
                gpui::Empty.into_any_element()
            }
        }
    };

    ctx.inherited = parent_inherited;
    built
}

fn build_virtual_list(
    element: &crate::retained_tree::RetainedElement,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    let child_ids: Vec<u64> = element
        .children
        .iter()
        .copied()
        .filter(|child_id| ctx.tree.elements.contains_key(child_id))
        .collect();
    let child_revisions: Vec<u64> = child_ids
        .iter()
        .filter_map(|child_id| {
            ctx.tree
                .elements
                .get(child_id)
                .map(|child| child.subtree_revision)
        })
        .collect();
    let focusable_rows: HashSet<u64> = ctx
        .focus_handles
        .keys()
        .filter_map(|element_id| virtual_row_ancestor(ctx.tree, element.id, *element_id))
        .collect();
    let focused_row = ctx
        .focus_handles
        .iter()
        .find_map(|(element_id, handle)| {
            handle
                .is_focused(window)
                .then(|| virtual_row_ancestor(ctx.tree, element.id, *element_id))
                .flatten()
        })
        .or_else(|| {
            ctx.focus_handles.keys().find_map(|element_id| {
                ctx.tree
                    .elements
                    .get(element_id)
                    .is_some_and(|element| element.auto_focus)
                    .then(|| virtual_row_ancestor(ctx.tree, element.id, *element_id))
                    .flatten()
            })
        });
    let config = VirtualListConfig::from_element(element);
    let list_state = match ctx.virtual_lists.entry(element.id) {
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            entry.get_mut().sync(
                config,
                child_ids.clone(),
                child_revisions,
                &focusable_rows,
                cx,
            );
            let entry = entry.into_mut();
            if let Some(row_id) = focused_row.filter(|row_id| !entry.seen_rows.contains(row_id)) {
                if let Some(index) = entry.child_ids.iter().position(|id| *id == row_id) {
                    entry.state.scroll_to(gpui::ListOffset {
                        item_ix: index,
                        offset_in_item: gpui::px(0.0),
                    });
                }
            }
            entry.state.clone()
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            let row_focus_handles = child_ids
                .iter()
                .map(|id| focusable_rows.contains(id).then(|| cx.focus_handle()))
                .collect();
            let entry = entry.insert(VirtualListEntry::new(
                config,
                child_ids.clone(),
                child_revisions,
                row_focus_handles,
            ));
            if let Some(row_id) = focused_row {
                if let Some(index) = entry.child_ids.iter().position(|id| *id == row_id) {
                    entry.state.scroll_to(gpui::ListOffset {
                        item_ix: index,
                        offset_in_item: gpui::px(0.0),
                    });
                }
            }
            entry.state.clone()
        }
    };

    let list_id = element.id;
    let child_ids = Rc::new(child_ids);
    let inherited = ctx.inherited;
    let render_item = cx.processor(move |view, index: usize, window, cx| {
        let Some(&child_id) = child_ids.get(index) else {
            return gpui::Empty.into_any_element();
        };
        view.build_virtual_child(list_id, index, child_id, inherited, window, cx)
    });
    let mut list = gpui::list(list_state.clone(), render_item)
        .with_sizing_behavior(gpui::ListSizingBehavior::Auto);
    if let Some(style) = element.style.as_ref() {
        list = apply_styles(list, style);
    }
    match config.estimated_item_height {
        Some(height) => EstimatedHeightList {
            list,
            state: list_state,
            height: gpui::px(height),
        }
        .into_any_element(),
        None => list.into_any_element(),
    }
}

/// GPUI's list drops every row's size hint when its width changes, including
/// its first layout, so unmeasured rows count as zero height and pixel scrolls
/// and the scrollbar stop short. Re-apply the estimate after that reset.
struct EstimatedHeightList {
    list: gpui::List,
    state: gpui::ListState,
    height: gpui::Pixels,
}

impl gpui::Element for EstimatedHeightList {
    type RequestLayoutState = ();
    type PrepaintState = <gpui::List as gpui::Element>::PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        self.list.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        let previous_width = self.state.viewport_bounds().size.width;
        let prepaint = self
            .list
            .prepaint(id, inspector_id, bounds, request_layout, window, cx);
        if bounds.size.width != previous_width {
            // Fills only rows without a size, so measured heights are kept.
            let _ = self.state.clone().with_uniform_item_height(self.height);
        }
        prepaint
    }

    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut (),
        prepaint: &mut Self::PrepaintState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.list
            .paint(id, inspector_id, bounds, request_layout, prepaint, window, cx);
    }
}

impl gpui::IntoElement for EstimatedHeightList {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

fn virtual_row_ancestor(tree: &RetainedTree, list_id: u64, element_id: u64) -> Option<u64> {
    let mut current = element_id;
    loop {
        let parent = tree.elements.get(&current)?.parent?;
        if parent == list_id {
            return Some(current);
        }
        current = parent;
    }
}

pub(crate) fn build_div(
    element: &crate::retained_tree::RetainedElement,
    style: Option<&StyleDesc>,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    let element_id_str = format!("__solo_{}", element.id);
    let mut el = gpui::div().id(gpui::SharedString::from(element_id_str));

    // The retained-tree root div is created unstyled by the JS side. Size it
    // to the window so percentage heights below it resolve against a definite
    // parent; without this, an app whose root uses height: "100%" collapses
    // to content height and its scroll containers never overflow.
    if ctx.root_id == Some(element.id) {
        el = el.size_full();
    }

    if let Some(style) = style {
        el = apply_styles(el, style);

        // ── Pseudo-selector styles (hover / active) ──────────────────
        // GPUI's .hover() and .active() take a closure that receives a
        // StyleRefinement and returns it with modifications. Since
        // StyleRefinement implements Styled, we can reuse apply_styles().
        if let Some(ref hover_style) = style.hover {
            el = el.hover(|refinement| apply_styles(refinement, hover_style));
        }
        if let Some(ref active_style) = style.active {
            el = el.active(|refinement| apply_styles(refinement, active_style));
        }

        if crate::style::should_occlude(style) {
            // BlockMouse (occlude) stops the hit test. The parent scroller
            // then never sees the wheel. In-flow fills must use
            // BlockMouseExceptScroll. Keep occlude for overlays that steal
            // the pointer: absolute, fixed, or pointerEvents: "auto".
            let steal_scroll = matches!(style.position.as_deref(), Some("absolute") | Some("fixed"))
                || style.pointer_events.as_deref() == Some("auto");
            el = if steal_scroll {
                el.occlude()
            } else {
                el.block_mouse_except_scroll()
            };
        }
    }

    // ── Overflow: scroll ─────────────────────────────────────────────
    // overflow_scroll() requires StatefulInteractiveElement (only on Stateful<Div>),
    // so we handle it here rather than in apply_styles (which takes E: Styled).
    //
    // CSS precedence: axis-specific props (overflowX/Y) override the shorthand
    // (overflow). E.g. { overflow: "scroll", overflowY: "hidden" } → scroll X only.
    //
    // overflow-x only works as a flex viewport. Default display is Block, so a
    // wide child fills the parent instead of overflowing. Zed's code-block path:
    // flex + min_w_0 on the scroller, flex_none on the child.
    let mut overflow_x_only = false;
    if let Some(style) = style {
        // Resolve each axis: axis-specific overrides shorthand.
        let resolved_x = style.overflow_x.as_deref().or(style.overflow.as_deref());
        let resolved_y = style.overflow_y.as_deref().or(style.overflow.as_deref());

        let needs_scroll_x = resolved_x == Some("scroll");
        let needs_scroll_y = resolved_y == Some("scroll");
        let contains_scroll_x = needs_scroll_x
            && style
                .overscroll_behavior_x
                .as_deref()
                .or(style.overscroll_behavior.as_deref())
                == Some("contain");
        let contains_scroll_y = needs_scroll_y
            && style
                .overscroll_behavior_y
                .as_deref()
                .or(style.overscroll_behavior.as_deref())
                == Some("contain");

        // Scroll containers must be allowed to shrink BELOW their content
        // size. Taffy's automatic minimum size (min-height/min-width: auto)
        // otherwise pins a flex item to its content extent, so the viewport
        // grows with the children, nothing overflows, and every wheel delta
        // clamps straight back to zero (observed on Linux: bounds ==
        // content_size, scroll_max = 0).
        if needs_scroll_x && needs_scroll_y {
            el = el
                .overflow_scroll()
                .min_w(gpui::px(0.))
                .min_h(gpui::px(0.));
        } else if needs_scroll_x {
            overflow_x_only = true;
            el = el
                .flex()
                .min_w_0()
                .overflow_x_scroll()
                .restrict_scroll_to_axis();
        } else if needs_scroll_y {
            el = el
                .overflow_y_scroll()
                .min_h(gpui::px(0.));
        }

        // Attach a persistent ScrollHandle when scrolling is enabled.
        // The handle persists across renders (stored in SoloView::scroll_handles)
        // so GPUI maintains the scroll offset between frames.
        if needs_scroll_x || needs_scroll_y {
            let handle = ctx
                .scroll_handles
                .entry(element.id)
                .or_insert_with(gpui::ScrollHandle::new);
            el = el.track_scroll(handle);

            if contains_scroll_x || contains_scroll_y {
                // GPUI registers this element's built-in scroll handler after
                // custom wheel handlers, then dispatches bubble handlers in
                // reverse order. The element therefore consumes its delta
                // first; stopping here prevents an ancestor from consuming the
                // same gesture even when this scroller is already at a boundary.
                el = el.on_scroll(move |event, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let contains_x = contains_scroll_x && delta.x != gpui::px(0.0);
                    let contains_y = contains_scroll_y && delta.y != gpui::px(0.0);
                    if contains_x || contains_y {
                        cx.stop_propagation();
                    }
                });
            }
        } else {
            // Element is no longer scrollable — remove stale handle.
            ctx.scroll_handles.remove(&element.id);
        }
    } else {
        // No style at all — remove stale handle if it existed.
        ctx.scroll_handles.remove(&element.id);
    }

    // If a FocusHandle was pre-created for this element (by sync_focus_handles),
    // attach it via track_focus. This makes the element focusable — clicking it
    // or tabbing to it gives it keyboard focus. The handle persists across renders
    // because it's stored in SoloView::focus_handles.
    if style.and_then(|style| style.position.as_deref()).is_none() {
        el = el.relative();
    }
    el = el.child(crate::automation::bounds_tracker(element.id));

    if let Some(handle) = ctx.focus_handles.get(&element.id) {
        el = el.track_focus(handle);
    }
    if let Some(tab_index) = element
        .custom_props
        .get("tabIndex")
        .and_then(|value| value.as_i64())
        .and_then(|index| isize::try_from(index).ok())
    {
        el = el.tab_index(tab_index).tab_stop(tab_index >= 0);
    }

    // Wire up events.
    // Some events (on_hover, on_click) require a stateful element (.id()),
    // which we already set above. Others (on_mouse_down, on_key_down) work
    // on any InteractiveElement.
    for event_type in &element.events {
        let id = element.id;
        let callback = ctx.event_callback.clone();
        match event_type.as_str() {
            // ── Click ────────────────────────────────────────────
            "click" => {
                el = el.on_click(move |click_event, _window, _cx| {
                    emit_event_full(&callback, id, "click", |p| {
                        let (x, y) = point_to_xy(click_event.position());
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(click_event.modifiers().into());
                        p.click_count = Some(click_event.click_count() as u32);
                        p.is_right_click = Some(click_event.is_right_click());
                    });
                });
            }

            // ── Mouse down (all buttons) ─────────────────────────
            "mouseDown" => {
                // Wire all three buttons so JS gets right-click, middle-click, etc.
                for &button in &[
                    gpui::MouseButton::Left,
                    gpui::MouseButton::Middle,
                    gpui::MouseButton::Right,
                ] {
                    let callback = callback.clone();
                    el = el.on_mouse_down(button, move |mouse_event, _window, _cx| {
                        emit_event_full(&callback, id, "mouseDown", |p| {
                            let (x, y) = point_to_xy(mouse_event.position);
                            p.x = Some(x);
                            p.y = Some(y);
                            p.button = Some(mouse_button_to_u32(mouse_event.button));
                            p.click_count = Some(mouse_event.click_count as u32);
                            p.modifiers = Some(mouse_event.modifiers.into());
                        });
                    });
                }
            }

            // ── Mouse up (all buttons) ───────────────────────────
            "mouseUp" => {
                for &button in &[
                    gpui::MouseButton::Left,
                    gpui::MouseButton::Middle,
                    gpui::MouseButton::Right,
                ] {
                    let callback = callback.clone();
                    el = el.on_mouse_up(button, move |mouse_event, _window, _cx| {
                        emit_event_full(&callback, id, "mouseUp", |p| {
                            let (x, y) = point_to_xy(mouse_event.position);
                            p.x = Some(x);
                            p.y = Some(y);
                            p.button = Some(mouse_button_to_u32(mouse_event.button));
                            p.click_count = Some(mouse_event.click_count as u32);
                            p.modifiers = Some(mouse_event.modifiers.into());
                        });
                    });
                }
            }

            // ── Mouse move ───────────────────────────────────────
            "mouseMove" => {
                el = el.on_mouse_move(move |mouse_event, _window, _cx| {
                    emit_event_full(&callback, id, "mouseMove", |p| {
                        let (x, y) = point_to_xy(mouse_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(mouse_event.modifiers.into());
                        p.pressed_button = mouse_event.pressed_button.map(mouse_button_to_u32);
                    });
                });
            }

            // ── Hover (mouseEnter + mouseLeave) ──────────────────
            // GPUI's on_hover fires with true on enter, false on leave.
            // We split into two distinct event types for the Solid side.
            "mouseEnter" | "mouseLeave" => {
                // Only wire once even if both mouseEnter and mouseLeave are registered.
                // Check if we already wired on_hover via the other event.
                let has_enter = element.events.contains("mouseEnter");
                let has_leave = element.events.contains("mouseLeave");
                // Wire on first encounter (mouseEnter sorts before mouseLeave).
                if event_type.as_str() == "mouseEnter" || !has_enter {
                    let callback_enter = if has_enter {
                        ctx.event_callback.clone()
                    } else {
                        None
                    };
                    let callback_leave = if has_leave {
                        ctx.event_callback.clone()
                    } else {
                        None
                    };
                    el = el.on_hover(move |&is_hovered, _window, _cx| {
                        if is_hovered {
                            emit_event_full(&callback_enter, id, "mouseEnter", |p| {
                                p.hovered = Some(true);
                            });
                        } else {
                            emit_event_full(&callback_leave, id, "mouseLeave", |p| {
                                p.hovered = Some(false);
                            });
                        }
                    });
                }
            }

            // ── Mouse down outside ───────────────────────────────
            // Fires when the user clicks OUTSIDE this element.
            // Critical for "click outside to close" pattern (dropdowns, modals).
            "mouseDownOutside" => {
                el = el.on_mouse_down_out(move |mouse_event, _window, _cx| {
                    emit_event_full(&callback, id, "mouseDownOutside", |p| {
                        let (x, y) = point_to_xy(mouse_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.button = Some(mouse_button_to_u32(mouse_event.button));
                        p.modifiers = Some(mouse_event.modifiers.into());
                    });
                });
            }

            // ── Scroll wheel ─────────────────────────────────────
            "scroll" => {
                el = el.on_scroll(move |scroll_event, _window, _cx| {
                    emit_event_full(&callback, id, "scroll", |p| {
                        let (x, y) = point_to_xy(scroll_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(scroll_event.modifiers.into());
                        p.precise = Some(scroll_event.delta.precise());

                        // Convert ScrollDelta to pixel values.
                        // For Lines delta, we use a default line height of 20px.
                        let line_height = gpui::px(20.0);
                        let pixel_delta = scroll_event.delta.pixel_delta(line_height);
                        p.delta_x = Some(f64::from(f32::from(pixel_delta.x)));
                        p.delta_y = Some(f64::from(f32::from(pixel_delta.y)));

                        p.touch_phase = Some(match scroll_event.touch_phase {
                            gpui::TouchPhase::Started => "started".to_string(),
                            gpui::TouchPhase::Moved => "moved".to_string(),
                            gpui::TouchPhase::Ended => "ended".to_string(),
                            gpui::TouchPhase::Cancelled => "cancelled".to_string(),
                        });
                    });
                });
            }

            // ── Key down ─────────────────────────────────────────
            // Requires .focusable() (set above). Element must be focused
            // (clicked or tabbed to) for these to fire.
            "keyDown" => {
                el = el.on_key_down(move |key_event, _window, _cx| {
                    emit_event_full(&callback, id, "keyDown", |p| {
                        p.key = Some(key_event.keystroke.key.clone());
                        p.key_char = key_event.keystroke.key_char.clone();
                        p.is_held = Some(key_event.is_held);
                        p.modifiers = Some(key_event.keystroke.modifiers.into());
                    });
                });
            }

            // ── Key up ───────────────────────────────────────────
            "keyUp" => {
                el = el.on_key_up(move |key_event, _window, _cx| {
                    emit_event_full(&callback, id, "keyUp", |p| {
                        p.key = Some(key_event.keystroke.key.clone());
                        p.key_char = key_event.keystroke.key_char.clone();
                        p.modifiers = Some(key_event.keystroke.modifiers.into());
                    });
                });
            }

            // ── Focus / Blur ─────────────────────────────────────
            // Event emission is handled by FocusHandle subscriptions
            // set up in SoloView::sync_focus_handles(). The handle is
            // attached to this element via .track_focus() above.
            "focus" | "blur" => {}

            _ => {}
        }
    }

    // Text content — selectable, same as a <text> leaf.
    if let Some(ref content) = element.content {
        el = el.child(text_content(element.id, content, ctx));
    }

    // Children
    let child_ids: Vec<u64> = element.children.clone();
    for child_id in child_ids {
        let child = build_element(child_id, ctx, window, cx);
        el = if overflow_x_only {
            el.child(gpui::div().flex_none().child(child))
        } else {
            el.child(child)
        };
    }

    el.into_any_element()
}

/// A selectable text run owned by `element_id`. Runs are left to gpui so the
/// text keeps inheriting colour, weight and family from ancestor styles.
fn text_content(element_id: u64, content: &str, ctx: &BuildCtx) -> gpui::AnyElement {
    if !ctx.inherited.selectable {
        // Still logged: `getPaintedText()` promises every painted string, and a
        // `userSelect: "none"` label is exactly the chrome tests want to assert.
        return crate::text::chrome_text(gpui::SharedString::from(content.to_string()), None);
    }
    selectable_text(crate::text::SelectableText::new(
        gpui::SharedString::from(content.to_string()),
        None,
        selection_key(element_id, 0),
        ctx.selection.clone(),
        ctx.inherited.selection_wash,
    ))
}

pub(crate) fn build_text(
    element: &crate::retained_tree::RetainedElement,
    style: Option<&StyleDesc>,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    // Fast path: plain text leaf without style. It still goes through
    // `text_content` so the glyphs land in the selection registry — the old
    // raw-string return was the reason text was not selectable.
    if style.is_none() && element.children.is_empty() {
        let content = element.content.clone().unwrap_or_default();
        return gpui::div()
            .relative()
            .child(crate::automation::bounds_tracker(element.id))
            .child(text_content(element.id, &content, ctx))
            .into_any_element();
    }

    // The full style set, exactly as `<div>` gets it. `<text>` used to apply a
    // text-only subset, so `padding`, `width` and every layout prop on a text
    // node were silently dropped — a hole with no error and no warning.
    let mut el = gpui::div();
    if let Some(style) = style {
        el = apply_styles(el, style);
    }
    if style.and_then(|style| style.position.as_deref()).is_none() {
        el = el.relative();
    }
    el = el.child(crate::automation::bounds_tracker(element.id));

    if let Some(ref content) = element.content {
        el = el.child(text_content(element.id, content, ctx));
    }

    let child_ids: Vec<u64> = element.children.clone();
    for child_id in child_ids {
        el = el.child(build_element(child_id, ctx, window, cx));
    }

    el.into_any_element()
}

// ── Style application ────────────────────────────────────────────────

pub(crate) fn apply_width<E: gpui::Styled>(el: E, dim: &crate::style::DimensionValue) -> E {
    match dim {
        crate::style::DimensionValue::Pixels(v) => el.w(gpui::px(*v as f32)),
        crate::style::DimensionValue::Percentage(v) if *v >= 0.999 => el.w_full(),
        crate::style::DimensionValue::Percentage(v) => el.w(gpui::relative(*v as f32)),
        crate::style::DimensionValue::Auto => el,
    }
}

pub(crate) fn apply_height<E: gpui::Styled>(el: E, dim: &crate::style::DimensionValue) -> E {
    match dim {
        crate::style::DimensionValue::Pixels(v) => el.h(gpui::px(*v as f32)),
        crate::style::DimensionValue::Percentage(v) if *v >= 0.999 => el.h_full(),
        crate::style::DimensionValue::Percentage(v) => el.h(gpui::relative(*v as f32)),
        crate::style::DimensionValue::Auto => el,
    }
}

/// The GPUI cursor for a CSS `cursor` value. Unknown values change nothing.
fn css_cursor_style(name: &str) -> Option<gpui::CursorStyle> {
    use gpui::CursorStyle::*;
    Some(match name {
        "default" => Arrow,
        "pointer" => PointingHand,
        "text" => IBeam,
        "vertical-text" => IBeamCursorForVerticalLayout,
        "crosshair" => Crosshair,
        "grab" => OpenHand,
        "grabbing" => ClosedHand,
        "w-resize" => ResizeLeft,
        "e-resize" => ResizeRight,
        "ew-resize" => ResizeLeftRight,
        "n-resize" => ResizeUp,
        "s-resize" => ResizeDown,
        "ns-resize" => ResizeUpDown,
        // GPUI's doc comments swap the diagonals; these follow the native
        // cursor it draws (UpLeftDownRight is AppKit's north-west/south-east).
        "nwse-resize" => ResizeUpLeftDownRight,
        "nesw-resize" => ResizeUpRightDownLeft,
        "col-resize" => ResizeColumn,
        "row-resize" => ResizeRow,
        "not-allowed" => OperationNotAllowed,
        "alias" => DragLink,
        "copy" => DragCopy,
        "context-menu" => ContextualMenu,
        _ => return None,
    })
}

pub(crate) fn apply_styles<E: gpui::Styled>(mut el: E, style: &StyleDesc) -> E {
    match style.display.as_deref() {
        Some("flex") => el = el.flex(),
        Some("grid") => el = el.grid(),
        _ => {}
    }
    if let Some(cols) = style.grid_template_columns {
        let count = cols.round().clamp(1.0, 64.0) as u16;
        el = match style.grid_column_min.as_deref() {
            Some("min-content") => el.grid_cols_min_content(count),
            Some("max-content") => el.grid_cols_max_content(count),
            _ => el.grid_cols(count),
        };
    }
    if let Some(rows) = style.grid_template_rows {
        let count = rows.round().clamp(1.0, 64.0) as u16;
        el = match style.grid_row_min.as_deref() {
            Some("min-content") => el.grid_rows_min_content(count),
            Some("max-content") => el.grid_rows_max_content(count),
            _ => el.grid_rows(count),
        };
    }
    if style.flex_direction.as_deref() == Some("column") {
        el = el.flex_col();
    }
    if style.flex_direction.as_deref() == Some("row") {
        el = el.flex_row();
    }
    match style.flex_wrap.as_deref() {
        Some("wrap") => el = el.flex_wrap(),
        Some("wrap-reverse") => el = el.flex_wrap_reverse(),
        Some("nowrap") => el = el.flex_nowrap(),
        _ => {}
    }
    if let Some(grow) = style.flex_grow {
        el.style().flex_grow = Some(grow as f32);
    }
    if let Some(shrink) = style.flex_shrink {
        el.style().flex_shrink = Some(shrink as f32);
    }
    match style.align_items.as_deref() {
        Some("center") => el = el.items_center(),
        Some("start") | Some("flex-start") => el = el.items_start(),
        Some("end") | Some("flex-end") => el = el.items_end(),
        _ => {}
    }
    match style.justify_content.as_deref() {
        Some("center") => el = el.justify_center(),
        Some("start") | Some("flex-start") => el = el.justify_start(),
        Some("end") | Some("flex-end") => el = el.justify_end(),
        Some("between") | Some("space-between") => el = el.justify_between(),
        Some("around") | Some("space-around") => el = el.justify_around(),
        _ => {}
    }
    match style.align_self.as_deref() {
        Some("center") => {
            el.style().align_self = Some(gpui::AlignItems::Center);
        }
        Some("start") | Some("flex-start") => {
            el.style().align_self = Some(gpui::AlignItems::FlexStart);
        }
        Some("end") | Some("flex-end") => {
            el.style().align_self = Some(gpui::AlignItems::FlexEnd);
        }
        Some("stretch") => {
            el.style().align_self = Some(gpui::AlignItems::Stretch);
        }
        Some("baseline") => {
            el.style().align_self = Some(gpui::AlignItems::Baseline);
        }
        _ => {}
    }
    if let Some(gap) = style.gap {
        el = el.gap(gpui::px(gap as f32));
    }
    // Per-axis gaps were in the style type and implemented nowhere. They come
    // after `gap` so the axis value wins, matching CSS shorthand order.
    if let Some(gap) = style.row_gap {
        el = el.gap_y(gpui::px(gap as f32));
    }
    if let Some(gap) = style.column_gap {
        el = el.gap_x(gpui::px(gap as f32));
    }
    if let Some(ref w) = style.width {
        el = apply_width(el, w);
    }
    if let Some(ref h) = style.height {
        el = apply_height(el, h);
    }
    if let Some(ref min_w) = style.min_width {
        match min_w {
            crate::style::DimensionValue::Pixels(v) => el = el.min_w(gpui::px(*v as f32)),
            crate::style::DimensionValue::Percentage(v) => el = el.min_w(gpui::relative(*v as f32)),
            crate::style::DimensionValue::Auto => {}
        }
    }
    if let Some(ref min_h) = style.min_height {
        match min_h {
            crate::style::DimensionValue::Pixels(v) => el = el.min_h(gpui::px(*v as f32)),
            crate::style::DimensionValue::Percentage(v) => el = el.min_h(gpui::relative(*v as f32)),
            crate::style::DimensionValue::Auto => {}
        }
    }
    if let Some(ref max_w) = style.max_width {
        match max_w {
            crate::style::DimensionValue::Pixels(v) => el = el.max_w(gpui::px(*v as f32)),
            crate::style::DimensionValue::Percentage(v) => el = el.max_w(gpui::relative(*v as f32)),
            crate::style::DimensionValue::Auto => {}
        }
    }
    if let Some(ref max_h) = style.max_height {
        match max_h {
            crate::style::DimensionValue::Pixels(v) => el = el.max_h(gpui::px(*v as f32)),
            crate::style::DimensionValue::Percentage(v) => el = el.max_h(gpui::relative(*v as f32)),
            crate::style::DimensionValue::Auto => {}
        }
    }
    if let Some(p) = style.padding {
        el = el.p(gpui::px(p as f32));
    }
    if let Some(pt) = style.padding_top {
        el = el.pt(gpui::px(pt as f32));
    }
    if let Some(pr) = style.padding_right {
        el = el.pr(gpui::px(pr as f32));
    }
    if let Some(pb) = style.padding_bottom {
        el = el.pb(gpui::px(pb as f32));
    }
    if let Some(pl) = style.padding_left {
        el = el.pl(gpui::px(pl as f32));
    }
    if let Some(m) = style.margin {
        el = el.m(gpui::px(m as f32));
    }
    if let Some(mt) = style.margin_top {
        el = el.mt(gpui::px(mt as f32));
    }
    if let Some(mr) = style.margin_right {
        el = el.mr(gpui::px(mr as f32));
    }
    if let Some(mb) = style.margin_bottom {
        el = el.mb(gpui::px(mb as f32));
    }
    if let Some(ml) = style.margin_left {
        el = el.ml(gpui::px(ml as f32));
    }
    match style.position.as_deref() {
        Some("absolute") => el = el.absolute(),
        Some("relative") => el = el.relative(),
        _ => {}
    }
    if let Some(top) = style.top {
        el = el.top(gpui::px(top as f32));
    }
    if let Some(right) = style.right {
        el = el.right(gpui::px(right as f32));
    }
    if let Some(bottom) = style.bottom {
        el = el.bottom(gpui::px(bottom as f32));
    }
    if let Some(left) = style.left {
        el = el.left(gpui::px(left as f32));
    }
    if let Some(ref bg) = style
        .background_color
        .as_ref()
        .or(style.background.as_ref())
    {
        if let Some(hex) = parse_color_hex(bg) {
            el = el.bg(gpui::rgba(hex));
        }
    }
    if let Some(ref color) = style.color {
        if let Some(hex) = parse_color_hex(color) {
            el = el.text_color(gpui::rgba(hex));
        }
    }
    if let Some(size) = style.font_size {
        el = el.text_size(gpui::px(size as f32));
    }
    if let Some(ref family) = style.font_family {
        el = el.font_family(family.clone());
    }
    if let Some(ref weight) = style.font_weight {
        el = el.font_weight(parse_font_weight(weight));
    }
    // `textAlign` was in the style type but implemented nowhere.
    match style.text_align.as_deref() {
        Some("center") => el = el.text_center(),
        Some("right") => el = el.text_right(),
        Some("left") | Some("start") => el = el.text_left(),
        _ => {}
    }
    match style.white_space.as_deref() {
        Some("nowrap") => el = el.whitespace_nowrap(),
        Some("normal") => el = el.whitespace_normal(),
        _ => {}
    }
    match style.text_overflow.as_deref() {
        Some("ellipsis") => el = el.text_ellipsis(),
        Some("ellipsis-start") => el = el.text_ellipsis_start(),
        _ => {}
    }
    if let Some(clamp) = style.line_clamp {
        if clamp >= 1.0 {
            el = el.line_clamp(clamp as usize);
        }
    }
    // `line_height` was accepted by the style type but never applied, so
    // multi-line text always used gpui's default leading.
    if let Some(line_height) = style.line_height {
        if line_height > 0.0 {
            el = el.line_height(gpui::px(line_height as f32));
        }
    }
    if let Some(radius) = style.border_radius {
        el = el.rounded(gpui::px(radius as f32));
    }
    // `borderWidth: 0` must clear a border, not be ignored: an element that
    // draws its own border needs a way for the caller to remove it.
    if let Some(width) = style.border_width {
        el = el.border(gpui::px(width.max(0.0) as f32));
    }
    if let Some(ref color) = style.border_color {
        if let Some(hex) = parse_color_hex(color) {
            el = el.border_color(gpui::rgba(hex));
        }
    }
    if let Some(opacity) = style.opacity {
        el = el.opacity(opacity as f32);
    }
    if let Some(cursor) = style.cursor.as_deref().and_then(css_cursor_style) {
        el = el.cursor(cursor);
    }
    // Overflow: hidden is on the Styled trait, so we handle it here.
    // overflow: "scroll" requires StatefulInteractiveElement — handled in build_div().
    // CSS precedence: axis-specific (overflowX/Y) overrides the shorthand (overflow).
    {
        let resolved_x = style.overflow_x.as_deref().or(style.overflow.as_deref());
        let resolved_y = style.overflow_y.as_deref().or(style.overflow.as_deref());
        // Only apply hidden here — scroll is handled in build_div.
        if resolved_x == Some("hidden") && resolved_y == Some("hidden") {
            el = el.overflow_hidden();
        } else if resolved_x == Some("hidden") {
            el = el.overflow_x_hidden();
        } else if resolved_y == Some("hidden") {
            el = el.overflow_y_hidden();
        }
    }

    el
}

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

// ── Batch processing ─────────────────────────────────────────────

/// Parsed batch operation — typed enum for atomic validation.
/// All ops are parsed and validated BEFORE any tree mutation occurs.
/// This prevents partial application on malformed batches.
enum BatchOp {
    CreateElement {
        id: u64,
        element_type: String,
    },
    DestroyElement {
        id: u64,
    },
    AppendChild {
        parent_id: u64,
        child_id: u64,
    },
    RemoveChild {
        parent_id: u64,
        child_id: u64,
    },
    InsertBefore {
        parent_id: u64,
        child_id: u64,
        before_id: u64,
    },
    SetStyle {
        id: u64,
        style: StyleDesc,
    },
    SetText {
        id: u64,
        content: String,
    },
    SetEventListener {
        id: u64,
        event_type: String,
        has_handler: bool,
    },
    SetRoot {
        id: u64,
    },
    SetCustomProp {
        id: u64,
        key: String,
        value: serde_json::Value,
    },
}

/// Parse all batch ops from JSON into typed enums.
/// Returns Err on the first invalid op — no tree mutation has occurred yet.
fn parse_batch_ops(ops: &[serde_json::Value]) -> Result<Vec<BatchOp>> {
    let mut parsed = Vec::with_capacity(ops.len());

    for (i, op) in ops.iter().enumerate() {
        let arr = op
            .as_array()
            .ok_or_else(|| Error::from_reason(format!("Batch op {} is not an array", i)))?;
        let op_name = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::from_reason(format!("Batch op {} missing op name string", i)))?;

        let batch_op = match op_name {
            "createElement" => BatchOp::CreateElement {
                id: batch_id(arr, 1, i)?,
                element_type: batch_str(arr, 2, i)?,
            },
            "destroyElement" => BatchOp::DestroyElement {
                id: batch_id(arr, 1, i)?,
            },
            "appendChild" => BatchOp::AppendChild {
                parent_id: batch_id(arr, 1, i)?,
                child_id: batch_id(arr, 2, i)?,
            },
            "removeChild" => BatchOp::RemoveChild {
                parent_id: batch_id(arr, 1, i)?,
                child_id: batch_id(arr, 2, i)?,
            },
            "insertBefore" => BatchOp::InsertBefore {
                parent_id: batch_id(arr, 1, i)?,
                child_id: batch_id(arr, 2, i)?,
                before_id: batch_id(arr, 3, i)?,
            },
            "setStyle" => {
                let style: StyleDesc = batch_decode(arr, 2, i).map_err(|e| {
                    Error::from_reason(format!("Batch op {} setStyle parse error: {}", i, e))
                })?;
                BatchOp::SetStyle {
                    id: batch_id(arr, 1, i)?,
                    style,
                }
            }
            "setText" => BatchOp::SetText {
                id: batch_id(arr, 1, i)?,
                content: batch_str(arr, 2, i)?,
            },
            "setEventListener" => {
                let has_handler = arr
                    .get(3)
                    .and_then(|v| v.as_bool().or_else(|| v.as_u64().map(|n| n != 0)))
                    .ok_or_else(|| {
                        Error::from_reason(format!(
                            "Batch op {} setEventListener missing/invalid hasHandler at index 3",
                            i
                        ))
                    })?;
                BatchOp::SetEventListener {
                    id: batch_id(arr, 1, i)?,
                    event_type: batch_str(arr, 2, i)?,
                    has_handler,
                }
            }
            "setRoot" => BatchOp::SetRoot {
                id: batch_id(arr, 1, i)?,
            },
            "setCustomProp" => BatchOp::SetCustomProp {
                id: batch_id(arr, 1, i)?,
                key: batch_str(arr, 2, i)?,
                value: batch_payload(arr, 3, i)?,
            },
            "setCustomPropValue" => BatchOp::SetCustomProp {
                id: batch_id(arr, 1, i)?,
                key: batch_str(arr, 2, i)?,
                value: arr.get(3).cloned().ok_or_else(|| {
                    Error::from_reason(format!("Batch op {} missing custom prop value", i))
                })?,
            },
            _ => {
                return Err(Error::from_reason(format!(
                    "Batch op {} unknown operation: {:?}",
                    i, op_name
                )));
            }
        };
        parsed.push(batch_op);
    }

    Ok(parsed)
}

/// Apply a batch of mutation tuples to a RetainedTree.
/// Shared between SoloRenderer::apply_batch and TestSoloRenderer::apply_batch.
/// Returns accumulated destroyed IDs (as f64) from all destroyElement ops.
///
/// ATOMIC: all ops are parsed and validated first. If any op is malformed,
/// the tree is left unchanged and an error is returned. This prevents
/// partial application that could desync JS and Rust state.
///
/// Batch format: JSON array of tuples [opcode, ...args].
/// See SoloRenderer::apply_batch for opcode documentation.
pub(crate) fn apply_batch_to_tree(
    tree: &mut RetainedTree,
    ops: &[serde_json::Value],
) -> Result<Vec<f64>> {
    // Phase 1: parse and validate all ops (no mutation).
    let parsed = parse_batch_ops(ops)?;

    // Phase 2: apply all validated ops to the tree.
    let mut destroyed_ids: Vec<f64> = Vec::new();
    for batch_op in parsed {
        match batch_op {
            BatchOp::CreateElement { id, element_type } => {
                tree.create_element(id, element_type);
            }
            BatchOp::DestroyElement { id } => {
                let destroyed = tree.destroy_element(id);
                destroyed_ids.extend(destroyed.iter().map(|&id| id as f64));
            }
            BatchOp::AppendChild {
                parent_id,
                child_id,
            } => {
                tree.append_child(parent_id, child_id);
            }
            BatchOp::RemoveChild {
                parent_id,
                child_id,
            } => {
                tree.remove_child(parent_id, child_id);
            }
            BatchOp::InsertBefore {
                parent_id,
                child_id,
                before_id,
            } => {
                tree.insert_before(parent_id, child_id, before_id);
            }
            BatchOp::SetStyle { id, style } => {
                tree.set_style(id, style);
            }
            BatchOp::SetText { id, content } => {
                tree.set_text(id, content);
            }
            BatchOp::SetEventListener {
                id,
                event_type,
                has_handler,
            } => {
                tree.set_event_listener(id, event_type, has_handler);
            }
            BatchOp::SetRoot { id } => {
                tree.root_id = Some(id);
            }
            BatchOp::SetCustomProp { id, key, value } => {
                tree.set_custom_prop(id, key, value);
            }
        }
    }

    Ok(destroyed_ids)
}

/// Extract a u64 element ID from a batch tuple at the given index.
fn batch_id(arr: &[serde_json::Value], idx: usize, op_idx: usize) -> Result<u64> {
    let v = arr.get(idx).and_then(|v| v.as_f64()).ok_or_else(|| {
        Error::from_reason(format!("Batch op {} missing id at index {}", op_idx, idx))
    })?;
    to_element_id(v)
}

/// A style or custom-prop payload. Objects land as JSON. Legacy batches
/// still send a JSON string and get decoded here.
fn batch_payload(arr: &[serde_json::Value], idx: usize, op_idx: usize) -> Result<serde_json::Value> {
    let value = arr.get(idx).ok_or_else(|| {
        Error::from_reason(format!(
            "Batch op {} missing value at index {}",
            op_idx, idx
        ))
    })?;
    if let Some(encoded) = value.as_str() {
        match serde_json::from_str(encoded) {
            Ok(parsed) => Ok(parsed),
            Err(_) => Ok(serde_json::Value::String(encoded.to_string())),
        }
    } else {
        Ok(value.clone())
    }
}

fn batch_decode<T: serde::de::DeserializeOwned>(
    arr: &[serde_json::Value],
    idx: usize,
    op_idx: usize,
) -> Result<T> {
    let value = batch_payload(arr, idx, op_idx)?;
    serde_json::from_value(value).map_err(|e| Error::from_reason(e.to_string()))
}

/// Extract a String from a batch tuple at the given index.
fn batch_str(arr: &[serde_json::Value], idx: usize, op_idx: usize) -> Result<String> {
    arr.get(idx)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            Error::from_reason(format!(
                "Batch op {} missing string at index {}",
                op_idx, idx
            ))
        })
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
        let task = update_window_in(target, move |_view, window, _cx| {
            crate::context_menu::show(&json, &tree.lock().unwrap(), window)
        })??;
        Ok(AsyncTask::new(task))
    }

    #[napi]
    pub fn cancel_context_menu(&self, request_id: f64) -> Result<()> {
        crate::context_menu::cancel(to_element_id(request_id)?);
        Ok(())
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
