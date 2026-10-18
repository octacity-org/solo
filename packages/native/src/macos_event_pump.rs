//! Keep the embedded AppKit pump from waiting for a future external event.

use std::cell::{Cell, OnceCell};
use std::ffi::c_void;
use std::ptr;

use napi::{Error, Result};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFRetained, CFRunLoop, CFRunLoopActivity, CFRunLoopObserver,
    CFRunLoopObserverContext,
};

thread_local! {
    // Installed once. Creating, adding and invalidating it on every tick was a
    // measurable share of idle CPU at the frame loop's 125 ticks per second.
    static WAKE_OBSERVER: OnceCell<CFRetained<CFRunLoopObserver>> = const { OnceCell::new() };
    // The observer outlives each pump, so it wakes the run loop only while one
    // is running. Run loops nested outside a pump must still be able to sleep.
    static PUMPING: Cell<bool> = const { Cell::new(false) };
}

struct Pumping;

impl Drop for Pumping {
    fn drop(&mut self) {
        PUMPING.with(|pumping| pumping.set(false));
    }
}

pub(crate) fn pump_events(platform: &gpui_macos::MacPlatform) -> Result<bool> {
    install_wake_observer()?;
    PUMPING.with(|pumping| pumping.set(true));
    let _pumping = Pumping;
    Ok(platform.pump_events())
}

fn install_wake_observer() -> Result<()> {
    if WAKE_OBSERVER.with(|observer| observer.get().is_some()) {
        return Ok(());
    }
    let run_loop = CFRunLoop::main()
        .ok_or_else(|| Error::from_reason("macOS main run loop is unavailable"))?;
    let mut context = CFRunLoopObserverContext {
        version: 0,
        info: ptr::null_mut(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    // GPUI stops NSApplication.run from an AfterWaiting observer. Its pre-posted
    // NSEvent can be drained before CFRunLoop sleeps, leaving no source to wake
    // it again. Queue the wake at the sleep boundary instead. Do not stop here:
    // the after-wake phase must still service GPUI's main-queue frame callbacks.
    // SAFETY: the callback owns no context and is registered only on the main
    // run loop, which outlives this thread-local observer.
    let observer = unsafe {
        CFRunLoopObserver::new(
            None,
            CFRunLoopActivity::BeforeWaiting.0,
            true,
            isize::MAX,
            Some(wake_before_wait),
            &mut context,
        )
    }
    .ok_or_else(|| Error::from_reason("Failed to create macOS pump wake observer"))?;
    // SAFETY: Core Foundation's process-lifetime common-modes constant.
    run_loop.add_observer(Some(&observer), unsafe { kCFRunLoopCommonModes });
    WAKE_OBSERVER.with(|slot| {
        let _ = slot.set(observer);
    });
    Ok(())
}

unsafe extern "C-unwind" fn wake_before_wait(
    _: *mut CFRunLoopObserver,
    _: CFRunLoopActivity,
    _: *mut c_void,
) {
    if !PUMPING.with(Cell::get) {
        return;
    }
    if let Some(run_loop) = CFRunLoop::main() {
        run_loop.wake_up();
    }
}
