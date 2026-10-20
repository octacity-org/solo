/// TestSoloRenderer — GPU-backed GPUI test renderer exposed to Node.js via napi.
///
/// Uses gpui::VisualTestAppContext (real Metal rendering on macOS) with
/// TestDispatcher for deterministic scheduling. Runs the SAME SoloView,
/// build_element(), apply_styles(), and event handlers as production.
///
/// Windows are positioned offscreen at (-10000, -10000) — invisible but
/// fully rendered by Metal. This enables capture_screenshot() for visual
/// test validation.
///
/// VisualTestAppContext is !Send, so it is stored in thread-local state.
/// All napi calls happen on the JS main thread.
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;
use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

use gpui::AppContext as _;

use crate::element_tree::EventPayload;
use crate::renderer::{
    apply_batch_to_tree, debug_frame_overlay_mode_name, emit_event_full,
    parse_debug_frame_overlay_mode, to_element_id, EventCallback, SoloView, WindowOptions,
    WINDOW_CLOSED_EVENT,
};
use crate::retained_tree::RetainedTree;
use crate::style::StyleDesc;

// ── Thread-local storage for !Send GPUI types ────────────────────────

/// Bundles VisualTestAppContext + window handle + view entity.
/// Stored in thread_local because VisualTestAppContext is !Send (Rc<AppCell>).
/// Field order is load-bearing: Rust drops fields in declaration order, and
/// gpui panics at app teardown if an `Entity` handle outlives its `App`.
/// `view` and `secondary` must therefore be declared before `cx`.
struct VisualTestState {
    view: gpui::Entity<SoloView>,
    window: gpui::AnyWindowHandle,
    /// Windows opened with `openWindow`, in opening order.
    secondary: Vec<TestWindow>,
    cx: gpui::VisualTestAppContext,
}

/// An offscreen window painting the subtree under `root`.
struct TestWindow {
    root: u64,
    view: gpui::Entity<SoloView>,
    window: gpui::AnyWindowHandle,
}

thread_local! {
    static TEST_STATE: RefCell<Option<VisualTestState>> = const { RefCell::new(None) };
}

/// Access VisualTestAppContext + window + view mutably within thread_local.
/// The closure receives (&mut cx, window_handle, &view_entity).
/// Returns Err if no TestSoloRenderer has been created on this thread.
fn with_test_state<R>(
    f: impl FnOnce(
        &mut gpui::VisualTestAppContext,
        gpui::AnyWindowHandle,
        &gpui::Entity<SoloView>,
    ) -> Result<R>,
) -> Result<R> {
    TEST_STATE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let state = borrow
            .as_mut()
            .ok_or_else(|| Error::from_reason("TestSoloRenderer not initialized"))?;
        f(&mut state.cx, state.window, &state.view)
    })
}

/// Like `with_test_state`, on the window that paints `root`: an `openWindow`
/// window for `Some`, the main window for `None`.
fn with_window<R>(
    root: Option<u64>,
    f: impl FnOnce(
        &mut gpui::VisualTestAppContext,
        gpui::AnyWindowHandle,
        &gpui::Entity<SoloView>,
    ) -> Result<R>,
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

/// Convert JS button number (0=left, 1=middle, 2=right) to GPUI MouseButton.
fn u32_to_mouse_button(button: u32) -> gpui::MouseButton {
    match button {
        1 => gpui::MouseButton::Middle,
        2 => gpui::MouseButton::Right,
        _ => gpui::MouseButton::Left,
    }
}

fn validate_display_count(display_count: usize) -> Result<()> {
    if display_count == 0 {
        return Err(Error::from_reason(
            "TestSoloRenderer requires an interactive macOS display; NSScreen reported no available displays",
        ));
    }

    Ok(())
}

fn ensure_macos_display_available() -> Result<()> {
    let mtm = MainThreadMarker::new().ok_or_else(|| {
        Error::from_reason("TestSoloRenderer must be created on the macOS main thread")
    })?;

    validate_display_count(NSScreen::screens(mtm).len())
}

// ── TestSoloRenderer ────────────────────────────────────────────────

/// GPU-backed GPUI test renderer. Uses VisualTestAppContext (real Metal
/// rendering on macOS) with TestDispatcher for deterministic scheduling.
/// Same SoloView and rendering pipeline as production.
///
/// Usage from JS:
///   const r = new TestSoloRenderer()
///   r.createElement(1, "div")
///   r.setRoot(1)
///   r.commitMutations()
///   r.flush()                  // triggers SoloView::render() via Metal
///   r.simulateClick(50, 50)    // dispatches through GPUI hit testing
///   const events = r.drainEvents()
///   r.captureScreenshot("/tmp/test.png")  // saves rendered UI as PNG
#[napi]
pub struct TestSoloRenderer {
    tree: Arc<Mutex<RetainedTree>>,
    events: Arc<Mutex<Vec<EventPayload>>>,
    file_dialog_state: Arc<Mutex<crate::file_dialogs::DialogState>>,
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
        ensure_macos_display_available()?;

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

        // Create VisualTestAppContext with real macOS Metal rendering +
        // TestDispatcher for deterministic scheduling.
        let mac_platform = gpui_macos::MacPlatform::new(false);
        let mut cx = gpui::VisualTestAppContext::new(Rc::new(mac_platform));
        cx.update(|cx| {
            crate::renderer::init_key_bindings(cx);
            crate::custom_elements::input::init(cx);
        });

        // Open an offscreen window at (-10000, -10000) — invisible but fully
        // rendered by Metal. Uses the same SoloView as production.
        let window_handle = cx
            .open_offscreen_window_default(|_window, app| {
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

        // Store !Send types on the JS main thread.
        TEST_STATE.with(|cell| {
            *cell.borrow_mut() = Some(VisualTestState {
                cx,
                window,
                view,
                secondary: Vec::new(),
            });
        });

        Ok(Self {
            tree,
            events,
            file_dialog_state: Arc::new(Mutex::new(Default::default())),
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
        self.tree
            .lock()
            .unwrap()
            .add_window_root(root)
            .map_err(Error::from_reason)?;
        let opened = with_test_state(|cx, _window, view| {
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
            let handle = cx
                .open_offscreen_window(size, |_window, app| {
                    app.new(|_cx| {
                        SoloView::new(tree, callback, title, selection)
                            .for_window_root(root, Some(clock))
                    })
                })
                .map_err(|e| Error::from_reason(format!("Failed to open test window: {}", e)))?;
            let view = handle
                .entity(cx)
                .map_err(|e| Error::from_reason(format!("Failed to get root view: {}", e)))?;
            cx.run_until_parked();
            Ok(TestWindow {
                root,
                view,
                window: handle.into(),
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

    #[napi]
    pub fn create_element(&self, id: f64, element_type: String) -> Result<()> {
        let id = to_element_id(id)?;
        self.tree.lock().unwrap().create_element(id, element_type);
        Ok(())
    }

    /// Destroy an element and all descendants. Returns destroyed IDs
    /// so JS can clean up event handlers.
    #[napi]
    pub fn destroy_element(&self, id: f64) -> Result<Vec<f64>> {
        let id = to_element_id(id)?;
        let destroyed = self.tree.lock().unwrap().destroy_element(id);
        Ok(destroyed.iter().map(|&id| id as f64).collect())
    }

    #[napi]
    pub fn append_child(&self, parent_id: f64, child_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        self.tree.lock().unwrap().append_child(parent_id, child_id);
        Ok(())
    }

    #[napi]
    pub fn remove_child(&self, parent_id: f64, child_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        self.tree.lock().unwrap().remove_child(parent_id, child_id);
        Ok(())
    }

    #[napi]
    pub fn insert_before(&self, parent_id: f64, child_id: f64, before_id: f64) -> Result<()> {
        let parent_id = to_element_id(parent_id)?;
        let child_id = to_element_id(child_id)?;
        let before_id = to_element_id(before_id)?;
        self.tree
            .lock()
            .unwrap()
            .insert_before(parent_id, child_id, before_id);
        Ok(())
    }

    #[napi]
    pub fn set_style(&self, id: f64, style_json: String) -> Result<()> {
        let id = to_element_id(id)?;
        let style: StyleDesc = serde_json::from_str(&style_json)
            .map_err(|e| Error::from_reason(format!("Failed to parse style: {}", e)))?;
        self.tree.lock().unwrap().set_style(id, style);
        Ok(())
    }

    #[napi]
    pub fn set_text(&self, id: f64, content: String) -> Result<()> {
        let id = to_element_id(id)?;
        self.tree.lock().unwrap().set_text(id, content);
        Ok(())
    }

    #[napi]
    pub fn set_event_listener(&self, id: f64, event_type: String, has_handler: bool) -> Result<()> {
        let id = to_element_id(id)?;
        self.tree
            .lock()
            .unwrap()
            .set_event_listener(id, event_type, has_handler);
        Ok(())
    }

    /// Set the root element (called from appendChildToContainer).
    #[napi]
    pub fn set_root(&self, id: f64) -> Result<()> {
        let id = to_element_id(id)?;
        self.tree.lock().unwrap().root_id = Some(id);
        Ok(())
    }

    /// Set a custom prop on an element (for non-div/text elements like input, editor, diff).
    #[napi]
    pub fn set_custom_prop(&self, id: f64, key: String, value_json: String) -> Result<()> {
        let id = to_element_id(id)?;
        let value: serde_json::Value = serde_json::from_str(&value_json)
            .map_err(|e| Error::from_reason(format!("Failed to parse custom prop value: {}", e)))?;
        self.tree.lock().unwrap().set_custom_prop(id, key, value);
        Ok(())
    }

    /// Get a custom prop value from an element.
    #[napi]
    pub fn get_custom_prop(&self, id: f64, key: String) -> Result<Option<String>> {
        let id = to_element_id(id)?;
        let tree = self.tree.lock().unwrap();
        Ok(tree
            .get_custom_prop(id, &key)
            .map(|v| serde_json::to_string(v).unwrap_or_default()))
    }

    /// Present a lifecycle-managed native menu for an element in this renderer.
    #[napi]
    pub fn show_context_menu(
        &self,
        json: String,
    ) -> Result<AsyncTask<crate::context_menu::ContextMenuTask>> {
        self.flush()?;
        // The menu opens in the window of the element it belongs to.
        let owner = serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|value| value["elementId"].as_f64());
        with_window(self.window_root_of(owner)?, |cx, window, _view| {
            cx.update_window(window, |_, window, _app| {
                crate::context_menu::show(&json, &self.tree.lock().unwrap(), window)
            })
            .map_err(|error| Error::from_reason(error.to_string()))?
        })
        .map(AsyncTask::new)
    }

    #[napi]
    pub fn cancel_context_menu(&self, request_id: f64) -> Result<()> {
        crate::context_menu::cancel(to_element_id(request_id)?);
        Ok(())
    }

    /// Evaluate JavaScript in the mounted WebView through the native bridge.
    #[napi(js_name = "evaluateWebviewJavaScript")]
    pub fn evaluate_webview_javascript(
        &self,
        element_id: f64,
        script: String,
    ) -> Result<AsyncTask<crate::native_view::webview::EvaluateJavaScriptTask>> {
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
        Ok(crate::native_view::webview::EvaluateJavaScriptTask::new(receiver))
    }

    #[napi]
    pub fn wait_webview_ready(
        &self,
        element_id: f64,
    ) -> Result<AsyncTask<crate::native_view::webview::WebviewReadyTask>> {
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
        Ok(crate::native_view::webview::WebviewReadyTask::new(receiver))
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
        let ops: Vec<serde_json::Value> = serde_json::from_str(&json)
            .map_err(|e| Error::from_reason(format!("Failed to parse batch: {}", e)))?;
        let mut tree = self.tree.lock().unwrap();
        apply_batch_to_tree(&mut tree, &ops)
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
        let button = u32_to_mouse_button(button.unwrap_or(0));
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let position = gpui::point(gpui::px(x as f32), gpui::px(y as f32));
            cx.simulate_mouse_down(window, position, button, gpui::Modifiers::default());
            cx.simulate_mouse_up(window, position, button, gpui::Modifiers::default());
            Ok(())
        })
    }

    /// Simulate key strokes through GPUI's input pipeline.
    /// Format: space-separated keys, e.g. "a", "enter", "cmd-shift-p".
    /// The focused element receives keyDown/keyUp events.
    #[napi]
    pub fn simulate_keystrokes(&self, keystrokes: String, element_id: Option<f64>) -> Result<()> {
        let keys = crate::automation::parse_keystrokes(&keystrokes).map_err(Error::from_reason)?;
        if crate::context_menu::dispatch_keys(&keys) {
            return Ok(());
        }
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            cx.simulate_keystrokes(window, &keystrokes);
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
        if crate::context_menu::dispatch_keys(std::slice::from_ref(&key)) {
            return Ok(());
        }
        with_window(self.window_root_of(element_id)?, |cx, window, _view| {
            let parsed = gpui::Keystroke::parse(&keystroke).map_err(|e| {
                Error::from_reason(format!("Invalid keystroke '{}': {}", keystroke, e))
            })?;

            cx.simulate_event(
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

            cx.simulate_event(window, gpui::KeyUpEvent { keystroke: parsed });

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
            let button: Option<gpui::MouseButton> = pressed_button.map(u32_to_mouse_button);

            cx.simulate_mouse_move(
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                button,
                gpui::Modifiers::default(),
            );

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

        with_window(root, |cx, window, view| {
            let view = view.clone();

            cx.update_window(window, |_, window, app| {
                view.update(app, |view, cx| {
                    view.reveal_virtual_list_ancestor(id);
                    if let Some(handle) = view.focus_handles.get(&id) {
                        handle.focus(window, cx);
                    }
                    cx.notify();
                });
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;

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
            cx.simulate_mouse_down(
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                u32_to_mouse_button(button.unwrap_or(0)),
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
            cx.simulate_mouse_up(
                window,
                gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                u32_to_mouse_button(button.unwrap_or(0)),
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
            cx.simulate_event(
                window,
                gpui::ScrollWheelEvent {
                    position: gpui::point(gpui::px(x as f32), gpui::px(y as f32)),
                    delta: gpui::ScrollDelta::Pixels(gpui::point(
                        gpui::px(delta_x as f32),
                        gpui::px(delta_y as f32),
                    )),
                    modifiers: gpui::Modifiers::default(),
                    touch_phase: gpui::TouchPhase::Moved,
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
        with_window(self.window_root_of(Some(element_id))?, |cx, window, view| {
            let view = view.clone();
            cx.update_window(window, |_, _window, app| {
                view.update(app, |view, _cx| {
                    if view.set_virtual_list_offset(id, x as f32, y as f32) {
                        return;
                    }
                    if let Some(handle) = view.scroll_handles.get(&id) {
                        handle.set_offset(gpui::point(gpui::px(x as f32), gpui::px(y as f32)));
                    }
                });
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;
            Ok(())
        })
    }

    /// Scroll a child into view by its index in the children list.
    /// Call flush() after to apply and re-render.
    #[napi]
    pub fn scroll_to_item(&self, element_id: f64, index: f64) -> Result<()> {
        let id = to_element_id(element_id)?;
        let index = index as usize;
        with_window(self.window_root_of(Some(element_id))?, |cx, window, view| {
            let view = view.clone();
            cx.update_window(window, |_, _window, app| {
                view.update(app, |view, _cx| {
                    if view.scroll_virtual_list_to_item(id, index) {
                        return;
                    }
                    if let Some(handle) = view.scroll_handles.get(&id) {
                        if !crate::renderer::scroll_retained_child_into_view(
                            &view.tree,
                            handle,
                            id,
                            index,
                        ) {
                            handle.scroll_to_item(index);
                        }
                    }
                });
            })
            .map_err(|e| Error::from_reason(e.to_string()))?;
            Ok(())
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
        with_window(self.window_root_of(Some(element_id))?, |cx, window, view| {
            let view = view.clone();
            let result = cx
                .update_window(window, |_, _window, app| {
                    view.update(app, |view, _cx| {
                        if let Some(offset) = view.virtual_list_offset(id) {
                            return Some(offset.to_vec());
                        }
                        view.scroll_handles.get(&id).map(|handle| {
                            let offset = handle.offset();
                            vec![
                                f64::from(f32::from(offset.x)),
                                f64::from(f32::from(offset.y)),
                            ]
                        })
                    })
                })
                .map_err(|e| Error::from_reason(e.to_string()))?;
            Ok(result)
        })
    }

    /// Capture a screenshot of the current rendered state and save as PNG.
    /// macOS only — requires Metal GPU rendering via VisualTestAppContext.
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
        let tree = self.tree.lock().unwrap();
        let mut texts = Vec::new();
        if let Some(root_id) = tree.root_id {
            Self::collect_text(root_id, &tree, &mut texts);
        }
        texts
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
        Ok(tree.elements.get(&id).and_then(|e| e.content.clone()))
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
        with_test_state(|cx, window, view| {
            let view = view.clone();
            let now_ms = cx
                .update_window(window, |_, _window, app| {
                    view.update(app, |view, cx| {
                        let now_ms = view.clock.pause();
                        cx.notify();
                        now_ms
                    })
                })
                .map_err(|e| Error::from_reason(e.to_string()))?;
            cx.run_until_parked();
            Ok(now_ms)
        })
    }

    #[napi]
    pub fn clock_set(&self, now_ms: f64) -> Result<f64> {
        with_test_state(|cx, window, view| {
            let view = view.clone();
            let now_ms = cx
                .update_window(window, |_, _window, app| {
                    view.update(app, |view, cx| {
                        let now_ms = view.clock.set_ms(now_ms);
                        cx.notify();
                        now_ms
                    })
                })
                .map_err(|e| Error::from_reason(e.to_string()))?;
            cx.run_until_parked();
            Ok(now_ms)
        })
    }

    #[napi]
    pub fn clock_fast_forward(&self, delta_ms: f64) -> Result<f64> {
        with_test_state(|cx, window, view| {
            let view = view.clone();
            let now_ms = cx
                .update_window(window, |_, _window, app| {
                    view.update(app, |view, cx| {
                        let now_ms = view.clock.fast_forward_ms(delta_ms);
                        cx.notify();
                        now_ms
                    })
                })
                .map_err(|e| Error::from_reason(e.to_string()))?;
            cx.run_until_parked();
            Ok(now_ms)
        })
    }

    #[napi]
    pub fn clock_resume(&self) -> Result<f64> {
        with_test_state(|cx, window, view| {
            let view = view.clone();
            let now_ms = cx
                .update_window(window, |_, _window, app| {
                    view.update(app, |view, cx| {
                        let now_ms = view.clock.resume();
                        cx.notify();
                        now_ms
                    })
                })
                .map_err(|e| Error::from_reason(e.to_string()))?;
            cx.run_until_parked();
            Ok(now_ms)
        })
    }

    /// Get the root element ID, or null if no root is set.
    #[napi]
    pub fn get_root_id(&self) -> Option<f64> {
        self.tree.lock().unwrap().root_id.map(|id| id as f64)
    }

    // ── Private helpers ──────────────────────────────────────────────

    fn collect_text(id: u64, tree: &RetainedTree, texts: &mut Vec<String>) {
        if let Some(element) = tree.elements.get(&id) {
            if let Some(ref content) = element.content {
                texts.push(content.clone());
            }
            for &child_id in &element.children {
                Self::collect_text(child_id, tree, texts);
            }
        }
    }
}

#[cfg(test)]
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
