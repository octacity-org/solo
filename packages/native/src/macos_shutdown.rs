//! Let native menu workers finish before AppKit terminates the host process.

use std::cell::Cell;

use napi::{Error, Result};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSApplication, NSApplicationDelegate, NSApplicationTerminateReply};
use objc2_foundation::{NSObject, NSObjectProtocol};

struct DelegateIvars {
    original: Retained<ProtocolObject<dyn NSApplicationDelegate>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements. AppKit delegate calls,
    // the embedded frame pump and worker-mode main tasks all run on the macOS
    // main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateIvars]
    struct SoloShutdownDelegate;

    unsafe impl NSObjectProtocol for SoloShutdownDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            let own: bool = unsafe { msg_send![super(self), respondsToSelector: selector] };
            own || self.ivars().original.respondsToSelector(selector)
        }
    }

    unsafe impl NSApplicationDelegate for SoloShutdownDelegate {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, app: &NSApplication) -> NSApplicationTerminateReply {
            crate::context_menu::cancel_all();
            if crate::context_menu::has_pending_cleanup() {
                QUIT_PENDING.set(true);
                // TerminateLater starts an AppKit modal run loop, blocking JS.
                // Cancel this attempt and retry from the regular frame pump, or
                // in worker mode when the last menu session completes.
                return NSApplicationTerminateReply::TerminateCancel;
            }
            QUIT_PENDING.set(false);
            if crate::renderer::worker_main::enabled() {
                // The Worker exits first so its JS `exit` handlers run; it then
                // ends the process through requestProcessExit.
                crate::renderer::request_worker_quit();
                return NSApplicationTerminateReply::TerminateCancel;
            }
            let original = &self.ivars().original;
            if original.respondsToSelector(sel!(applicationShouldTerminate:)) {
                original.applicationShouldTerminate(app)
            } else {
                NSApplicationTerminateReply::TerminateNow
            }
        }
    }

    impl SoloShutdownDelegate {
        #[unsafe(method_id(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _selector: Sel) -> Retained<AnyObject> {
            // SAFETY: every Objective-C protocol object is also an AnyObject.
            unsafe { Retained::cast_unchecked(self.ivars().original.clone()) }
        }
    }
);

thread_local! {
    static QUIT_PENDING: Cell<bool> = const { Cell::new(false) };
}

// NSApplication keeps its delegate weakly. The platform owner retains this
// guard and drops it BEFORE GPUI, whose teardown inspects its original delegate.
pub(crate) struct ShutdownGuard {
    app: Retained<NSApplication>,
    proxy: Retained<SoloShutdownDelegate>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        if self.app.delegate().is_some_and(|delegate| {
            Retained::as_ptr(&delegate).cast::<AnyObject>()
                == Retained::as_ptr(&self.proxy).cast::<AnyObject>()
        }) {
            self.app.setDelegate(Some(&self.proxy.ivars().original));
        }
    }
}

pub(crate) fn install() -> Result<ShutdownGuard> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| Error::from_reason("Application shutdown requires the main thread"))?;
    let app = NSApplication::sharedApplication(mtm);
    let original = app
        .delegate()
        .ok_or_else(|| Error::from_reason("GPUI application delegate is not initialized"))?;
    let proxy = SoloShutdownDelegate::alloc(mtm).set_ivars(DelegateIvars { original });
    let proxy: Retained<SoloShutdownDelegate> = unsafe { msg_send![super(proxy), init] };
    app.setDelegate(Some(ProtocolObject::from_ref(&*proxy)));
    QUIT_PENDING.set(false);
    Ok(ShutdownGuard { app, proxy })
}

pub(crate) fn is_quitting() -> bool {
    QUIT_PENDING.get()
}

pub(crate) fn poll() -> Result<()> {
    if is_quitting() && !crate::context_menu::has_pending_cleanup() {
        crate::renderer::quit_macos_application()?;
    }
    Ok(())
}
