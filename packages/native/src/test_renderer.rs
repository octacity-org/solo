/// TestSoloRenderer — GPU-backed GPUI test renderer exposed to Node.js via napi.
///
/// Uses gpui::VisualTestAppContext on macOS (Metal) and
/// gpui::HeadlessAppContext on Linux (wgpu/Vulkan), both with TestDispatcher
/// for deterministic scheduling. Runs the SAME SoloView, build_element(),
/// apply_styles(), and event handlers as production.
///
/// macOS positions its windows offscreen at (-10000, -10000) — invisible but
/// fully rendered. Linux windows are headless: GPUI's `TestPlatform` owns
/// them and the wgpu headless renderer rasterizes their frames, so
/// capture_screenshot() works on both platforms.
///
/// TestCx is !Send, so it is stored in thread-local state. All napi calls
/// happen on the JS main thread.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;
#[cfg(target_os = "macos")]
use objc2::MainThreadMarker;
#[cfg(target_os = "macos")]
use objc2_app_kit::NSScreen;

use gpui::AppContext as _;
use gpui::{AnyWindowHandle, Entity};

/// A unified handle over the two GPUI test contexts. macOS keeps its Metal
/// rendering through `VisualTestAppContext`; Linux uses `HeadlessAppContext`
/// with a wgpu headless renderer, so frames are rasterized on both.
#[cfg(target_os = "macos")]
type TestCx = gpui::VisualTestAppContext;
#[cfg(not(target_os = "macos"))]
type TestCx = gpui::HeadlessAppContext;

/// The platform's context-menu task: an AppKit helper on macOS, the in-window
/// GPUI menu on Linux.
#[cfg(target_os = "macos")]
use crate::context_menu::ContextMenuTask as NativeContextMenuTask;
#[cfg(not(target_os = "macos"))]
use crate::context_menu_gpui::MenuTask as NativeContextMenuTask;

use crate::element_tree::EventPayload;
use crate::renderer::{
    apply_batch_to_tree, parse_batch, debug_frame_overlay_mode_name, emit_event_full,
    parse_debug_frame_overlay_mode, to_element_id, validate_window_options, window_size,
    EventCallback, SoloView, WindowOptions, WindowSize, WINDOW_CLOSED_EVENT,
};
use crate::retained_tree::RetainedTree;

/// Simulate an input event on `window`, as GPUI's `VisualTestAppContext`
/// does on macOS: dispatch through the window and settle the loop. The same
/// path also runs under the Linux headless context, whose windows receive
/// events exactly like a real GPUI window.
fn simulate_event<E: gpui::InputEvent>(
    cx: &mut TestCx,
    window: AnyWindowHandle,
    event: E,
) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(event.to_platform_input(), cx);
    })
    .ok();
    cx.run_until_parked();
}

fn simulate_mouse_down(
    cx: &mut TestCx,
    window: AnyWindowHandle,
    position: gpui::Point<gpui::Pixels>,
    button: gpui::MouseButton,
    modifiers: gpui::Modifiers,
) {
    simulate_event(
        cx,
        window,
        gpui::MouseDownEvent {
            position,
            modifiers,
            button,
            click_count: 1,
            first_mouse: false,
        },
    );
}

fn simulate_mouse_up(
    cx: &mut TestCx,
    window: AnyWindowHandle,
    position: gpui::Point<gpui::Pixels>,
    button: gpui::MouseButton,
    modifiers: gpui::Modifiers,
) {
    simulate_event(
        cx,
        window,
        gpui::MouseUpEvent {
            position,
            modifiers,
            button,
            click_count: 1,
        },
    );
}

fn simulate_mouse_move(
    cx: &mut TestCx,
    window: AnyWindowHandle,
    position: gpui::Point<gpui::Pixels>,
    button: Option<gpui::MouseButton>,
    modifiers: gpui::Modifiers,
) {
    simulate_event(
        cx,
        window,
        gpui::MouseMoveEvent {
            position,
            modifiers,
            pressed_button: button,
        },
    );
}

/// Park the pointer outside the window.
///
/// GPUI's `TestPlatform` starts every window's pointer at (0, 0), and a
/// headless window has no cursor to move it. Left there, whatever sits in the
/// top-left corner is hovered from the first frame, before a test simulates
/// any input. macOS opens its test windows offscreen at (-10000, -10000), so
/// the pointer never lands inside one.
#[cfg(not(target_os = "macos"))]
fn park_pointer_outside_window(cx: &mut TestCx, window: AnyWindowHandle) {
    simulate_mouse_move(
        cx,
        window,
        gpui::point(gpui::px(-10_000.0), gpui::px(-10_000.0)),
        None,
        gpui::Modifiers::default(),
    );
}

#[cfg(target_os = "macos")]
fn park_pointer_outside_window(_cx: &mut TestCx, _window: AnyWindowHandle) {}

fn simulate_keystrokes(cx: &mut TestCx, window: AnyWindowHandle, keystrokes: &str) {
    for keystroke_text in keystrokes.split_whitespace() {
        let keystroke = gpui::Keystroke::parse(keystroke_text)
            .unwrap_or_else(|_| panic!("Invalid keystroke: {}", keystroke_text));
        cx.update_window(window, |_, window, cx| {
            window.dispatch_keystroke(keystroke, cx);
        })
        .ok();
    }
    cx.run_until_parked();
}

/// The test app context, fresh for each renderer.
#[cfg(target_os = "macos")]
fn new_test_context() -> TestCx {
    gpui::VisualTestAppContext::new(test_platform())
}

/// The headless app context on Linux: real text shaping and wgpu rendering
/// (Vulkan; lavapipe on CI), with deterministic time. GPUI's
/// `WgpuHeadlessRenderer` lets `capture_screenshot` read the same frame the
/// app would paint, so the visual tests run here too.
#[cfg(not(target_os = "macos"))]
fn new_test_context() -> TestCx {
    let text_system: Arc<dyn gpui::PlatformTextSystem> =
        Arc::new(gpui_wgpu::CosmicTextSystem::new("fallback"));
    gpui::HeadlessAppContext::with_platform(
        text_system,
        Arc::new(()),
        gpui_platform::current_headless_renderer,
    )
}

/// Open a window at the default test size (1280x800, offscreen on macOS).
fn open_window_default<V: gpui::Render + 'static>(
    cx: &mut TestCx,
    build_root: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> Entity<V>,
) -> anyhow::Result<gpui::WindowHandle<V>> {
    #[cfg(target_os = "macos")]
    return cx.open_offscreen_window_default(build_root);
    #[cfg(not(target_os = "macos"))]
    return cx.open_window(gpui::size(gpui::px(1280.0), gpui::px(800.0)), build_root);
}

/// Open a window at an explicit size, offscreen on macOS, at the origin on
/// headless Linux (position is invisible there, so it is harmless).
fn open_window_at<V: gpui::Render + 'static>(
    cx: &mut TestCx,
    size: gpui::Size<gpui::Pixels>,
    build_root: impl FnOnce(&mut gpui::Window, &mut gpui::App) -> Entity<V>,
) -> anyhow::Result<gpui::WindowHandle<V>> {
    #[cfg(target_os = "macos")]
    return cx.open_offscreen_window(size, build_root);
    #[cfg(not(target_os = "macos"))]
    return cx.open_window(size, build_root);
}

// ── Thread-local storage for !Send GPUI types ────────────────────────

/// Bundles the test context + window handle + view entity.
/// Stored in thread_local because the context is !Send (Rc<AppCell>).
/// Field order is load-bearing: Rust drops fields in declaration order, and
/// gpui panics at app teardown if an `Entity` handle outlives its `App`.
/// `view` and `secondary` must therefore be declared before `cx`.
struct VisualTestState {
    view: Entity<SoloView>,
    window: AnyWindowHandle,
    /// Windows opened with `openWindow`, in opening order.
    secondary: Vec<TestWindow>,
    /// The alert the prompt builder is showing, for `pendingAlert`.
    alert: Rc<RefCell<Option<PendingAlert>>>,
    /// Linux keeps the tabbing identifier in the UI-thread window registry,
    /// which the test renderer does not use; store it here instead so the JS
    /// API round-trips.
    main_tabbing_identifier: String,
    cx: TestCx,
}

/// An alert shown through `TestAlert` instead of a real `NSAlert` sheet.
struct PendingAlert {
    window: gpui::AnyWindowHandle,
    level: gpui::PromptLevel,
    message: String,
    detail: Option<String>,
    buttons: Vec<String>,
    /// Weak: the window's prompt owns the view, and no entity handle may
    /// outlive the test app.
    view: gpui::WeakEntity<TestAlert>,
}

/// Stand-in for the native alert. GPUI keeps it as the window's prompt
/// until `answerAlert` emits the chosen button index.
struct TestAlert {
    focus: gpui::FocusHandle,
}

impl gpui::EventEmitter<gpui::PromptResponse> for TestAlert {}

impl gpui::Focusable for TestAlert {
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl gpui::Render for TestAlert {
    fn render(
        &mut self,
        _window: &mut gpui::Window,
        _cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        gpui::div()
    }
}

/// An offscreen window painting the subtree under `root`.
struct TestWindow {
    root: u64,
    view: gpui::Entity<SoloView>,
    window: gpui::AnyWindowHandle,
    /// See [`VisualTestState::main_tabbing_identifier`].
    tabbing_identifier: String,
}

thread_local! {
    static TEST_STATE: RefCell<Option<VisualTestState>> = const { RefCell::new(None) };
}

/// Access the test app context + window + view mutably within thread_local.
/// The closure receives (&mut cx, window_handle, &view_entity).
/// Returns Err if no TestSoloRenderer has been created on this thread.
fn with_test_state<R>(
    f: impl FnOnce(&mut TestCx, AnyWindowHandle, &Entity<SoloView>) -> Result<R>,
) -> Result<R> {
    TEST_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
        f(&mut state.cx, state.window, &state.view)
    })
}

/// Whether a `TestSoloRenderer` exists on this thread. Free napi helpers use
/// it to prefer the test app over the live UI thread.
pub(crate) fn has_app() -> bool {
    TEST_STATE.with(|cell| cell.borrow().is_some())
}

/// Run `f` against the test app, if a `TestSoloRenderer` exists on this
/// thread.
pub(crate) fn with_app<R>(f: impl FnOnce(&mut gpui::App) -> R) -> Option<R> {
    TEST_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        borrow.as_mut().map(|state| state.cx.update(|app| f(app)))
    })
}

/// Like `with_test_state`, on the window that paints `root`: an `openWindow`
/// window for `Some`, the main window for `None`.
fn with_window<R>(
    root: Option<u64>,
    f: impl FnOnce(&mut TestCx, AnyWindowHandle, &Entity<SoloView>) -> Result<R>,
) -> Result<R> {
    TEST_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
        let (window, view) = match root {
            None => (state.window, state.view.clone()),
            Some(root) => state
                .secondary
                .iter()
                .find(|window| window.root == root)
                .map(|window| (window.window, window.view.clone()))
                .ok_or_else(|| Error::from_reason(format!("Window {root} is not open")))?,
        };
        f(&mut state.cx, window, &view)
    })
}

/// Run `f` against the root view of the window that paints `root`.
fn in_view<R>(
    root: Option<u64>,
    f: impl FnOnce(&mut SoloView, &mut gpui::Window, &mut gpui::Context<SoloView>) -> R,
) -> Result<R> {
    with_window(root, |cx, window, view| {
        let view = view.clone();
        cx.update_window(window, |_, window, app| {
            view.update(app, |view, cx| f(view, window, cx))
        })
        .map_err(|e| Error::from_reason(e.to_string()))
    })
}

/// Run `f` on the NSWindow that paints `root`, outside the test state:
/// AppKit calls back into GPUI synchronously. Then settle the app.
#[cfg(target_os = "macos")]
fn with_appkit_window<R>(
    root: Option<u64>,
    f: impl FnOnce(&objc2_app_kit::NSWindow) -> Result<R>,
) -> Result<R> {
    let window = with_window(root, |cx, window, _view| {
        cx.update_window(window, |_, window, _| crate::renderer::appkit_window(window))
            .map_err(|e| Error::from_reason(e.to_string()))?
    })?;
    let result = f(&window)?;
    with_test_state(|cx, _window, _view| {
        cx.run_until_parked();
        Ok(())
    })?;
    Ok(result)
}

#[cfg(target_os = "macos")]
fn validate_display_count(display_count: usize) -> Result<()> {
    if display_count == 0 {
        return Err(Error::from_reason(
            "TestSoloRenderer requires an interactive macOS display; NSScreen reported no available displays",
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_display_available() -> Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        Error::from_reason("TestSoloRenderer must be created on the macOS main thread")
    })?;

    validate_display_count(NSScreen::screens(mtm).len())
}

/// A test renderer needs no display on Linux: the backing platform is
/// headless, so nothing has to be on screen.
#[cfg(not(target_os = "macos"))]
fn ensure_display_available() -> Result<()> {
    Ok(())
}

/// The GPUI platform backing the macOS test app: the real Metal platform, so
/// screenshots reflect what apps see. Linux uses `HeadlessAppContext`
/// instead, which brings its own `TestPlatform`.
#[cfg(target_os = "macos")]
fn test_platform() -> Rc<dyn gpui::Platform> {
    Rc::new(gpui_macos::MacPlatform::new(false))
}

// ── TestSoloRenderer ────────────────────────────────────────────────

crate::renderer::tree_mutation_api!(TestSoloRenderer);

/// GPU-backed GPUI test renderer. Uses VisualTestAppContext and Metal on
/// macOS, HeadlessAppContext and wgpu on Linux, both with TestDispatcher for
/// deterministic scheduling. Same SoloView and rendering pipeline as
/// production.
///
/// Usage from JS:
///   const r = new TestSoloRenderer()
///   r.createElement(1, "div")
///   r.setRoot(1)
///   r.commitMutations()
///   r.flush()                  // triggers SoloView::render()
///   r.simulateClick(50, 50)    // dispatches through GPUI hit testing
///   const events = r.drainEvents()
///   r.captureScreenshot("/tmp/test.png")  // saves rendered UI as PNG
#[napi]
pub struct TestSoloRenderer {
    tree: Arc<Mutex<RetainedTree>>,
    events: Arc<Mutex<Vec<EventPayload>>>,
    file_dialog_state: Arc<Mutex<crate::file_dialogs::DialogState>>,
    alert_state: Arc<Mutex<crate::file_dialogs::DialogState>>,
    /// Same handle SoloView paints against, so tests can assert on the live
    /// selection after simulating a drag.
    selection: crate::text::SharedSelection,
    /// The main view's callback, for the windows `openWindow` adds.
    event_callback: Option<EventCallback>,
}

#[napi]
impl TestSoloRenderer {
    #[napi(constructor)]
    pub fn new() -> Result<Self> {
        ensure_display_available()?;

        let tree = Arc::new(Mutex::new(RetainedTree::new()));
        let events: Arc<Mutex<Vec<EventPayload>>> = Arc::new(Mutex::new(Vec::new()));

        // Event callback: push to Vec instead of ThreadsafeFunction.
        let events_clone = events.clone();
        let event_callback: Option<EventCallback> = Some(Arc::new(move |payload: EventPayload| {
            events_clone.lock().unwrap().push(payload);
        }));

        let tree_clone = tree.clone();
        let callback_clone = event_callback.clone();
        let event_callback_for_windows = event_callback.clone();
        let selection = crate::text::SharedSelection::default();
        let selection_clone = selection.clone();

        // Create the test context with deterministic scheduling. On macOS
        // it is the Metal-backed visual context; on Linux the cross-platform
        // headless context with real text shaping and no rasterizer.
        let mut cx = new_test_context();
        let alert: Rc<RefCell<Option<PendingAlert>>> = Rc::default();
        let pending = alert.clone();
        cx.update(|cx| {
            crate::renderer::init_key_bindings(cx);
            crate::custom_elements::input::init(cx);
            // Offscreen windows would still show a real NSAlert sheet.
            cx.set_prompt_builder(move |level, message, detail, buttons, handle, window, cx| {
                let view = cx.new(|cx| TestAlert {
                    focus: cx.focus_handle(),
                });
                *pending.borrow_mut() = Some(PendingAlert {
                    window: window.window_handle(),
                    level,
                    message: message.to_string(),
                    detail: detail.map(str::to_string),
                    buttons: buttons.iter().map(|button| button.label().to_string()).collect(),
                    view: view.downgrade(),
                });
                handle.with_view(view, window, cx)
            });
        });

        // Open the main window. macOS puts it offscreen at (-10000, -10000)
        // (invisible but fully rendered by Metal); Linux uses a headless
        // window that the wgpu headless renderer rasterizes. Both run the
        // same SoloView as production.
        let window_handle = open_window_default(&mut cx, |_window, app| {
            app.new(|_cx| {
                SoloView::new(
                    tree_clone,
                    callback_clone,
                    "Solo Test".to_string(),
                    selection_clone,
                )
            })
        })
        .map_err(|e| Error::from_reason(format!("Failed to open test window: {}", e)))?;

        // Get the root entity (Entity<SoloView>) from the window.
        let view = window_handle
            .entity(&cx)
            .map_err(|e| Error::from_reason(format!("Failed to get root view: {}", e)))?;

        // Convert typed WindowHandle<SoloView> to AnyWindowHandle for simulation methods.
        let window: gpui::AnyWindowHandle = window_handle.into();
        park_pointer_outside_window(&mut cx, window);

        // Store !Send types on the JS main thread.
        TEST_STATE.with(|cell| {
            *cell.borrow_mut() = Some(VisualTestState {
                cx,
                window,
                view,
                secondary: Vec::new(),
                alert,
                main_tabbing_identifier: String::new(),
            });
        });

        Ok(Self {
            tree,
            events,
            file_dialog_state: Arc::new(Mutex::new(Default::default())),
            alert_state: Arc::new(Mutex::new(Default::default())),
            selection,
            event_callback: event_callback_for_windows,
        })
    }

    /// The `openWindow` root that paints element `id`, if any.
    fn window_root_of(&self, id: Option<f64>) -> Result<Option<u64>> {
        let Some(id) = id else {
            return Ok(None);
        };
        let id = to_element_id(id)?;
        Ok(self.tree.lock().unwrap().window_root_of(id))
    }

    // ── Windows (same interface as SoloRenderer) ─────────────────────

    /// Open an offscreen window painting `root_id`, a parentless element.
    #[napi]
    pub fn open_window(&self, root_id: f64, options: Option<WindowOptions>) -> Result<()> {
        let root = to_element_id(root_id)?;
        let options = options.unwrap_or_default();
        if options.menu_bar.is_some() {
            return Err(Error::from_reason(
                "menuBar is an application option; set it on the main window",
            ));
        }
        validate_window_options(&options)?;
        self.tree
            .lock()
            .unwrap()
            .add_window_root(root)
            .map_err(Error::from_reason)?;
        let opened = with_test_state(|cx, _window, view| {
            // Offscreen windows have no screen, but an unknown one still fails.
            cx.update(|app| crate::screens::display_for(options.screen.as_deref(), app))?;
            // One automation clock for the whole app.
            let clock = cx.update(|app| view.read(app).clock.clone());
            let size = gpui::size(
                gpui::px(options.width.unwrap_or(800.0) as f32),
                gpui::px(options.height.unwrap_or(600.0) as f32),
            );
            let tree = self.tree.clone();
            let callback = self.event_callback.clone();
            let title = options.title.clone().unwrap_or_else(|| "Solo".to_string());
            let selection = self.selection.clone();
            let handle = open_window_at(cx, size, |_window, app| {
                app.new(|_cx| {
                    SoloView::new(tree, callback, title, selection)
                        .for_window_root(root, Some(clock))
                })
            })
            .map_err(|e| Error::from_reason(format!("Failed to open test window: {}", e)))?;
            let view = handle
                .entity(cx)
                .map_err(|e| Error::from_reason(format!("Failed to get root view: {}", e)))?;
            let window: gpui::AnyWindowHandle = handle.into();
            park_pointer_outside_window(cx, window);
            cx.run_until_parked();
            Ok(TestWindow {
                root,
                view,
                window,
                tabbing_identifier: String::new(),
            })
        });
        match opened {
            Ok(window) => {
                TEST_STATE.with(|cell| {
                    if let Some(state) = cell.borrow_mut().as_mut() {
                        state.secondary.push(window);
                    }
                });
                Ok(())
            }
            Err(error) => {
                self.tree.lock().unwrap().window_roots.retain(|id| *id != root);
                Err(error)
            }
        }
    }

    /// Close a window opened with `openWindow` and emit `windowClosed` on its
    /// root, as the live renderer does. The main window cannot be closed.
    #[napi]
    pub fn close_window(&self, window_id: Option<f64>) -> Result<()> {
        let Some(window_id) = window_id else {
            return Err(Error::from_reason(
                "TestSoloRenderer cannot close its main window",
            ));
        };
        let root = to_element_id(window_id)?;
        let closed = TEST_STATE.with(|cell| -> Result<TestWindow> {
            let mut borrow = cell.borrow_mut();
            let state = borrow
                .as_mut()
                .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
            let index = state
                .secondary
                .iter()
                .position(|window| window.root == root)
                .ok_or_else(|| Error::from_reason(format!("Window {root} is not open")))?;
            let closed = state.secondary.remove(index);
            state
                .cx
                .update_window(closed.window, |_, window, _app| window.remove_window())
                .map_err(|e| Error::from_reason(e.to_string()))?;
            state.cx.run_until_parked();
            Ok(closed)
        })?;
        crate::renderer::forget_window_state(closed.window.window_id());
        let ids = {
            let mut tree = self.tree.lock().unwrap();
            tree.window_roots.retain(|id| *id != root);
            tree.subtree_ids(root)
        };
        crate::renderer::release_window_subtree(&ids);
        emit_event_full(&self.event_callback, root, WINDOW_CLOSED_EVENT, |_| {});
        Ok(())
    }

    /// Opt a window in or out of close requests, as the live renderer does.
    #[napi]
    pub fn set_window_close_guard(&self, guarded: bool, window_id: Option<f64>) -> Result<()> {
        let target = crate::renderer::WindowTarget::from_arg(window_id)?;
        let root = window_id.map(to_element_id).transpose()?;
        in_view(root, move |view, _window, _cx| {
            view.tab_for_mut(target).close_guarded = guarded
        })
    }

    /// Ask to close a window as its close button does: a guarded window stays
    /// open and emits `windowCloseRequested`; any other closes like
    /// `closeWindow`. This makes the close guard's decision directly rather
    /// than through AppKit's `performClose:`, which would also call back into
    /// windows left by earlier test renderers, whose GPUI app is gone.
    #[napi]
    pub fn request_window_close(&self, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        if in_view(root, |view, _window, _cx| view.allow_close())? {
            return self.close_window(window_id);
        }
        Ok(())
    }

    /// A window's close handler kept it open. As in the live renderer on
    /// macOS, each window is asked on its own, so there is nothing to drop.
    #[napi]
    pub fn decline_window_close(&self, window_id: Option<f64>) -> Result<()> {
        window_id.map(to_element_id).transpose()?;
        Ok(())
    }

    /// Enter or leave native fullscreen, as the live renderer does.
    #[napi]
    pub fn set_window_fullscreen(&self, fullscreen: bool, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| {
            crate::renderer::set_appkit_fullscreen(window, fullscreen);
            Ok(())
        });
        #[cfg(not(target_os = "macos"))]
        {
            return in_view(root, move |_view, window, _cx| {
                if window.is_fullscreen() != fullscreen {
                    window.toggle_fullscreen();
                }
            });
        }
    }

    #[napi]
    pub fn is_window_fullscreen(&self, window_id: Option<f64>) -> Result<bool> {
        let root = window_id.map(to_element_id).transpose()?;
        in_view(root, |_view, window, _cx| window.is_fullscreen())
    }

    #[napi]
    pub fn minimize_window(&self, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| {
            window.miniaturize(None);
            Ok(())
        });
        #[cfg(not(target_os = "macos"))]
        {
            return in_view(root, |_view, window, _cx| window.minimize_window());
        }
    }

    #[napi]
    pub fn set_window_background(&self, background: String, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        let appearance = crate::renderer::window_background_appearance(&background)?;
        in_view(root, move |_view, window, _cx| {
            window.set_background_appearance(appearance)
        })
    }

    /// Resize the window's content to `width` × `height` logical pixels, the
    /// size `getWindowSize` reports.
    #[napi]
    pub fn resize_window(&self, width: f64, height: f64, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| {
            crate::renderer::resize_appkit_window(window, width, height)
        });
        #[cfg(not(target_os = "macos"))]
        {
            let size = crate::renderer::validated_window_size(width, height)?;
            return in_view(root, move |_view, window, cx| {
                window.resize(size);
                // `resize` only updates the platform window on Linux; the
                // cached viewport size moves in `bounds_changed`, which the
                // platform would normally call.
                window.bounds_changed(cx);
            });
        }
    }

    #[napi]
    pub fn set_window_tabbing_identifier(
        &self,
        identifier: Option<String>,
        window_id: Option<f64>,
    ) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let root = window_id.map(to_element_id).transpose()?;
            return in_view(root, move |_view, window, _cx| {
                window.set_tabbing_identifier(identifier)
            });
        }
        #[cfg(not(target_os = "macos"))]
        {
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            TEST_STATE.with(|cell| {
                let mut borrow = cell.borrow_mut();
                let state = borrow
                    .as_mut()
                    .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
                let identifier = identifier.unwrap_or_default();
                match target {
                    crate::renderer::WindowTarget::Main => {
                        state.main_tabbing_identifier = identifier;
                    }
                    crate::renderer::WindowTarget::Root(root) => {
                        let window = state
                            .secondary
                            .iter_mut()
                            .find(|window| window.root == root)
                            .ok_or_else(|| {
                                Error::from_reason(format!("Window {root} is not open"))
                            })?;
                        window.tabbing_identifier = identifier;
                    }
                }
                Ok(())
            })
        }
    }

    #[napi]
    pub fn get_window_tabbing_identifier(&self, window_id: Option<f64>) -> Result<String> {
        #[cfg(target_os = "macos")]
        {
            let root = window_id.map(to_element_id).transpose()?;
            return with_appkit_window(root, |window| Ok(window.tabbingIdentifier().to_string()));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            TEST_STATE.with(|cell| {
                let borrow = cell.borrow();
                let state = borrow
                    .as_ref()
                    .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
                match target {
                    crate::renderer::WindowTarget::Main => {
                        Ok(state.main_tabbing_identifier.clone())
                    }
                    crate::renderer::WindowTarget::Root(root) => state
                        .secondary
                        .iter()
                        .find(|window| window.root == root)
                        .map(|window| window.tabbing_identifier.clone())
                        .ok_or_else(|| Error::from_reason(format!("Window {root} is not open"))),
                }
            })
        }
    }

    #[napi]
    pub fn toggle_window_tab_overview(&self, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| {
            window.toggleTabOverview(None);
            Ok(())
        });
        #[cfg(not(target_os = "macos"))]
        {
            let _ = root;
            Err(Error::from_reason(
                "The tab overview is macOS-only; on Linux the tabs are the window's tab strip",
            ))
        }
    }

    #[napi]
    pub fn show_character_palette(&self, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return in_view(root, |_view, window, _cx| window.show_character_palette());
        #[cfg(not(target_os = "macos"))]
        {
            let _ = root;
            Err(Error::from_reason(
                "The character palette is supported only on macOS",
            ))
        }
    }

    #[napi]
    pub fn set_window_document_edited(&self, edited: bool, window_id: Option<f64>) -> Result<()> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return in_view(root, move |_view, window, _cx| window.set_window_edited(edited));
        #[cfg(not(target_os = "macos"))]
        {
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            return in_view(root, move |view, _window, _cx| {
                view.tab_for_mut(target).document_edited = edited;
            });
        }
    }

    #[napi]
    pub fn set_window_document_path(&self, path: Option<String>, window_id: Option<f64>) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let path = crate::renderer::document_path(path)?;
            let root = window_id.map(to_element_id).transpose()?;
            return in_view(root, move |_view, window, _cx| window.set_document_path(path.as_deref()));
        }
        #[cfg(not(target_os = "macos"))]
        {
            let path = crate::renderer::document_path(path)?;
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            let root = window_id.map(to_element_id).transpose()?;
            let path = path.map(|path| path.to_string_lossy().into_owned());
            return in_view(root, move |view, _window, _cx| {
                view.tab_for_mut(target).document_path = path;
            });
        }
    }

    #[napi]
    pub fn is_window_document_edited(&self, window_id: Option<f64>) -> Result<bool> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| Ok(window.isDocumentEdited()));
        #[cfg(not(target_os = "macos"))]
        {
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            return in_view(root, move |view, _window, _cx| view.tab_for(target).document_edited);
        }
    }

    #[napi]
    pub fn get_window_document_path(&self, window_id: Option<f64>) -> Result<Option<String>> {
        let root = window_id.map(to_element_id).transpose()?;
        #[cfg(target_os = "macos")]
        return with_appkit_window(root, |window| Ok(crate::renderer::represented_filename(window)));
        #[cfg(not(target_os = "macos"))]
        {
            let target = crate::renderer::WindowTarget::from_arg(window_id)?;
            return in_view(root, move |view, _window, _cx| {
                view.tab_for(target).document_path.clone()
            });
        }
    }

    /// Content size of the main window, or of the window `window_id`.
    #[napi]
    pub fn get_window_size(&self, window_id: Option<f64>) -> Result<WindowSize> {
        let root = window_id.map(to_element_id).transpose()?;
        with_window(root, |cx, window, _view| {
            cx.update_window(window, |_, window, _| window_size(window))
                .map_err(|e| Error::from_reason(e.to_string()))
        })
    }

    /// JSON array of the trees of the windows opened with `openWindow`.
    #[napi]
    pub fn get_window_automation_trees(&self) -> Result<String> {
        self.flush()?;
        let json = self
            .tree
            .lock()
            .unwrap()
            .windows_to_json(&crate::automation::all_bounds());
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    // ── Mutation API (same interface as SoloRenderer) ────────────────

    #[napi]
    pub fn set_application_commands(&self, json: String) -> Result<()> {
        let commands = crate::application_commands::parse_application_commands(&json)?;
        let events = self.events.clone();
        let callback: Option<EventCallback> = Some(Arc::new(move |event| {
            events.lock().unwrap().push(event);
        }));
        with_test_state(|cx, _window, _view| {
            cx.update(|cx| {
                crate::application_commands::replace_application_commands(commands, callback, cx)
            })
        })
    }

    #[napi]
    pub fn set_menu_bar(&self, _json: Option<String>) -> Result<()> {
        Err(Error::from_reason(
            "Menu-bar configuration requires a live macOS application",
        ))
    }

    #[napi]
    pub fn select_files(
        &self,
        options_json: String,
    ) -> Result<AsyncTask<crate::file_dialogs::OpenDialogTask>> {
        let options = crate::file_dialogs::parse_open_options(&options_json)?;
        let lease = crate::file_dialogs::DialogLease::acquire(self.file_dialog_state.clone())?;
        let receiver = with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, _window, app| {
                app.prompt_for_paths(options.to_path_prompt_options())
            })
            .map_err(|error| Error::from_reason(error.to_string()))
        })?;
        Ok(crate::file_dialogs::OpenDialogTask::new(receiver, lease))
    }

    #[napi]
    pub fn select_save_path(
        &self,
        options_json: String,
    ) -> Result<AsyncTask<crate::file_dialogs::SaveDialogTask>> {
        let options = crate::file_dialogs::parse_save_options(&options_json)?;
        let lease = crate::file_dialogs::DialogLease::acquire(self.file_dialog_state.clone())?;
        let receiver = with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, _window, app| {
                app.prompt_for_new_path(
                    &options.initial_directory,
                    options.suggested_name.as_deref(),
                )
            })
            .map_err(|error| Error::from_reason(error.to_string()))
        })?;
        Ok(crate::file_dialogs::SaveDialogTask::new(receiver, lease))
    }

    #[napi]
    pub fn show_alert(
        &self,
        options_json: String,
        window_id: Option<f64>,
    ) -> Result<AsyncTask<crate::alerts::AlertTask>> {
        let options = crate::alerts::parse_alert_options(&options_json)?;
        let root = window_id.map(to_element_id).transpose()?;
        let lease = crate::file_dialogs::DialogLease::acquire_as(
            self.alert_state.clone(),
            crate::alerts::ALERT_ALREADY_OPEN,
        )?;
        let receiver = with_window(root, |cx, window, _view| {
            cx.update_window(window, |_, window, app| options.show(window, app))
                .map_err(|error| Error::from_reason(error.to_string()))
        })?;
        Ok(crate::alerts::AlertTask::new(receiver, lease))
    }

    /// Test hook: the open alert as JSON, `{ windowId, level, message, detail,
    /// buttons }`, or null. `windowId` is null for the main window.
    #[napi]
    pub fn pending_alert(&self) -> Result<Option<String>> {
        TEST_STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
            let alert = state.alert.borrow();
            let Some(alert) = alert.as_ref() else {
                return Ok(None);
            };
            let window_id = state
                .secondary
                .iter()
                .find(|window| window.window == alert.window)
                .map(|window| window.root);
            let level = match alert.level {
                gpui::PromptLevel::Info => "info",
                gpui::PromptLevel::Warning => "warning",
                gpui::PromptLevel::Critical => "critical",
            };
            Ok(Some(
                serde_json::json!({
                    "windowId": window_id,
                    "level": level,
                    "message": alert.message,
                    "detail": alert.detail,
                    "buttons": alert.buttons,
                })
                .to_string(),
            ))
        })
    }

    /// Test hook: click button `index` of the open alert.
    #[napi]
    pub fn answer_alert(&self, index: u32) -> Result<()> {
        let alert = TEST_STATE.with(|cell| {
            let borrow = cell.borrow();
            let state = borrow
                .as_ref()
                .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
            let mut alert = state.alert.borrow_mut();
            let count = alert
                .as_ref()
                .ok_or_else(|| Error::from_reason("No alert is open"))?
                .buttons
                .len();
            if index as usize >= count {
                return Err(Error::from_reason(format!(
                    "Button {index} does not exist; the alert has {count}"
                )));
            }
            Ok(alert.take().unwrap())
        })?;
        let index = index as usize;
        with_test_state(|cx, _window, _view| {
            cx.update(|cx| {
                alert
                    .view
                    .update(cx, |_, cx| cx.emit(gpui::PromptResponse(index)))
                    .map_err(|error| Error::from_reason(error.to_string()))
            })
        })
    }

    #[napi]
    pub fn set_system_appearance_subscription(&self, token: Option<String>) -> Result<String> {
        with_test_state(|cx, window, view| {
            let view = view.clone();
            cx.update_window(window, |_, window, app| {
                view.update(app, |view, cx| {
                    view.set_system_appearance_subscription(token, window, cx)
                })
            })
            .map_err(|error| Error::from_reason(error.to_string()))?
        })
    }


    /// Present a lifecycle-managed native menu for an element in this renderer.
    #[napi]
    pub fn show_context_menu(
        &self,
        json: String,
    ) -> Result<AsyncTask<NativeContextMenuTask>> {
        self.flush()?;
        // The menu opens in the window of the element it belongs to.
        let owner = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|value| value["elementId"].as_f64());
        let root = self.window_root_of(owner)?;
        #[cfg(target_os = "macos")]
        {
            return with_window(root, |cx, window, _view| {
                cx.update_window(window, |_, window, _app| {
                    // Every menu in this harness is automation's.
                    crate::context_menu::show(&json, &self.tree.lock().unwrap(), window, true)
                })
                .map_err(|error| Error::from_reason(error.to_string()))?
            })
            .map(AsyncTask::new);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let tree = self.tree.clone();
            let receiver = in_view(
                root,
                move |_view, window, cx| -> Result<std::sync::mpsc::Receiver<Option<String>>> {
                    let previous_focus = window.focused(cx);
                    let focus = cx.focus_handle();
                    let receiver = crate::context_menu_gpui::show(
                        &json,
                        &tree.lock().unwrap(),
                        focus.clone(),
                        previous_focus,
                    )?;
                    focus.focus(window, cx);
                    // Physical keys reach the menu before any key binding or
                    // focused input, whichever element currently has focus.
                    let weak = cx.weak_entity();
                    let subscription = cx.intercept_keystrokes(move |event, window, cx| {
                        if crate::context_menu_gpui::handle_key(&event.keystroke.key) {
                            crate::context_menu_gpui::restore_focus(window, cx);
                            if let Some(view) = weak.upgrade() {
                                view.update(cx, |_view, cx| cx.notify());
                            }
                            cx.stop_propagation();
                        }
                    });
                    crate::context_menu_gpui::set_interceptor(subscription);
                    cx.notify();
                    Ok(receiver)
                },
            )??;
            Ok(AsyncTask::new(crate::context_menu_gpui::MenuTask::new(
                receiver,
            )))
        }
    }

    #[napi]
    pub fn cancel_context_menu(&self, request_id: f64) -> Result<()> {
        let request_id = to_element_id(request_id)?;
        #[cfg(target_os = "macos")]
        {
            crate::context_menu::cancel(request_id);
            Ok(())
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Cancel in the window that owns the menu, as the live renderer
            // does; a menu opened in a secondary window repaints there.
            let root = match crate::context_menu_gpui::owner() {
                Some(owner) => self.window_root_of(Some(owner as f64))?,
                None => None,
            };
            in_view(root, move |_view, window, cx| {
                if crate::context_menu_gpui::cancel(request_id) {
                    crate::context_menu_gpui::restore_focus(window, cx);
                    cx.notify();
                }
            })
        }
    }

    /// Evaluate JavaScript in the mounted WebView through the native bridge.
    #[napi(js_name = "evaluateWebviewJavaScript")]
    pub fn evaluate_webview_javascript(
        &self,
        element_id: f64,
        script: String,
    ) -> Result<AsyncTask<crate::native_view::EvaluateJavaScriptTask>> {
        let element_id = to_element_id(element_id)?;
        let root = self.tree.lock().unwrap().window_root_of(element_id);
        let receiver = with_window(root, |cx, window, _view| {
            cx.update_window(window, |_, _window, _app| {
                crate::native_view::with_registry(|registry| {
                    registry
                        .evaluate_javascript(element_id, &script)
                        .map_err(|error| Error::from_reason(error.to_string()))
                })
            })
            .map_err(|error| Error::from_reason(error.to_string()))?
        })?;
        Ok(crate::native_view::EvaluateJavaScriptTask::new(receiver))
    }

    #[napi]
    pub fn wait_webview_ready(
        &self,
        element_id: f64,
    ) -> Result<AsyncTask<crate::native_view::WebviewReadyTask>> {
        let element_id = to_element_id(element_id)?;
        let root = self.tree.lock().unwrap().window_root_of(element_id);
        let receiver = with_window(root, |cx, window, _view| {
            cx.update_window(window, |_, _window, _app| {
                crate::native_view::with_registry(|registry| {
                    registry
                        .wait_for_ready(element_id)
                        .map_err(|error| Error::from_reason(error.to_string()))
                })
            })
            .map_err(|error| Error::from_reason(error.to_string()))?
        })?;
        Ok(crate::native_view::WebviewReadyTask::new(receiver))
    }

    #[napi]
    pub fn allow_webview_navigation(&self, element_id: f64, navigation_id: f64) -> Result<()> {
        self.decide_webview_navigation(element_id, navigation_id, true)
    }

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
        let root = self.tree.lock().unwrap().window_root_of(element_id);
        with_window(root, |cx, window, _view| {
            cx.update_window(window, |_, _window, _app| {
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
            })
            .map_err(|error| Error::from_reason(error.to_string()))?
        })?;
        Ok(())
    }

    /// Signal that a batch of mutations is complete.
    /// In tests, this is a no-op — flush() handles the actual re-render.
    #[napi]
    pub fn commit_mutations(&self) -> Result<()> {
        Ok(())
    }

    /// Apply a batch of mutations in a single FFI call.
    /// Same format as SoloRenderer::apply_batch (string op names).
    /// Returns accumulated destroyed IDs from all destroyElement ops.
    #[napi]
    pub fn apply_batch(&self, json: String) -> Result<Vec<f64>> {
        let ops = parse_batch(&json)?;
        Ok(apply_batch_to_tree(&mut self.tree.lock().unwrap(), ops))
    }

    // ── Test-specific methods ────────────────────────────────────────

    /// Notify the view entity and run GPUI until parked.
    /// This triggers SoloView::render() → build_element() → GPUI layout.
    /// Must be called after mutations and before simulating events (GPUI's
    /// hit testing requires elements to be laid out).
    #[napi]
    pub fn flush(&self) -> Result<()> {
        TEST_STATE.with(|cell| {
            let mut borrow = cell.borrow_mut();
            let state = borrow
                .as_mut()
                .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
            let windows = std::iter::once((state.window, state.view.clone()))
                .chain(
                    state
                        .secondary
                        .iter()
                        .map(|window| (window.window, window.view.clone())),
                )
                .collect::<Vec<_>>();
            for (window, view) in windows {
                state
                    .cx
                    .update_window(window, |_, _window, app| {
                        view.update(app, |_, cx| {
                            cx.notify();
                        });
                    })
                    .map_err(|e| Error::from_reason(e.to_string()))?;
            }

            state.cx.run_until_parked();
            Ok(())
        })
    }

    /// Simulate a click at the given window coordinates.
    /// Dispatches MouseDown + MouseUp through GPUI's input pipeline,
    /// which triggers the same event handlers as production.
    /// IMPORTANT: Call flush() before this — hit testing requires laid-out elements.
    /// Coordinates are window-local; `element_id` picks that element's window.
    #[napi]
    pub fn simulate_click(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        let button = crate::automation::mouse_button(button.unwrap_or(0));
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let position = gpui::point(gpui::px(x as f32), gpui::px(y as f32));
            simulate_mouse_down(cx, window, position, button, gpui::Modifiers::default());
            simulate_mouse_up(cx, window, position, button, gpui::Modifiers::default());
            Ok(())
        })
    }

    /// Simulate key strokes through GPUI's input pipeline.
    /// Format: space-separated keys, e.g. "a", "enter", "cmd-shift-p".
    /// The focused element receives keyDown/keyUp events.
    #[napi]
    pub fn simulate_keystrokes(&self, keystrokes: String, element_id: Option<f64>) -> Result<()> {
        let keys = crate::automation::parse_keystrokes(&keystrokes).map_err(Error::from_reason)?;
        #[cfg(target_os = "macos")]
        if crate::context_menu::dispatch_keys(&keys) {
            return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = keys;
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            simulate_keystrokes(cx, window, &keystrokes);
            Ok(())
        })
    }

    /// Simulate a single key down event through GPUI's input pipeline.
    /// Format: modifier-key string, e.g. "a", "enter", "cmd-s".
    /// Unlike simulate_keystrokes, this dispatches ONLY a KeyDownEvent —
    /// no automatic KeyUpEvent follows. Use with simulate_key_up for
    /// fine-grained key event testing.
    #[napi]
    pub fn simulate_key_down(
        &self,
        keystroke: String,
        is_held: Option<bool>,
        element_id: Option<f64>,
    ) -> Result<()> {
        let key = crate::automation::parse_keystroke(&keystroke).map_err(Error::from_reason)?;
        #[cfg(target_os = "macos")]
        if crate::context_menu::dispatch_keys(std::slice::from_ref(&key)) {
            return Ok(());
        }
        #[cfg(not(target_os = "macos"))]
        let _ = key;
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let parsed = gpui::Keystroke::parse(&keystroke).map_err(|e| {
                Error::from_reason(format!("Invalid keystroke '{}': {}", keystroke, e))
            })?;

            simulate_event(
                cx,
                window,
                gpui::KeyDownEvent {
                    keystroke: parsed,
                    is_held: is_held.unwrap_or(false),
                    prefer_character_input: false,
                },
            );

            Ok(())
        })
    }

    /// Simulate a single key up event through GPUI's input pipeline.
    /// Format: modifier-key string, e.g. "a", "enter", "cmd-s".
    /// Pairs with simulate_key_down for fine-grained key event testing.
    #[napi]
    pub fn simulate_key_up(&self, keystroke: String, element_id: Option<f64>) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let parsed = gpui::Keystroke::parse(&keystroke).map_err(|e| {
                Error::from_reason(format!("Invalid keystroke '{}': {}", keystroke, e))
            })?;

            simulate_event(cx, window, gpui::KeyUpEvent { keystroke: parsed });

            Ok(())
        })
    }

    /// Simulate a mouse move to the given coordinates.
    /// pressed_button: optional mouse button held during move (0=left, 1=middle, 2=right).
    /// Used to simulate drag events.
    #[napi]
    pub fn simulate_mouse_move(
        &self,
        x: f64,
        y: f64,
        pressed_button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let button: Option<gpui::MouseButton> = pressed_button.map(crate::automation::mouse_button);

            simulate_mouse_move(
                cx,
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                button,
                gpui::Modifiers::default(),
            );

            Ok(())
        })
    }

    /// Drag files from outside the app (e.g. Finder) over window coordinates
    /// `(x, y)` without dropping them, so `fileDragOver` styles apply.
    #[napi]
    pub fn simulate_file_drag_over(
        &self,
        paths: Vec<String>,
        x: f64,
        y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            cx.update_window(window, |_, window, cx| {
                crate::automation::dispatch_file_drag_over(window, cx, paths, x, y);
            })
            .map_err(|error| Error::from_reason(error.to_string()))?;
            cx.run_until_parked();
            Ok(())
        })
    }

    /// Drop files from outside the app (e.g. Finder) at window coordinates
    /// `(x, y)`. The element under that point with `onFileDrop` receives them.
    #[napi]
    pub fn simulate_file_drop(
        &self,
        paths: Vec<String>,
        x: f64,
        y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            cx.update_window(window, |_, window, cx| {
                crate::automation::dispatch_file_drop(window, cx, paths, x, y);
            })
            .map_err(|error| Error::from_reason(error.to_string()))?;
            cx.run_until_parked();
            Ok(())
        })
    }

    /// Focus an element by its numeric ID.
    /// The element must have a FocusHandle (created by sync_focus_handles when
    /// the element has keyDown, keyUp, focus, or blur listeners).
    /// Call flush() before this so the element tree and focus handles exist.
    #[napi]
    pub fn focus_element(&self, id: f64) -> Result<()> {
        let root = self.window_root_of(Some(id))?;
        let id = to_element_id(id)?;
        in_view(root, |view, window, cx| {
            view.focus_element(id, window, cx);
            cx.notify();
        })?;
        with_window(root, |cx, _window, _view| {
            cx.run_until_parked();
            Ok(())
        })
    }

    /// Simulate a mouse down event at the given window coordinates.
    /// Button: 0=left, 1=middle, 2=right. Defaults to left (0).
    #[napi]
    pub fn simulate_mouse_down(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            simulate_mouse_down(
                cx,
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                crate::automation::mouse_button(button.unwrap_or(0)),
                gpui::Modifiers::default(),
            );
            Ok(())
        })
    }

    /// Simulate a mouse up event at the given window coordinates.
    /// Button: 0=left, 1=middle, 2=right. Defaults to left (0).
    #[napi]
    pub fn simulate_mouse_up(
        &self,
        x: f64,
        y: f64,
        button: Option<u32>,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            simulate_mouse_up(
                cx,
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                crate::automation::mouse_button(button.unwrap_or(0)),
                gpui::Modifiers::default(),
            );
            Ok(())
        })
    }

    /// Simulate a scroll wheel event at the given position.
    /// delta_x and delta_y are in pixels (negative = scroll up/left).
    #[napi]
    pub fn simulate_scroll_wheel(
        &self,
        x: f64,
        y: f64,
        delta_x: f64,
        delta_y: f64,
        element_id: Option<f64>,
    ) -> Result<()> {
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            simulate_event(
                cx,
                window,
                gpui::ScrollEvent {
                    position: gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::px(delta_x as f32),
                        gpui::px(delta_y as f32),
                    )),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
                    source: gpui::ScrollSource::Pointer,
                },
            );
            Ok(())
        })
    }

    // ── Selection API ──────────────────────────────────────────────────

    /// The current text selection joined in document order, or null.
    #[napi]
    pub fn get_selected_text(&self) -> Option<String> {
        self.selection.lock().selected_text()
    }

    /// Drop the current selection.
    #[napi]
    pub fn clear_selection(&self) {
        self.selection.lock().clear();
    }

    /// Syntax-cache counters as `[hits, misses, documents]`.
    ///
    /// Solo rebuilds its whole element tree every frame, so a `<code>` block
    /// that misses the cache reparses at frame rate. A test can watch the hit
    /// count to catch that regression before a profiler does.
    #[napi]
    pub fn get_syntax_cache_stats(&self) -> Vec<f64> {
        let stats = crate::syntax::cache::stats();
        vec![
            stats.hits as f64,
            stats.misses as f64,
            stats.documents as f64,
        ]
    }

    /// Every string painted in the last frame, in paint order.
    ///
    /// `getAllText()` only sees `<text>` nodes in the retained tree. Native
    /// elements such as `<code>` and `<diff>` draw their text inside gpui, so
    /// this is the only way to assert on what they actually rendered.
    #[napi]
    pub fn get_painted_text(&self) -> Result<Vec<String>> {
        self.flush()?;
        Ok(crate::text::painted_text())
    }

    /// Drag-select from one point to another: mouse down, move, up.
    ///
    /// A single helper rather than three calls because the listeners that drive
    /// selection are registered during **paint**, so a flush must sit between
    /// the down and the move. Getting that order wrong silently selects nothing,
    /// which is a miserable thing to debug from JS.
    #[napi]
    pub fn drag_select(&self, x1: f64, y1: f64, x2: f64, y2: f64) -> Result<()> {
        self.flush()?;
        self.simulate_mouse_down(x1, y1, None, None)?;
        self.flush()?;
        self.simulate_mouse_move(x2, y2, Some(0), None)?;
        self.flush()?;
        self.simulate_mouse_up(x2, y2, None, None)?;
        self.flush()?;
        Ok(())
    }

    // ── Scroll API ─────────────────────────────────────────────────────

    /// Set the scroll offset of a scrollable element.
    /// x and y are negative pixel values (scroll down = more negative y).
    /// Call flush() after to apply the offset and re-render.
    #[napi]
    pub fn scroll_to(&self, element_id: f64, x: f64, y: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        in_view(self.window_root_of(Some(element_id))?, |view, _window, _cx| {
            view.scroll_to(id, x as f32, y as f32)
        })
    }

    /// Scroll a child into view by its index in the children list.
    /// Call flush() after to apply and re-render.
    #[napi]
    pub fn scroll_to_item(&self, element_id: f64, index: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        in_view(self.window_root_of(Some(element_id))?, |view, _window, _cx| {
            view.scroll_to_item(id, index as usize)
        })
    }

    /// `"hidden"` | `"minimal"` | `"full"`.
    #[napi]
    pub fn set_debug_frame_overlay(&self, mode: String) -> Result<String> {
        let mode = parse_debug_frame_overlay_mode(&mode)?;
        with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, window, _app| {
                window.set_debug_frame_overlay_mode(mode);
                debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
            })
            .map_err(|e| Error::from_reason(e.to_string()))
        })
    }

    /// Hidden → minimal → full → hidden.
    #[napi]
    pub fn cycle_debug_frame_overlay(&self) -> Result<String> {
        with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, window, _app| {
                window.cycle_debug_frame_overlay_mode();
                debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
            })
            .map_err(|e| Error::from_reason(e.to_string()))
        })
    }

    #[napi]
    pub fn get_debug_frame_overlay(&self) -> Result<String> {
        with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, window, _app| {
                debug_frame_overlay_mode_name(window.debug_frame_overlay_mode()).to_string()
            })
            .map_err(|e| Error::from_reason(e.to_string()))
        })
    }

    /// Clears the last 1000 draw samples. Frame count stays.
    #[napi]
    pub fn reset_debug_frame_overlay_stats(&self) -> Result<()> {
        with_test_state(|cx, window, _view| {
            cx.update_window(window, |_, window, _app| {
                window.reset_debug_frame_overlay_stats();
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;
            Ok(())
        })
    }

    /// Get the current scroll offset of a scrollable element.
    /// Returns [x, y] or null if the element has no scroll handle.
    #[napi]
    pub fn get_scroll_offset(&self, element_id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(element_id)?;
        let offset = in_view(self.window_root_of(Some(element_id))?, |view, _window, _cx| {
            view.scroll_offset(id)
        })?;
        Ok(offset.map(|[x, y]| vec![x, y]))
    }

    /// Capture a screenshot of the current rendered state and save as PNG.
    /// macOS renders through Metal, Linux through the wgpu headless renderer.
    #[napi]
    pub fn capture_screenshot(&self, path: String) -> Result<()> {
        with_test_state(|cx, window, view| {
            let view = view.clone();

            // Flush: notify view and run until parked so layout/rendering are current.
            cx.update_window(window, |_, _window, app| {
                view.update(app, |_, cx| {
                    cx.notify();
                });
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;

            // Force a window refresh before capture so render_to_image reads
            // the most recent frame scene.
            cx.update_window(window, |_, window, _app| {
                window.refresh();
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;

            cx.run_until_parked();

            // Capture via GPUI's render_to_image (Metal texture → RgbaImage).
            let image = cx
                .capture_screenshot(window)
                .map_err(|e| Error::from_reason(format!("Screenshot capture failed: {}", e)))?;

            // Save as PNG (format inferred from file extension).
            image
                .save(&path)
                .map_err(|e| Error::from_reason(format!("Failed to save screenshot: {}", e)))?;

            Ok(())
        })
    }

    /// Return and clear all collected events since the last drain.
    /// Events are collected synchronously — no event loop queuing.
    #[napi]
    pub fn drain_events(&self) -> Vec<EventPayload> {
        let mut events = self.events.lock().unwrap();
        events.drain(..).collect()
    }

    // ── Tree inspection ──────────────────────────────────────────────

    /// Get all text content in the tree (depth-first order).
    #[napi]
    pub fn get_all_text(&self) -> Vec<String> {
        crate::automation::collect_text(&self.tree.lock().unwrap())
    }

    /// Find element IDs matching the given type (e.g. "div", "text").
    #[napi]
    pub fn find_by_type(&self, element_type: String) -> Vec<f64> {
        let tree = self.tree.lock().unwrap();
        tree.elements
            .values()
            .filter(|e| e.element_type == element_type)
            .map(|e| e.id as f64)
            .collect()
    }

    /// Check if an element has a specific event listener.
    #[napi]
    pub fn has_event_listener(&self, id: f64, event_type: String) -> Result<bool> {
        let id = to_element_id(id)?;
        let tree = self.tree.lock().unwrap();
        Ok(tree
            .elements
            .get(&id)
            .map(|e| e.events.contains(&event_type))
            .unwrap_or(false))
    }

    /// Get the text content of an element.
    #[napi]
    pub fn get_text(&self, id: f64) -> Result<Option<String>> {
        let id = to_element_id(id)?;
        let tree = self.tree.lock().unwrap();
        Ok(tree.elements.get(&id).and_then(|e| e.content.as_ref().map(|c| c.to_string())))
    }

    /// Get the full tree as JSON for snapshot testing.
    #[napi]
    pub fn get_tree_json(&self) -> Result<String> {
        let tree = self.tree.lock().unwrap();
        let json = tree.to_json(&std::collections::HashMap::new());
        serde_json::to_string_pretty(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    /// Tree JSON with last-paint bounds. Used by the automation locators.
    #[napi]
    pub fn get_automation_tree(&self) -> Result<String> {
        self.flush()?;
        let tree = self.tree.lock().unwrap();
        let json = tree.to_json(&crate::automation::all_bounds());
        serde_json::to_string(&json)
            .map_err(|e| Error::from_reason(format!("JSON serialization failed: {}", e)))
    }

    /// Last painted bounds for an element, or null if it was not painted.
    #[napi]
    pub fn get_element_bounds(&self, id: f64) -> Result<Option<Vec<f64>>> {
        let id = to_element_id(id)?;
        self.flush()?;
        Ok(crate::automation::get_bounds(id).map(|bounds| {
            vec![bounds.x, bounds.y, bounds.width, bounds.height]
        }))
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
        change: impl FnOnce(&crate::automation::AutomationClock) -> f64,
    ) -> Result<f64> {
        let now_ms = in_view(None, |view, _window, cx| {
            let now_ms = change(&view.clock);
            cx.notify();
            now_ms
        })?;
        with_test_state(|cx, _window, _view| {
            cx.run_until_parked();
            Ok(now_ms)
        })
    }

    /// Get the root element ID, or null if no root is set.
    #[napi]
    pub fn get_root_id(&self) -> Option<f64> {
        self.tree.lock().unwrap().root_id.map(|id| id as f64)
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn display_preflight_rejects_zero_displays() {
        let error = validate_display_count(0).expect_err("zero displays must be rejected");

        assert!(error
            .to_string()
            .contains("requires an interactive macOS display"));
    }

    #[test]
    fn display_preflight_accepts_an_available_display() {
        assert!(validate_display_count(1).is_ok());
    }
}
