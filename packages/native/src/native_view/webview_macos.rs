//! macOS WKWebView host for Solo's `<webview>` element.
//!
//! A WKWebView is an AppKit NSView, not a GPUI element: it cannot be painted
//! into GPUI's scene, so it is attached to the real NSView hierarchy above
//! GPUI's opaque Metal surface and positioned from GPUI's layout bounds.
//!
//! GPUI owns layout; WebKit owns rendering and input. Nothing here is driven
//! from JavaScript.
//!
use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::rc::Rc;

use block2::RcBlock;
use futures::channel::oneshot;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::NSView;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{NSError, NSObject, NSObjectProtocol, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{
    WKContentWorld, WKNavigation, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKNavigationType, WKWebView, WKWebViewConfiguration,
};

use super::{EvaluateReceiver, NativeViewFrame, NativeViewInstance, ReadyReceiver, evaluation_script};
use crate::renderer::{EventCallback, emit_event_full};

// ── Event names ──────────────────────────────────────────────────────

/// Must match the native event types in `EVENT_PROPS`
/// (`packages/core/src/events.ts`).
const EVENT_NAVIGATION: &str = "navigation";
const EVENT_NAVIGATION_REQUEST: &str = "navigationRequest";
const EVENT_LOAD: &str = "load";
const EVENT_LOAD_ERROR: &str = "loadError";

/// Property keys accepted by [`MacWebView::set_content`].
const PROP_URL: &str = "url";
const PROP_HTML: &str = "html";
const PROP_BASE_URL: &str = "baseUrl";
const PROP_USER_AGENT: &str = "userAgent";

type EvaluateSender = oneshot::Sender<anyhow::Result<String>>;
type ReadySender = oneshot::Sender<anyhow::Result<u64>>;
type DecisionHandler = RcBlock<dyn Fn(WKNavigationActionPolicy)>;

#[derive(Default)]
struct NavigationState {
    intercept: bool,
    controlled_load: bool,
    next_id: u64,
    pending: HashMap<u64, DecisionHandler>,
}

type EvaluationState = Rc<RefCell<HashMap<u64, EvaluateSender>>>;

#[derive(Default)]
struct ReadinessState {
    generation: u64,
    ready: bool,
    /// Identity of the navigation whose completion can make this generation
    /// ready. Keeping this identity prevents a late callback for the previous
    /// document from masquerading as the current document's load.
    expected_navigation: Option<usize>,
    waiters: HashMap<u64, Vec<ReadySender>>,
}

type SharedReadiness = Rc<RefCell<ReadinessState>>;

// ── Navigation delegate ──────────────────────────────────────────────

/// State the delegate needs in order to emit. Immutable after construction.
///
/// The callback is `Arc<dyn Fn(EventPayload) + Send + Sync>`, so the delegate
/// can emit straight from a WebKit callback without touching GPUI's `App` and
/// without ever blocking on JavaScript.
struct DelegateIvars {
    element_id: u64,
    callback: Option<EventCallback>,
    navigation: RefCell<NavigationState>,
    evaluations: EvaluationState,
    readiness: SharedReadiness,
}

define_class!(
    /// Forwards WebKit navigation callbacks into Solo's event pipeline.
    ///
    /// One per mounted `<webview>`. Events always fire; the JS-side handler
    /// registry decides whether anything is listening, so a handler appearing
    /// or disappearing between frames needs no re-plumbing here.
    //
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - `DelegateIvars` has no `Drop` impl, so the class does not need one
    //   either.
    #[unsafe(super(NSObject))]
    // WebKit calls these methods on the main thread, and `WKNavigationDelegate`
    // is declared `MainThreadOnly`, so the class has to opt in to match.
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct SoloNavigationDelegate;

    unsafe impl NSObjectProtocol for SoloNavigationDelegate {}

    unsafe impl WKNavigationDelegate for SoloNavigationDelegate {
        /// Intercept main-frame navigation before WebKit commits it. When the
        /// public Solid listener is absent, preserve WKWebView's normal allow
        /// behavior; with a listener, keep the decision handler alive until
        /// the controller explicitly allows or cancels it.
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        unsafe fn webView_decidePolicyForNavigationAction_decisionHandler(
            &self,
            web_view: &WKWebView,
            navigation_action: &WKNavigationAction,
            decision_handler: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let ivars = self.ivars();
            let requested_url = action_url(navigation_action);
            let current = current_url(web_view);
            let same_document = requested_url
                .as_deref()
                .zip(current.as_deref())
                .is_some_and(|(requested, current)| {
                    without_fragment(requested) == without_fragment(current)
                });
            let new_window = navigation_action.targetFrame().is_none();
            let navigation_type = navigation_action.navigationType();
            let (intercept, controlled_load, navigation_id) = {
                let mut state = ivars.navigation.borrow_mut();
                let intercept = state.intercept;
                let controlled_load = state.controlled_load;
                state.controlled_load = false;
                let navigation_id = state.next_id;
                state.next_id = state.next_id.wrapping_add(1).max(1);
                if intercept
                    && navigation_type == WKNavigationType::LinkActivated
                    && !same_document
                    && !new_window
                {
                    state.pending.insert(navigation_id, decision_handler.copy());
                }
                (intercept, controlled_load, navigation_id)
            };

            if new_window {
                emit_navigation_request(
                    &ivars.callback,
                    ivars.element_id,
                    navigation_id,
                    requested_url,
                    false,
                    true,
                );
                // Solo deliberately does not create a second native window.
                // The Solid handler can open the URL externally.
                decision_handler.call((WKNavigationActionPolicy::Cancel,));
            } else if same_document {
                emit_navigation_request(
                    &ivars.callback,
                    ivars.element_id,
                    navigation_id,
                    requested_url,
                    true,
                    false,
                );
                decision_handler.call((WKNavigationActionPolicy::Allow,));
            } else if controlled_load {
                decision_handler.call((WKNavigationActionPolicy::Allow,));
            } else if navigation_type != WKNavigationType::LinkActivated {
                // Redirects and programmatic location changes are allowed
                // without waiting on application code. This keeps a source
                // load from deadlocking while user article links remain
                // cancellable through onNavigationRequest.
                begin_document(&ivars.evaluations, &ivars.readiness);
                decision_handler.call((WKNavigationActionPolicy::Allow,));
            } else if intercept {
                emit_navigation_request(
                    &ivars.callback,
                    ivars.element_id,
                    navigation_id,
                    requested_url,
                    false,
                    false,
                );
            } else {
                decision_handler.call((WKNavigationActionPolicy::Allow,));
            }
        }

        /// A navigation started — the earliest meaningful signal, and the one
        /// that covers link clicks and redirects rather than just first loads.
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        unsafe fn webView_didStartProvisionalNavigation(
            &self,
            web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
        ) {
            let ivars = self.ivars();
            emit(
                &ivars.callback,
                ivars.element_id,
                EVENT_NAVIGATION,
                current_url(web_view),
            );
        }

        /// Content finished arriving for the main frame.
        #[unsafe(method(webView:didFinishNavigation:))]
        unsafe fn webView_didFinishNavigation(
            &self,
            web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
        ) {
            let ivars = self.ivars();
            if mark_ready(&ivars.readiness, _navigation) {
                emit(
                    &ivars.callback,
                    ivars.element_id,
                    EVENT_LOAD,
                    current_url(web_view),
                );
            }
        }

        /// A committed navigation failed part-way through.
        #[unsafe(method(webView:didFailNavigation:withError:))]
        unsafe fn webView_didFailNavigation_withError(
            &self,
            web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            let ivars = self.ivars();
            if mark_failed(
                &ivars.readiness,
                _navigation,
                &error.localizedDescription().to_string(),
            ) {
                emit(
                    &ivars.callback,
                    ivars.element_id,
                    EVENT_LOAD_ERROR,
                    failure_url(web_view, error),
                );
            }
        }

        /// The navigation failed before committing: bad host, no network, or
        /// an unparseable URL. Often the only signal we get for a typo'd URL.
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        unsafe fn webView_didFailProvisionalNavigation_withError(
            &self,
            web_view: &WKWebView,
            _navigation: Option<&WKNavigation>,
            error: &NSError,
        ) {
            let ivars = self.ivars();
            if mark_failed(
                &ivars.readiness,
                _navigation,
                &error.localizedDescription().to_string(),
            ) {
                emit(
                    &ivars.callback,
                    ivars.element_id,
                    EVENT_LOAD_ERROR,
                    failure_url(web_view, error),
                );
            }
        }
    }
);

impl SoloNavigationDelegate {
    fn new(
        mtm: MainThreadMarker,
        element_id: u64,
        callback: Option<EventCallback>,
        evaluations: EvaluationState,
        readiness: SharedReadiness,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            element_id,
            callback,
            navigation: RefCell::new(NavigationState::default()),
            evaluations,
            readiness,
        });
        unsafe { msg_send![super(this), init] }
    }
}

// ── Emitting ─────────────────────────────────────────────────────────

/// `EventPayload` has no message field, so a failure puts the reason after the
/// URL in `value`.
fn failure_url(web_view: &WKWebView, error: &NSError) -> Option<String> {
    let url = current_url(web_view).unwrap_or_default();
    let reason = error.localizedDescription().to_string();
    Some(format!("{url} — {reason}"))
}

fn current_url(web_view: &WKWebView) -> Option<String> {
    let url: Retained<NSURL> = unsafe { web_view.URL() }?;
    let absolute = url.absoluteString()?;
    Some(absolute.to_string())
}

fn action_url(action: &WKNavigationAction) -> Option<String> {
    let request = unsafe { action.request() };
    let url = request.URL()?;
    let absolute = url.absoluteString()?;
    Some(absolute.to_string())
}

fn without_fragment(url: &str) -> &str {
    url.split_once('#').map_or(url, |(without, _)| without)
}

/// Start a new document generation and settle work that targeted the old one.
/// The native callbacks are asynchronous, so dropping a sender is not enough:
/// every caller must receive an explicit error and late WebKit callbacks must
/// find no sender to complete.
fn begin_document(evaluations: &EvaluationState, readiness: &SharedReadiness) {
    for (_, sender) in evaluations.borrow_mut().drain() {
        let _ = sender.send(Err(anyhow::anyhow!("WebView document changed")));
    }

    let waiters = {
        let mut state = readiness.borrow_mut();
        state.generation = state.generation.wrapping_add(1).max(1);
        state.ready = false;
        state.expected_navigation = None;
        std::mem::take(&mut state.waiters)
    };
    for (_, senders) in waiters {
        for sender in senders {
            let _ = sender.send(Err(anyhow::anyhow!("WebView document changed")));
        }
    }
}

fn navigation_key(navigation: Option<&WKNavigation>) -> Option<usize> {
    navigation.map(|navigation| std::ptr::from_ref(navigation) as usize)
}

/// Record the navigation returned by a controlled `loadRequest` or
/// `loadHTMLString` call. This is the only callback allowed to complete that
/// generation; an older navigation can still report completion after a source
/// update.
fn expect_navigation(readiness: &SharedReadiness, navigation: Option<&WKNavigation>) {
    let mut state = readiness.borrow_mut();
    if !state.ready {
        state.expected_navigation = navigation_key(navigation);
    }
}

fn navigation_matches(state: &mut ReadinessState, navigation: Option<&WKNavigation>) -> bool {
    if state.ready {
        return false;
    }
    let key = navigation_key(navigation);
    match state.expected_navigation {
        Some(expected) => key == Some(expected),
        None => {
            // User-approved links and WebKit redirects do not give us a
            // WKNavigation at the policy callback. The first start/finish
            // callback after that decision establishes the identity.
            state.expected_navigation = key;
            true
        }
    }
}

fn mark_ready(readiness: &SharedReadiness, navigation: Option<&WKNavigation>) -> bool {
    let (generation, waiters) = {
        let mut state = readiness.borrow_mut();
        if !navigation_matches(&mut state, navigation) {
            return false;
        }
        state.ready = true;
        state.expected_navigation = None;
        (state.generation, std::mem::take(&mut state.waiters))
    };
    for (_, senders) in waiters {
        for sender in senders {
            let _ = sender.send(Ok(generation));
        }
    }
    true
}

fn mark_failed(
    readiness: &SharedReadiness,
    navigation: Option<&WKNavigation>,
    reason: &str,
) -> bool {
    let waiters = {
        let mut state = readiness.borrow_mut();
        if !navigation_matches(&mut state, navigation) {
            return false;
        }
        state.expected_navigation = None;
        std::mem::take(&mut state.waiters)
    };
    for (_, senders) in waiters {
        for sender in senders {
            let _ = sender.send(Err(anyhow::anyhow!(reason.to_string())));
        }
    }
    true
}

fn cancel_pending_navigation(navigation: &RefCell<NavigationState>) {
    let pending = std::mem::take(&mut navigation.borrow_mut().pending);
    for handler in pending.into_values() {
        handler.call((WKNavigationActionPolicy::Cancel,));
    }
}

fn emit(
    callback: &Option<EventCallback>,
    element_id: u64,
    event_type: &str,
    value: Option<String>,
) {
    let value = value.unwrap_or_default();
    emit_event_full(callback, element_id, event_type, move |payload| {
        payload.value = Some(value);
    });
}

fn emit_navigation_request(
    callback: &Option<EventCallback>,
    element_id: u64,
    navigation_id: u64,
    value: Option<String>,
    same_document: bool,
    new_window: bool,
) {
    emit_event_full(
        callback,
        element_id,
        EVENT_NAVIGATION_REQUEST,
        move |payload| {
            payload.navigation_id = Some(navigation_id as f64);
            payload.value = value.clone();
            payload.navigation_url = value;
            payload.is_same_document = Some(same_document);
            payload.is_new_window = Some(new_window);
        },
    );
}

// ── The native view ──────────────────────────────────────────────────

/// A WKWebView mounted as a child of Solo's GPUI window view.
pub struct MacWebView {
    element_id: u64,
    callback: Option<EventCallback>,
    view: Option<Retained<WKWebView>>,
    /// WebKit holds the navigation delegate weakly, so Solo must keep it
    /// alive for exactly as long as the WKWebView.
    delegate: Option<Retained<SoloNavigationDelegate>>,
    /// Last frame pushed to AppKit, so layout passes that produce identical
    /// geometry do not churn `setFrame:`.
    last_frame: Option<NativeViewFrame>,
    /// Last URL requested. `set_prop` runs every frame, so without this a
    /// single `url` write would reload the page on every render.
    last_url: Option<String>,
    last_html: Option<String>,
    last_base_url: Option<String>,
    last_user_agent: Option<String>,
    /// Whether the view is currently shown. Driven by frame area: a zero-area
    /// view keeps a web process alive for nothing, so it is hidden instead.
    visible: bool,
    evaluations: EvaluationState,
    readiness: SharedReadiness,
    next_evaluation_id: u64,
}

impl MacWebView {
    /// Create the host for `element_id`.
    ///
    /// Nothing is created until [`NativeViewInstance::mount`]; the view is
    /// built lazily once GPUI can supply a host.
    pub fn new(element_id: u64, callback: Option<EventCallback>) -> Self {
        Self {
            element_id,
            callback,
            view: None,
            delegate: None,
            last_frame: None,
            last_url: None,
            last_html: None,
            last_base_url: None,
            last_user_agent: None,
            visible: true,
            evaluations: Rc::new(RefCell::new(HashMap::new())),
            readiness: Rc::new(RefCell::new(ReadinessState {
                ready: true,
                ..ReadinessState::default()
            })),
            next_evaluation_id: 1,
        }
    }

    /// Navigate, reusing the existing WKWebView.
    ///
    /// A reactive `url` change must never rebuild the native view.
    fn load_url(&self, url: &str) {
        let Some(ns_url) = NSURL::URLWithString(&NSString::from_str(url)) else {
            mark_failed(&self.readiness, None, &format!("{url} — invalid URL"));
            emit(
                &self.callback,
                self.element_id,
                EVENT_LOAD_ERROR,
                Some(format!("{url} — invalid URL")),
            );
            return;
        };
        let Some(view) = self.view.as_ref() else {
            return;
        };
        let request = NSURLRequest::requestWithURL(&ns_url);
        self.mark_controlled_load();
        let navigation = unsafe { view.loadRequest(&request) };
        expect_navigation(&self.readiness, navigation.as_deref());
    }

    fn load_html(&self, html: &str, base_url: Option<&str>) {
        let base = match base_url {
            Some(value) => match NSURL::URLWithString(&NSString::from_str(value)) {
                Some(url) => Some(url),
                None => {
                    mark_failed(
                        &self.readiness,
                        None,
                        &format!("{value} — invalid base URL"),
                    );
                    emit(
                        &self.callback,
                        self.element_id,
                        EVENT_LOAD_ERROR,
                        Some(format!("{value} — invalid base URL")),
                    );
                    return;
                }
            },
            None => None,
        };
        let Some(view) = self.view.as_ref() else {
            return;
        };
        self.mark_controlled_load();
        let navigation =
            unsafe { view.loadHTMLString_baseURL(&NSString::from_str(html), base.as_deref()) };
        expect_navigation(&self.readiness, navigation.as_deref());
    }

    fn begin_document(&mut self) {
        if let Some(delegate) = self.delegate.as_ref() {
            cancel_pending_navigation(&delegate.ivars().navigation);
        }
        begin_document(&self.evaluations, &self.readiness);
    }

    fn mark_controlled_load(&self) {
        if let Some(delegate) = self.delegate.as_ref() {
            delegate.ivars().navigation.borrow_mut().controlled_load = true;
        }
    }

    fn wait_for_ready_inner(&mut self) -> anyhow::Result<ReadyReceiver> {
        let _view = self
            .view
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView is not mounted"))?;
        let (sender, receiver) = oneshot::channel();
        let (generation, ready) = {
            let state = self.readiness.borrow();
            (state.generation, state.ready)
        };
        if ready {
            let _ = sender.send(Ok(generation));
        } else {
            self.readiness
                .borrow_mut()
                .waiters
                .entry(generation)
                .or_default()
                .push(sender);
        }
        Ok(receiver)
    }

    fn evaluate_javascript_inner(&mut self, script: &str) -> anyhow::Result<EvaluateReceiver> {
        let view = self
            .view
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("WebView is not mounted"))?;
        let (sender, receiver) = oneshot::channel();
        let evaluation_id = self.next_evaluation_id;
        self.next_evaluation_id = self.next_evaluation_id.wrapping_add(1).max(1);
        self.evaluations.borrow_mut().insert(evaluation_id, sender);
        let evaluations = self.evaluations.clone();
        let callback = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
            let response = if !error.is_null() {
                let error = unsafe { &*error };
                Err(anyhow::anyhow!(error.localizedDescription().to_string()))
            } else if result.is_null() {
                Err(anyhow::anyhow!("WebView JavaScript returned no result"))
            } else {
                let value = unsafe { &*(result as *const NSString) };
                Ok(value.to_string())
            };
            if let Some(sender) = evaluations.borrow_mut().remove(&evaluation_id) {
                let _ = sender.send(response);
            }
        });
        let body = evaluation_script(script);
        let content_world = unsafe {
            WKContentWorld::defaultClientWorld(MainThreadMarker::new().expect("main thread"))
        };
        unsafe {
            view.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
                &NSString::from_str(&body),
                None,
                None,
                &content_world,
                Some(&callback),
            );
        }
        Ok(receiver)
    }
}

impl NativeViewInstance for MacWebView {
    fn mount(&mut self, host: *mut c_void) {
        if self.view.is_some() || host.is_null() {
            return;
        }

        // Everything below is main-thread-only: `WKWebView` and
        // `WKWebViewConfiguration` are both declared `MainThreadOnly`, and
        // `SoloNavigationDelegate` has to match `WKNavigationDelegate`. Solo's
        // `prepaint` runs inside the window's draw on the main thread, so this
        // is a formality — but `MainThreadMarker` makes the requirement a
        // compile-time one rather than a WebKit crash.
        let Some(mtm) = MainThreadMarker::new() else {
            log::error!("webview: mounted off the main thread, refusing to create a WKWebView");
            return;
        };

        // `host` is the GPUIView handed over by Solo's element. Appending the
        // WKWebView as its subview puts it above GPUI's layer-backed content,
        // which is what makes it visible at all: GPUI clears its Metal
        // drawable opaquely every frame, so anything *below* it is hidden.
        let host: &NSView = unsafe { &*(host as *const NSView) };
        let frame = self.last_frame.unwrap_or_default();
        let config: Retained<WKWebViewConfiguration> =
            unsafe { msg_send![WKWebViewConfiguration::alloc(mtm), init] };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                native_rect(frame),
                &config,
            )
        };

        let delegate = SoloNavigationDelegate::new(
            mtm,
            self.element_id,
            self.callback.clone(),
            self.evaluations.clone(),
            self.readiness.clone(),
        );
        unsafe {
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        }
        // Retained here, not just handed to WebKit: the property is weak.
        self.delegate = Some(delegate);

        view.setHidden(!self.visible);
        host.addSubview(&view);
        self.view = Some(view);

        // Content set before the view existed still has to load.
        if let Some(html) = self.last_html.clone() {
            self.load_html(&html, self.last_base_url.as_deref());
        } else if let Some(url) = self.last_url.clone() {
            self.load_url(&url);
        }
    }

    fn update_frame(&mut self, frame: NativeViewFrame) {
        if self.last_frame == Some(frame) {
            return;
        }
        self.last_frame = Some(frame);

        // A zero-area view keeps compositing layers alive for a web process
        // that shows nothing, so hide rather than leave it mounted.
        let renderable = frame.is_renderable();
        if renderable != self.visible {
            self.visible = renderable;
            if let Some(view) = self.view.as_ref() {
                view.setHidden(!renderable);
            }
        }

        if let Some(view) = self.view.as_ref() {
            view.setFrame(native_rect(frame));
        }
    }

    fn set_content(&mut self, key: &str, value: Option<&str>) {
        match key {
            PROP_URL => {
                if value.is_some() && self.last_html.is_some() {
                    emit(
                        &self.callback,
                        self.element_id,
                        EVENT_LOAD_ERROR,
                        Some("WebView url and html are mutually exclusive".to_string()),
                    );
                    return;
                }
                if self.last_url.as_deref() == value {
                    return;
                }
                self.begin_document();
                self.last_url = value.map(str::to_string);
                if let Some(url) = value {
                    self.load_url(url);
                }
            }
            PROP_HTML => {
                if value.is_some() && self.last_url.is_some() {
                    emit(
                        &self.callback,
                        self.element_id,
                        EVENT_LOAD_ERROR,
                        Some("WebView url and html are mutually exclusive".to_string()),
                    );
                    return;
                }
                if self.last_html.as_deref() == value {
                    return;
                }
                self.begin_document();
                self.last_html = value.map(str::to_string);
                if let Some(html) = value {
                    self.load_html(html, self.last_base_url.as_deref());
                }
            }
            PROP_BASE_URL => {
                if value.is_some() && self.last_url.is_some() {
                    emit(
                        &self.callback,
                        self.element_id,
                        EVENT_LOAD_ERROR,
                        Some("WebView baseUrl requires html and cannot be used with url".to_string()),
                    );
                    return;
                }
                if self.last_base_url.as_deref() == value {
                    return;
                }
                self.begin_document();
                self.last_base_url = value.map(str::to_string);
                if let Some(html) = self.last_html.clone() {
                    self.load_html(&html, self.last_base_url.as_deref());
                }
            }
            PROP_USER_AGENT => {
                if self.last_user_agent.as_deref() == value {
                    return;
                }
                self.last_user_agent = value.map(str::to_string);
                if let Some(view) = self.view.as_ref() {
                    let agent = value.map(NSString::from_str);
                    unsafe { view.setCustomUserAgent(agent.as_deref()) };
                }
            }
            _ => {}
        }
    }

    fn set_webview_source(
        &mut self,
        url: Option<&str>,
        html: Option<&str>,
        base_url: Option<&str>,
    ) {
        if url.is_some() && html.is_some() {
            emit(
                &self.callback,
                self.element_id,
                EVENT_LOAD_ERROR,
                Some("WebView url and html are mutually exclusive".to_string()),
            );
            return;
        }
        if base_url.is_some() && html.is_none() {
            emit(
                &self.callback,
                self.element_id,
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

        self.begin_document();
        self.last_url = url.map(str::to_string);
        self.last_html = html.map(str::to_string);
        self.last_base_url = base_url.map(str::to_string);

        if let Some(html) = html {
            self.load_html(html, base_url);
        } else if let Some(url) = url {
            self.load_url(url);
        } else if let Some(view) = self.view.as_ref() {
            self.mark_controlled_load();
            let navigation = unsafe { view.loadHTMLString_baseURL(&NSString::new(), None) };
            expect_navigation(&self.readiness, navigation.as_deref());
        }
    }

    fn evaluate_javascript(&mut self, script: &str) -> anyhow::Result<EvaluateReceiver> {
        self.evaluate_javascript_inner(script)
    }

    fn wait_for_ready(&mut self) -> anyhow::Result<ReadyReceiver> {
        self.wait_for_ready_inner()
    }

    fn set_navigation_interception(&mut self, enabled: bool) {
        if let Some(delegate) = self.delegate.as_ref() {
            let navigation = &delegate.ivars().navigation;
            navigation.borrow_mut().intercept = enabled;
            if !enabled {
                cancel_pending_navigation(navigation);
            }
        }
    }

    fn decide_navigation(&mut self, navigation_id: u64, allow: bool) -> bool {
        let Some(delegate) = self.delegate.as_ref() else {
            return false;
        };
        let handler = delegate
            .ivars()
            .navigation
            .borrow_mut()
            .pending
            .remove(&navigation_id);
        if let Some(handler) = handler {
            if allow {
                self.begin_document();
            }
            handler.call((if allow {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            },));
            true
        } else {
            false
        }
    }

    fn destroy(&mut self) {
        self.begin_document();
        mark_failed(&self.readiness, None, "WebView was destroyed");
        if let Some(delegate) = self.delegate.as_ref() {
            cancel_pending_navigation(&delegate.ivars().navigation);
        }
        if let Some(view) = self.view.take() {
            view.removeFromSuperview();
        }
        // Dropping the delegate last: the WKWebView holds it weakly, so it must
        // stay alive until the view is gone. Both `Retained`s drop here.
        self.delegate = None;
        self.callback = None;
        self.last_frame = None;
        self.last_url = None;
        self.last_html = None;
        self.last_base_url = None;
        self.last_user_agent = None;
    }
}

/// Build the AppKit rect for a frame.
///
/// GPUI bounds are logical points and `to_native_frame` has already flipped Y,
/// so this is a plain conversion — no scale factor, since `NSView` frames are
/// points rather than device pixels.
fn native_rect(frame: NativeViewFrame) -> CGRect {
    CGRect {
        origin: CGPoint {
            x: frame.x as f64,
            y: frame.y as f64,
        },
        size: CGSize {
            width: frame.width as f64,
            height: frame.height as f64,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::element_tree::EventPayload;

    #[test]
    fn malformed_url_emits_load_error_before_mount() {
        let events = Arc::new(Mutex::new(Vec::<EventPayload>::new()));
        let captured = events.clone();
        let callback: EventCallback = Arc::new(move |payload| {
            captured.lock().unwrap().push(payload);
        });
        let mut webview = MacWebView::new(42, Some(callback));

        webview.set_content(PROP_URL, Some("https://exa mple.com"));

        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].element_id, 42.0);
        assert_eq!(events[0].event_type, EVENT_LOAD_ERROR);
        assert!(
            events[0]
                .value
                .as_deref()
                .is_some_and(|value| value.contains("https://exa mple.com"))
        );
    }

    #[test]
    fn fragment_only_changes_are_same_document() {
        assert_eq!(
            without_fragment("https://example.com/article#one"),
            "https://example.com/article"
        );
        assert_eq!(without_fragment("about:blank"), "about:blank");
    }

    #[test]
    fn evaluation_wrapper_awaits_and_rejects_unsupported_values() {
        let source = evaluation_script("Promise.resolve({ answer: 42 })");
        assert!(source.contains("await (Promise.resolve({ answer: 42 }))"));
        assert!(source.contains("value === undefined"));
        assert!(source.contains("Number.isFinite"));
        assert!(source.contains("unsupported JavaScript result"));
        assert!(source.contains("unsupported cyclic result"));
    }

    #[test]
    fn beginning_a_document_rejects_old_work_and_resets_readiness() {
        let evaluations: EvaluationState = Rc::new(RefCell::new(HashMap::new()));
        let readiness: SharedReadiness = Rc::new(RefCell::new(ReadinessState {
            generation: 3,
            ready: true,
            ..ReadinessState::default()
        }));
        let (evaluation_sender, evaluation_receiver) = oneshot::channel();
        evaluations.borrow_mut().insert(1, evaluation_sender);
        let (ready_sender, ready_receiver) = oneshot::channel();
        readiness
            .borrow_mut()
            .waiters
            .entry(3)
            .or_default()
            .push(ready_sender);

        begin_document(&evaluations, &readiness);

        assert_eq!(readiness.borrow().generation, 4);
        assert!(!readiness.borrow().ready);
        assert!(
            futures::executor::block_on(evaluation_receiver)
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("document changed")
        );
        assert!(
            futures::executor::block_on(ready_receiver)
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("document changed")
        );
    }
}
