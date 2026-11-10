//! Linux WebKitGTK host for Solo's `<webview>` element (X11 only).
//!
//! A WebKitWebView is a GTK widget, not a GPUI element: it cannot be painted
//! into GPUI's scene, so it lives in its own X window, reparented below
//! GPUI's X11 window and composited *above* GPUI's surface. This mirrors the
//! macOS WKWebView bridge: GPUI owns layout, WebKit owns rendering and input.
//!
//! GTK cannot be touched from Solo's `solo-ui` thread. One dedicated
//! "webview" thread owns GTK (`gtk::init` + `gtk::main`) and every
//! `WebKitWebView`; the [`LinuxWebView`] instance the registry holds only
//! posts commands through a GLib main-context channel. Event callbacks are
//! `EventCallback` (`Send + Sync`), so WebKit signals can emit straight from
//! signal handlers on that thread. Reply channels for JS evaluation and
//! readiness are std oneshots, resolved on the webview thread.
//!
//! ```text
//! <webview url=…>
//!   → WebviewElement (CustomElement: props, events, destroy)
//!     → WebviewPlaceholder (gpui::Element: layout + bounds capture)
//!       → prepaint(bounds) -- scale_factor --> physical rect
//!         → registry.update_frame(element_id, frame)
//!           → Frame → XMoveResizeWindow on the container child
//! ```
//!
//! Everything X is done through the display GTK opened on the webview thread:
//! the container child is created with `XCreateSimpleWindow` under the parent
//! XID (XIDs are server-wide), then wrapped by a GTK window through
//! `gdk_x11_window_foreign_new_for_display` — the same trick wry uses for
//! `build_as_child` on X11. Geometry therefore never enters GTK coordinates:
//! GPUI's bounds, scaled to physical pixels by the placeholder, are the
//! container's position and size directly.
//!
//! Wayland cannot embed another toolkit's surface, so a Wayland window never
//! reaches `mount`: the placeholder reports the clear error instead (see
//! `custom_elements/webview.rs`).
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::OnceLock;

use anyhow::Result;
use gtk::glib;
use gtk::glib::translate::FromGlibPtrFull;
use gtk::prelude::*;
use webkit2gtk::{
    LoadEvent, NavigationPolicyDecisionExt, NavigationType, PolicyDecision, PolicyDecisionExt,
    Settings, SettingsExt, URIRequestExt, UserContentManagerExt, WebView, WebViewExt,
};
use x11_dl::xlib::Xlib;

use super::{EvaluateReceiver, NativeViewFrame, NativeViewInstance, ReadyReceiver};
use crate::renderer::{EventCallback, emit_event_full};

/// Must match the native event types in `EVENT_PROPS`
/// (`packages/core/src/events.ts`).
const EVENT_NAVIGATION: &str = "navigation";
const EVENT_NAVIGATION_REQUEST: &str = "navigationRequest";
const EVENT_LOAD: &str = "load";
const EVENT_LOAD_ERROR: &str = "loadError";

/// Property keys accepted by [`LinuxWebView::set_content`].
const PROP_URL: &str = "url";
const PROP_HTML: &str = "html";
const PROP_BASE_URL: &str = "baseUrl";
const PROP_USER_AGENT: &str = "userAgent";

/// The isolated JavaScript world evaluations run in and the message handler
/// listens in. Page scripts can read and post to `window.webkit.messageHandlers`
/// in their own world, so a page could otherwise settle the app's pending
/// evaluation (or receive its result) with a value of its choosing.
const EVAL_WORLD: &str = "solo-eval";

/// Wrap an expression so WebKitGTK returns its JSON without needing
/// top-level await. There is no sync equivalent of macOS's top-level
/// `await`, and `run_javascript` will not unwrap a Promise ("Unsupported
/// result type"), so the result is delivered through a registered script
/// message instead: `{ id, nonce, ok, value }` as JSON text.
///
/// The envelope carries the evaluation id and a per-view nonce because
/// results arrive in completion order, not submission order, and the handler
/// must match a result to the call that produced it.
fn evaluation_script(script: &str, id: u64, nonce: &str) -> String {
    format!(
        "(async () => {{ try {{ const value = await ({script}); if (value === undefined) return \"null\"; const seen = new WeakSet(); return JSON.stringify(value, (_key, item) => {{ if (typeof item === \"number\" && !Number.isFinite(item)) throw new Error(\"unsupported non-finite number\"); if (typeof item === \"undefined\" || typeof item === \"function\" || typeof item === \"symbol\" || typeof item === \"bigint\") throw new Error(\"unsupported JavaScript result\"); if (item !== null && typeof item === \"object\") {{ if (seen.has(item)) throw new Error(\"unsupported cyclic result\"); seen.add(item); }} return item; }}); }} catch (error) {{ throw error; }} }})().then((value) => window.webkit.messageHandlers.soloEval.postMessage(JSON.stringify({{ id: {id}, nonce: \"{nonce}\", ok: true, value }})), (error) => window.webkit.messageHandlers.soloEval.postMessage(JSON.stringify({{ id: {id}, nonce: \"{nonce}\", ok: false, value: String(error && error.message ? error.message : error) }})));"
    )
}

pub(crate) type EvaluateSender = futures::channel::oneshot::Sender<Result<String>>;
pub(crate) type ReadySender = futures::channel::oneshot::Sender<Result<u64>>;

/// An evaluation that can be cancelled by a new document and resolved by the
/// WebKit callback. The shared cell lets `begin_document` take the sender
/// away while `run_javascript`'s callback holds the only other owner.
type PendingEvaluation = Rc<RefCell<Option<EvaluateSender>>>;

/// Commands from the UI thread to the webview thread. The channel is
/// ordered, so a Mount sent before the first frame or load is applied first.
enum Command {
    Mount {
        element_id: u64,
        /// GPUI's X11 window, as an XID. XIDs are server-wide, so the
        /// webview thread can parent a child under it on its own display.
        parent: u64,
        callback: Option<EventCallback>,
    },
    Frame {
        element_id: u64,
        frame: NativeViewFrame,
    },
    LoadUrl {
        element_id: u64,
        url: String,
    },
    LoadHtml {
        element_id: u64,
        html: String,
        base_url: Option<String>,
    },
    UserAgent {
        element_id: u64,
        user_agent: Option<String>,
    },
    Intercept {
        element_id: u64,
        enabled: bool,
    },
    Evaluate {
        element_id: u64,
        script: String,
        reply: EvaluateSender,
    },
    WaitReady {
        element_id: u64,
        reply: ReadySender,
    },
    Decide {
        element_id: u64,
        navigation_id: u64,
        allow: bool,
    },
    Destroy {
        element_id: u64,
    },
}

fn service_sender() -> async_channel::Sender<Command> {
    static SENDER: OnceLock<async_channel::Sender<Command>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (sender, receiver) = async_channel::bounded(1024);
            std::thread::Builder::new()
                .name("solo-webview".to_string())
                .spawn(move || {
                    if gtk::init().is_err() {
                        log::error!(
                            "webview: gtk::init failed; <webview> cannot be created on this session"
                        );
                        return;
                    }
                    let service = Rc::new(RefCell::new(Service::default()));
                    glib::MainContext::ref_thread_default().spawn_local(async move {
                        while let Ok(command) = receiver.recv().await {
                            process(&service, command);
                        }
                    });
                    // The sender lives in the OnceLock; the service and its
                    // views live exactly as long as the loop. Nothing here is
                    // ever restarted.
                    gtk::main();
                })
                .expect("failed to spawn the webview thread");
            sender
        })
        .clone()
}

/// One mounted `<webview>`, as seen from the UI thread.
///
/// It owns no GTK objects: everything lives in `ViewState` on the webview
/// thread. This struct only keeps the per-frame diff state (the placeholder
/// re-sends url/html/UA every frame) and the mount flag, exactly like the
/// macOS bridge's `MacWebView`.
pub struct LinuxWebView {
    element_id: u64,
    callback: Option<EventCallback>,
    sender: async_channel::Sender<Command>,
    mounted: bool,
    last_frame: Option<NativeViewFrame>,
    last_url: Option<String>,
    last_html: Option<String>,
    last_base_url: Option<String>,
    last_user_agent: Option<String>,
}

impl LinuxWebView {
    pub fn new(element_id: u64, callback: Option<EventCallback>) -> Self {
        Self {
            element_id,
            callback,
            sender: service_sender(),
            mounted: false,
            last_frame: None,
            last_url: None,
            last_html: None,
            last_base_url: None,
            last_user_agent: None,
        }
    }

    fn send(&self, command: Command) {
        // The service thread outlives the process's windows; a failed send
        // means the webview thread is gone and there is nothing to do.
        // Fire-and-forget, back-pressured by the bounded channel; a
        // closed channel means the webview thread is gone.
        let _ = self.sender.try_send(command);
    }

    fn emit(&self, event_type: &str, value: Option<String>) {
        let value = value.unwrap_or_default();
        emit_event_full(&self.callback, self.element_id, event_type, move |payload| {
            payload.value = Some(value);
        });
    }

    /// Keep url/html/baseUrl in step with the props, without reloading on
    /// every frame. `set_content` runs on each `prepaint`, so the value is
    /// only forwarded when it actually changed — including to `None`.
    fn sync_source(&mut self, key: &str, value: Option<&str>) {
        match key {
            PROP_URL => {
                if self.last_url.as_deref() == value {
                    return;
                }
                if value.is_some() && self.last_html.is_some() {
                    self.emit(
                        EVENT_LOAD_ERROR,
                        Some("WebView url and html are mutually exclusive".to_string()),
                    );
                    return;
                }
                self.last_url = value.map(str::to_string);
            }
            PROP_HTML => {
                if self.last_html.as_deref() == value {
                    return;
                }
                if value.is_some() && self.last_url.is_some() {
                    self.emit(
                        EVENT_LOAD_ERROR,
                        Some("WebView url and html are mutually exclusive".to_string()),
                    );
                    return;
                }
                self.last_html = value.map(str::to_string);
            }
            PROP_BASE_URL => {
                if self.last_base_url.as_deref() == value {
                    return;
                }
                if value.is_some() && self.last_url.is_some() {
                    self.emit(
                        EVENT_LOAD_ERROR,
                        Some(
                            "WebView baseUrl requires html and cannot be used with url".to_string(),
                        ),
                    );
                    return;
                }
                self.last_base_url = value.map(str::to_string);
            }
            _ => return,
        }
        if !self.mounted {
            return;
        }
        if let Some(url) = &self.last_url {
            self.send(Command::LoadUrl {
                element_id: self.element_id,
                url: url.clone(),
            });
        } else if let Some(html) = &self.last_html {
            self.send(Command::LoadHtml {
                element_id: self.element_id,
                html: html.clone(),
                base_url: self.last_base_url.clone(),
            });
        } else if key == PROP_HTML || key == PROP_BASE_URL {
            // Clearing the source loads a blank page, as on macOS.
            self.send(Command::LoadHtml {
                element_id: self.element_id,
                html: String::new(),
                base_url: self.last_base_url.clone(),
            });
        }
    }
}

/// The UI-thread half of the `NativeViewInstance` contract. Every method runs
/// on GPUI's thread (the solo-ui thread) inside `prepaint`; construction is
/// cheap and the first `mount` is what starts GTK.
impl NativeViewInstance for LinuxWebView {
    fn mount(&mut self, host: *mut c_void) {
        if self.mounted || host.is_null() {
            return;
        }
        self.mounted = true;
        // On Linux the host pointer carries the parent X window ID (see
        // `custom_elements/webview.rs`): copy it out now, it is not a real
        // pointer with a lifetime to keep.
        let parent = host as usize as u64;
        self.send(Command::Mount {
            element_id: self.element_id,
            parent,
            callback: self.callback.clone(),
        });
        // Content set before the view existed still has to load.
        if let Some(url) = self.last_url.clone() {
            self.send(Command::LoadUrl {
                element_id: self.element_id,
                url,
            });
        } else if let Some(html) = self.last_html.clone() {
            self.send(Command::LoadHtml {
                element_id: self.element_id,
                html,
                base_url: self.last_base_url.clone(),
            });
        }
    }

    fn update_frame(&mut self, frame: NativeViewFrame) {
        if !self.mounted || self.last_frame == Some(frame) {
            return;
        }
        self.last_frame = Some(frame);
        self.send(Command::Frame {
            element_id: self.element_id,
            frame,
        });
    }

    fn set_content(&mut self, key: &str, value: Option<&str>) {
        match key {
            PROP_URL | PROP_HTML | PROP_BASE_URL => self.sync_source(key, value),
            PROP_USER_AGENT => {
                if self.last_user_agent.as_deref() == value {
                    return;
                }
                self.last_user_agent = value.map(str::to_string);
                if self.mounted {
                    self.send(Command::UserAgent {
                        element_id: self.element_id,
                        user_agent: self.last_user_agent.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    /// Apply a WebView's mutually exclusive URL/HTML source as one update, as
    /// on macOS: a frame must not flip from url to html halfway through the
    /// per-prop notifications.
    fn set_webview_source(
        &mut self,
        url: Option<&str>,
        html: Option<&str>,
        base_url: Option<&str>,
    ) {
        if url.is_some() && html.is_some() {
            self.emit(
                EVENT_LOAD_ERROR,
                Some("WebView url and html are mutually exclusive".to_string()),
            );
            return;
        }
        if base_url.is_some() && html.is_none() {
            self.emit(
                EVENT_LOAD_ERROR,
                Some("WebView baseUrl requires html".to_string()),
            );
            return;
        }
        if self.last_url.as_deref() == url
            && self.last_html.as_deref() == html
            && self.last_base_url.as_deref() == base_url
        {
            return;
        }
        self.last_url = url.map(str::to_string);
        self.last_html = html.map(str::to_string);
        self.last_base_url = base_url.map(str::to_string);
        if !self.mounted {
            return;
        }
        if let Some(html) = &self.last_html {
            self.send(Command::LoadHtml {
                element_id: self.element_id,
                html: html.clone(),
                base_url: self.last_base_url.clone(),
            });
        } else if let Some(url) = &self.last_url {
            self.send(Command::LoadUrl {
                element_id: self.element_id,
                url: url.clone(),
            });
        } else {
            // Clearing the source loads a blank page, as on macOS.
            self.send(Command::LoadHtml {
                element_id: self.element_id,
                html: String::new(),
                base_url: self.last_base_url.clone(),
            });
        }
    }

    fn evaluate_javascript(&mut self, script: &str) -> Result<EvaluateReceiver> {
        if !self.mounted {
            return Err(anyhow::anyhow!("WebView {} is not mounted", self.element_id));
        }
        let (reply, receiver) = futures::channel::oneshot::channel();
        self.send(Command::Evaluate {
            element_id: self.element_id,
            script: script.to_string(),
            reply,
        });
        Ok(receiver)
    }

    fn wait_for_ready(&mut self) -> Result<ReadyReceiver> {
        if !self.mounted {
            return Err(anyhow::anyhow!("WebView {} is not mounted", self.element_id));
        }
        let (reply, receiver) = futures::channel::oneshot::channel();
        self.send(Command::WaitReady {
            element_id: self.element_id,
            reply,
        });
        Ok(receiver)
    }

    fn set_navigation_interception(&mut self, enabled: bool) {
        if self.mounted {
            self.send(Command::Intercept {
                element_id: self.element_id,
                enabled,
            });
        }
    }

    fn decide_navigation(&mut self, navigation_id: u64, allow: bool) -> bool {
        // The `PolicyDecision` lives on the webview thread, so whether the
        // request is still pending is only known there. This callback cannot
        // wait for a reply (it runs on the UI thread), so it is
        // fire-and-forget; an already-decided request is dropped on the
        // webview thread. (macOS resolves synchronously and reports whether
        // the request was still pending.)
        if self.mounted {
            self.send(Command::Decide {
                element_id: self.element_id,
                navigation_id,
                allow,
            });
        }
        true
    }

    fn destroy(&mut self) {
        if !self.mounted {
            self.callback = None;
            return;
        }
        self.mounted = false;
        self.send(Command::Destroy {
            element_id: self.element_id,
        });
        self.callback = None;
        self.last_frame = None;
        self.last_url = None;
        self.last_html = None;
        self.last_base_url = None;
        self.last_user_agent = None;
    }
}

// ── The webview thread ────────────────────────────────────────────────────

#[derive(Default)]
struct Service {
    views: HashMap<u64, ViewState>,
}

/// GTK/WebKit state the UI thread never sees.
struct ViewState {
    element_id: u64,
    callback: Option<EventCallback>,
    webview: WebView,
    settings: Settings,
    gtk_window: gtk::Window,
    xlib: Xlib,
    display: *mut c_void,
    /// GDK's display, for error traps around X calls (GTK treats untrapped X
    /// errors as fatal).
    gdk_display: *mut gdkx11::ffi::GdkX11Display,
    container: u64,
    last_frame: Option<NativeViewFrame>,
    visible: bool,
    intercept: bool,
    /// Decisions held for JS-side `allowNavigation`/`cancelNavigation`.
    pending: HashMap<u64, PolicyDecision>,
    next_navigation_id: u64,
    evaluations: std::collections::BTreeMap<u64, PendingEvaluation>,
    next_evaluation_id: u64,
    /// Random per-view token that a page cannot guess, checked against every
    /// `soloEval` envelope.
    nonce: String,
    generation: u64,
    ready: bool,
    waiters: HashMap<u64, Vec<ReadySender>>,
}

type SharedService = Rc<RefCell<Service>>;

/// Dispatch one command. Runs only on the webview thread: from the main
/// context channel, or from a synthetic WebKit signal via the shared
/// service. The `Rc` is how signal handlers reborrow the service that the
/// channel callback drives.
fn process(service: &SharedService, command: Command) {
    match command {
        Command::Mount {
            element_id,
            parent,
            callback,
        } => service.borrow_mut().mount(element_id, parent, callback, service),
        Command::Frame { element_id, frame } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                view.apply_frame(frame);
            }
        }
        Command::LoadUrl { element_id, url } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                view.load_url(&url);
            }
        }
        Command::LoadHtml {
            element_id,
            html,
            base_url,
        } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                view.begin_document();
                match base_url.as_deref() {
                    Some(base) => view.webview.load_html(&html, Some(base)),
                    None => view.webview.load_html(&html, None),
                }
            }
        }
        Command::UserAgent {
            element_id,
            user_agent,
        } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                view.settings.set_user_agent(user_agent.as_deref());
            }
        }
        Command::Intercept {
            element_id,
            enabled,
        } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                view.intercept = enabled;
                if !enabled {
                    view.cancel_pending();
                }
            }
        }
        Command::Evaluate {
            element_id,
            script,
            reply,
        } => {
            let mut service = service.borrow_mut();
            let Some(view) = service.views.get_mut(&element_id) else {
                let _ =
                    reply.send(Err(anyhow::anyhow!("WebView {element_id} is not mounted")));
                return;
            };
            let id = view.next_evaluation_id;
            view.next_evaluation_id = view.next_evaluation_id.wrapping_add(1).max(1);
            let pending: PendingEvaluation = Rc::new(RefCell::new(Some(reply)));
            view.evaluations.insert(id, pending.clone());
            let webview = view.webview.clone();
            let body = evaluation_script(&script, id, &view.nonce);
            drop(service);
            #[allow(deprecated)] // The callback never carries the value; the
            // result is delivered through `soloEval` script messages. The
            // wrapper runs in its own world so page scripts cannot see or
            // fake the handler.
            webview.run_javascript_in_world(
                &body,
                EVAL_WORLD,
                gtk::gio::Cancellable::NONE,
                move |result| {
                    // The wrapper always returns a Promise, so `Ok` never
                    // carries the value here and a rejection is expected
                    // ("Unsupported result type") rather than a failure. A
                    // syntax error, on the other hand, means the script
                    // never runs: resolve the pending evaluation now instead
                    // of leaving it until the next document change.
                    // The wrapper always returns a Promise, so `Ok` never
                    // carries the value here and a rejection is expected
                    // ("Unsupported result type") rather than a failure. A
                    // syntax error, on the other hand, means the script
                    // never runs: resolve the pending evaluation now instead
                    // of leaving it until the next document change.
                    if let Err(error) = result {
                        if !error.to_string().contains("Unsupported result type") {
                            if let Some(sender) = pending.borrow_mut().take() {
                                let _ = sender.send(Err(anyhow::anyhow!(error.to_string())));
                            }
                        }
                    }
                },
            );
        }
        Command::WaitReady {
            element_id,
            reply,
        } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                if view.ready {
                    let _ = reply.send(Ok(view.generation));
                } else {
                    view.waiters
                        .entry(view.generation)
                        .or_default()
                        .push(reply);
                }
            } else {
                let _ =
                    reply.send(Err(anyhow::anyhow!("WebView {element_id} is not mounted")));
            }
        }
        Command::Decide {
            element_id,
            navigation_id,
            allow,
        } => {
            if let Some(view) = service.borrow_mut().views.get_mut(&element_id) {
                if let Some(decision) = view.pending.remove(&navigation_id) {
                    // macOS starts the document on allow; on cancel the
                    // document never starts, so the generation must not move.
                    if allow {
                        view.begin_document();
                        decision.use_();
                    } else {
                        decision.ignore();
                    }
                }
            }
        }
        Command::Destroy { element_id } => {
            if let Some(mut view) = service.borrow_mut().views.remove(&element_id) {
                view.destroy();
            }
        }
    }
}

impl Service {
    fn mount(
        &mut self,
        element_id: u64,
        parent: u64,
        callback: Option<EventCallback>,
        service: &SharedService,
    ) {
        if self.views.contains_key(&element_id) {
            return;
        }
        let Some(display) = x11_display() else {
            log::error!(
                "webview: no X11 display; <webview> needs X11 (a Wayland window cannot embed it)"
            );
            return;
        };
        let Some(xlib) = Xlib::open().ok() else {
            log::error!("webview: Xlib could not be loaded");
            return;
        };
        let Some(gdk_display) = gdk_x11_display() else {
            log::error!("webview: the session display is not X11");
            return;
        };
        let container = create_container(&xlib, display, parent);
        let gtk_window = create_gtk_window(container);
        let webview = WebView::new();
        let settings = Settings::new();
        webview.set_settings(&settings);
        gtk_window.add(&webview);
        gtk_window.show_all();
        let view = ViewState {
            element_id,
            callback: callback.clone(),
            webview: webview.clone(),
            settings,
            gtk_window,
            xlib,
            display,
            gdk_display,
            container,
            last_frame: None,
            visible: false,
            intercept: false,
            pending: HashMap::new(),
            next_navigation_id: 1,
            evaluations: std::collections::BTreeMap::new(),
            next_evaluation_id: 1,
            nonce: new_nonce(),
            generation: 1,
            ready: true,
            waiters: HashMap::new(),
        };
        self.views.insert(element_id, view);
        connect_signals(&webview, element_id, callback, service);
    }

    fn on_load_changed(&mut self, element_id: u64, event: LoadEvent) {
        let Some(view) = self.views.get_mut(&element_id) else {
            return;
        };
        match event {
            LoadEvent::Started => {
                // WebKitGTK reports `Started` asynchronously, after the load
                // request reaches the web process. A controlled load already
                // began its generation (and rejected the previous document's
                // waiters) when Solo called load_url/load_html, so bumping it
                // again here would reject a `ready()` issued right after the
                // source change. Only a navigation Solo did not start — a
                // link click — begins a document here. Redirects reuse the
                // in-flight generation the same way macOS's navigation
                // identity does.
                if view.ready {
                    view.begin_document();
                }
                view.emit(EVENT_NAVIGATION, view.current_url());
            }
            LoadEvent::Finished => {
                view.mark_ready();
                view.emit(EVENT_LOAD, view.current_url());
            }
            _ => {}
        }
    }

    fn on_load_failed(
        &mut self,
        element_id: u64,
        failing_url: String,
        error: String,
        cancelled: bool,
    ) {
        let Some(view) = self.views.get_mut(&element_id) else {
            return;
        };
        // Changing `url` cancels the load in flight; WebKit reports that as a
        // failure of the superseded load. It is not an error the app asked
        // about, and its waiters belong to the newer generation.
        if cancelled {
            return;
        }
        view.mark_failed(&error);
        view.emit(EVENT_LOAD_ERROR, Some(failing_url));
    }

    /// A `soloEval` script message: resolve the evaluation its envelope names.
    /// Results arrive in completion order, so matching by id (and checking the
    /// per-view nonce) is what keeps concurrent evaluations apart.
    fn on_eval_message(&mut self, element_id: u64, text: String) {
        let Some(envelope) = EvaluationEnvelope::parse(&text) else {
            log::warn!("webview: ignored a malformed evaluation result");
            return;
        };
        let pending = {
            let Some(view) = self.views.get_mut(&element_id) else {
                return;
            };
            if envelope.nonce != view.nonce {
                log::warn!("webview: ignored an evaluation result with a foreign nonce");
                return;
            }
            view.evaluations.remove(&envelope.id)
        };
        let Some(pending) = pending else {
            return;
        };
        let sender = pending.borrow_mut().take();
        if let Some(sender) = sender {
            let _ = sender.send(envelope.into_result());
        }
    }

    /// A `decide-policy` from WebKit: decide immediately when nothing
    /// intercepts, otherwise hold the decision and tell JS. Returns whether
    /// the signal was handled (the decision is ours either way).
    fn on_decide_policy(&mut self, element_id: u64, decision: PolicyDecision) -> bool {
        let Some(view) = self.views.get_mut(&element_id) else {
            return false;
        };
        let Some(navigation) = decision.downcast_ref::<webkit2gtk::NavigationPolicyDecision>()
        else {
            decision.use_();
            return true;
        };
        let Some(action) = navigation.navigation_action() else {
            decision.use_();
            return true;
        };
        let requested_url = action
            .request()
            .and_then(|request| request.uri().map(|uri| uri.to_string()));
        // These bindings expose no target-frame accessor, so a new-window
        // request cannot be told apart from a main-frame one. WebKitGTK
        // delivers those through `create-web-view`, which Solo does not
        // implement: treat every request as main-frame (`is_new_window`
        // stays false on Linux; macOS distinguishes it exactly).
        let new_window = false;
        let same = same_document(requested_url.as_deref(), view.current_url().as_deref());
        let link = action.navigation_type() == NavigationType::LinkClicked;
        if new_window {
            // Unreachable today (see above): macOS cancels new-window
            // requests automatically. Kept so the policy stays correct if
            // the bindings gain a target-frame accessor.
            decision.ignore();
            return true;
        }
        if !(view.intercept && link && !same) {
            decision.use_();
            return true;
        }
        let id = view.next_navigation_id;
        view.next_navigation_id = view.next_navigation_id.wrapping_add(1).max(1);
        view.pending.insert(id, decision);
        let callback = view.callback.clone();
        emit_event_full(&callback, element_id, EVENT_NAVIGATION_REQUEST, move |payload| {
            payload.navigation_id = Some(id as f64);
            payload.value = requested_url.clone();
            payload.navigation_url = requested_url;
            payload.is_same_document = Some(same);
            payload.is_new_window = Some(new_window);
        });
        true
    }
}

impl ViewState {
    fn current_url(&self) -> Option<String> {
        self.webview.uri().map(|uri| uri.to_string())
    }

    fn emit(&self, event_type: &str, value: Option<String>) {
        let value = value.unwrap_or_default();
        emit_event_full(&self.callback, self.element_id, event_type, move |payload| {
            payload.value = Some(value);
        });
    }

    /// A new document replaces the pending evaluations and readies, as on
    /// macOS: promises never wait on a page that no longer exists.
    fn begin_document(&mut self) {
        let pending: Vec<PendingEvaluation> = self.evaluations.values().cloned().collect();
        self.evaluations.clear();
        for sender in pending {
            if let Some(sender) = sender.borrow_mut().take() {
                let _ = sender.send(Err(anyhow::anyhow!("WebView document changed")));
            }
        }
        let waiters = std::mem::take(&mut self.waiters);
        for (_, senders) in waiters {
            for sender in senders {
                let _ = sender.send(Err(anyhow::anyhow!("WebView document changed")));
            }
        }
        self.generation = self.generation.wrapping_add(1).max(1);
        self.ready = false;
    }

    fn mark_ready(&mut self) {
        self.ready = true;
        if let Some(senders) = self.waiters.remove(&self.generation) {
            for sender in senders {
                let _ = sender.send(Ok(self.generation));
            }
        }
    }

    fn mark_failed(&mut self, reason: &str) {
        let waiters = std::mem::take(&mut self.waiters);
        for (_, senders) in waiters {
            for sender in senders {
                let _ = sender.send(Err(anyhow::anyhow!(reason.to_string())));
            }
        }
    }

    fn cancel_pending(&mut self) {
        let pending = std::mem::take(&mut self.pending);
        for (_, decision) in pending {
            decision.ignore();
        }
    }

    fn load_url(&mut self, url: &str) {
        if url.trim().is_empty() {
            return;
        }
        self.begin_document();
        self.webview.load_uri(url);
    }

    /// Apply a frame from GPUI layout. Frames are physical pixels (the
    /// placeholder scales them); the container window is a real X window, so
    /// geometry is a plain `XMoveResizeWindow`, no GTK coordinates involved.
    /// A zero-area view is unmapped instead of left mounted: WebKit keeps
    /// compositing layers alive for an empty frame.
    ///
    /// Resizing the X window alone is not enough: the `WebView` inside the
    /// foreign gdk window is laid out by GTK, and GTK never learns an
    /// external resize happened. So, as wry does, the gdk window is resized
    /// and the GtkWindow is re-allocated, which relayouts its children.
    fn apply_frame(&mut self, frame: NativeViewFrame) {
        if self.last_frame == Some(frame) {
            return;
        }
        self.last_frame = Some(frame);
        let renderable = frame.is_renderable();
        // The container may have gone with its parent window (a closed
        // secondary window destroys its children); GDK treats an untrapped X
        // error as fatal, so every X call runs under a trap.
        self.trap_errors();
        unsafe {
            if renderable {
                if !self.visible {
                    (self.xlib.XMapWindow)(self.display as _, self.container as _);
                }
                (self.xlib.XMoveResizeWindow)(
                    self.display as _,
                    self.container as _,
                    frame.x as i32,
                    frame.y as i32,
                    frame.width as u32,
                    frame.height as u32,
                );
                if let Some(gdk_window) = self.gtk_window.window() {
                    gdk_window.resize(frame.width as i32, frame.height as i32);
                }
                self.gtk_window
                    .size_allocate(&gtk::Allocation::new(
                        0,
                        0,
                        frame.width as i32,
                        frame.height as i32,
                    ));
            } else if self.visible {
                (self.xlib.XUnmapWindow)(self.display as _, self.container as _);
            }
        }
        self.untrap_errors();
        self.visible = renderable;
    }

    fn destroy(&mut self) {
        self.begin_document();
        self.mark_failed("WebView was destroyed");
        self.cancel_pending();
        // Destroy the GTK side first: it owns the foreign gdk window wrapped
        // around the container. JS disposes a tree on `windowClosed`, after
        // the X server already destroyed the children, so the X calls must
        // tolerate a gone window instead of aborting the process.
        self.gtk_window.close();
        self.trap_errors();
        unsafe {
            (self.xlib.XDestroyWindow)(self.display as _, self.container as _);
        }
        self.untrap_errors();
        self.callback = None;
    }

    fn trap_errors(&self) {
        unsafe { gdkx11::ffi::gdk_x11_display_error_trap_push(self.gdk_display) };
    }

    fn untrap_errors(&self) {
        unsafe { gdkx11::ffi::gdk_x11_display_error_trap_pop_ignored(self.gdk_display) };
    }
}

/// The `{ id, nonce, ok, value }` JSON envelope posted by the evaluation
/// wrapper. On error the value is the script error's message.
struct EvaluationEnvelope {
    id: u64,
    nonce: String,
    ok: bool,
    value: String,
}

impl EvaluationEnvelope {
    fn parse(text: &str) -> Option<Self> {
        let parsed: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(Self {
            id: parsed.get("id").and_then(|v| v.as_u64())?,
            nonce: parsed.get("nonce").and_then(|v| v.as_str())?.to_string(),
            ok: parsed.get("ok").and_then(|v| v.as_bool()).unwrap_or(false),
            value: parsed
                .get("value")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }

    fn into_result(self) -> Result<String> {
        if self.ok {
            Ok(self.value)
        } else {
            Err(anyhow::anyhow!(self.value))
        }
    }
}

/// An unguessable per-view token. `RandomState` is seeded randomly per
/// instance, so hashing a constant yields unrelated keys rather than the
/// fixed `DefaultHasher` output a page could compute.
fn new_nonce() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut nonce = String::new();
    for _ in 0..2 {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u64(std::process::id() as u64);
        nonce.push_str(&format!("{:016x}", hasher.finish()));
    }
    nonce
}

/// The X11 display `gtk::init` opened, as an opaque pointer for x11-dl.
fn x11_display() -> Option<*mut c_void> {
    let display = gdk_x11_display()?;
    Some(unsafe { gdkx11::ffi::gdk_x11_display_get_xdisplay(display) } as *mut c_void)
}

fn gdk_x11_display() -> Option<*mut gdkx11::ffi::GdkX11Display> {
    let display = gtk::gdk::Display::default()?;
    let display = display.downcast_ref::<gdkx11::X11Display>()?;
    Some(display.as_ptr())
}

/// Create the container X window that hosts one webview: a child of GPUI's
/// X window, unmapped until the first renderable frame.
fn create_container(xlib: &Xlib, display: *mut c_void, parent: u64) -> u64 {
    unsafe {
        let window = (xlib.XCreateSimpleWindow)(
            display as _,
            parent as _,
            0,
            0,
            1,
            1,
            0,
            0,
            0,
        );
        window as u64
    }
}

/// The gtk window that wraps the container child, as wry does for X11: the
/// foreign gdk window stands in for the child, and the GtkWindow borrows it
/// so GTK draws into our X window instead of making one of its own. The
/// window is adopted on realize, not before: `set_window` on an unrealized
/// widget is dropped.
fn create_gtk_window(container: u64) -> gtk::Window {
    let display = gtk::gdk::Display::default().expect("webview: no gdk display after gtk::init");
    let display = display
        .downcast_ref::<gdkx11::X11Display>()
        .expect("webview: the session display is not X11; <webview> needs X11");
    let raw = display.as_ptr();
    let gdk_window =
        unsafe { gdkx11::ffi::gdk_x11_window_foreign_new_for_display(raw, container as _) };
    let gdk_window = unsafe { gtk::gdk::Window::from_glib_full(gdk_window) };
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.connect_realize(glib::clone!(@weak gdk_window as wd => move |w| w.set_window(wd)));
    window.set_has_window(true);
    window.realize();
    window
}

/// Wire WebKit signals to the shared service. The service is the same `Rc`
/// the channel callback drives, so handlers reborrow the view by element id
/// on the webview thread. A borrow is only held while one command or one
/// signal runs; neither synchronously emits the other's signals, so the
/// `RefCell` never re-enters.
fn connect_signals(
    webview: &WebView,
    element_id: u64,
    _callback: Option<EventCallback>,
    service: &SharedService,
) {
    // JavaScript evaluation results arrive as script messages; the wrapper
    // posts the `{ ok, value }` envelope. The handler name is app-scoped:
    // `WebView` owns its content manager, so no other view's handler can
    // leak a result across elements.
    if let Some(manager) = webview.user_content_manager() {
        // Registered in the evaluation world only: a page's own world cannot
        // reach it, so page scripts cannot settle the app's evaluations.
        manager.register_script_message_handler_in_world("soloEval", EVAL_WORLD);
        let service = service.clone();
        manager.connect_script_message_received(Some("soloEval"), move |_manager, result| {
            let text = result
                .js_value()
                .map(|value| value.to_string())
                .unwrap_or_default();
            service
                .borrow_mut()
                .on_eval_message(element_id, text);
        });
    }

    {
        let service = service.clone();
        webview.connect_load_changed(move |_webview, event| match event {
            LoadEvent::Started | LoadEvent::Finished => {
                service.borrow_mut().on_load_changed(element_id, event)
            }
            _ => {}
        });
    }

    {
        let service = service.clone();
        webview.connect_load_failed(move |_webview, _event, failing_url, error| {
            let cancelled = error.matches(webkit2gtk::NetworkError::Cancelled);
            service.borrow_mut().on_load_failed(
                element_id,
                failing_url.to_string(),
                error.to_string(),
                cancelled,
            );
            false
        });
    }

    {
        let service = service.clone();
        webview.connect_decide_policy(move |_webview, decision, _kind| {
            service
                .borrow_mut()
                .on_decide_policy(element_id, decision.clone())
        });
    }
}

/// Same-document detection for intercepted navigations, as on macOS:
/// fragment-only navigations are automatic, and new-window requests are
/// cancelled automatically.
fn same_document(requested: Option<&str>, current: Option<&str>) -> bool {
    match (requested, current) {
        (Some(requested), Some(current)) => {
            without_fragment(requested) == without_fragment(current)
        }
        _ => false,
    }
}

fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::custom_elements::webview::WAYLAND_ERROR;

    #[test]
    fn wayland_error_message_is_clear() {
        assert!(WAYLAND_ERROR.contains("Wayland"));
        assert!(WAYLAND_ERROR.contains("X11"));
    }

    #[test]
    fn evaluation_envelopes_name_their_call() {
        let envelope = EvaluationEnvelope::parse(
            r#"{"id":7,"nonce":"0123456789abcdef","ok":true,"value":"1"}"#,
        )
        .unwrap();
        assert_eq!(envelope.id, 7);
        assert_eq!(envelope.nonce, "0123456789abcdef");
        assert_eq!(envelope.into_result().unwrap(), "1");
        // A page (or an older wrapper) cannot settle an evaluation without
        // naming it.
        assert!(EvaluationEnvelope::parse(r#"{"ok":true,"value":"1"}"#).is_none());
        assert_eq!(new_nonce().len(), 32);
    }

    #[test]
    fn same_document_ignores_the_fragment() {
        assert!(same_document(
            Some("https://example.com/a#x"),
            Some("https://example.com/a#y")
        ));
        assert!(!same_document(
            Some("https://example.com/a"),
            Some("https://example.com/b")
        ));
        assert!(!same_document(None, Some("https://example.com/a")));
    }
}
