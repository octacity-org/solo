use std::cell::Cell;
use std::ffi::c_void;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, sel, DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSEventModifierFlags, NSEventType,
    NSMenu, NSMenuDelegate, NSMenuItem,
};
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFRetained, CFRunLoop, CFRunLoopTimer, CFRunLoopTimerContext, CGPoint,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSProcessInfo, NSString};

use crate::protocol::{parse_request, MenuItem, MenuRequest};
use crate::scripted::{Navigator, Step};

struct TargetIvars {
    selected: Cell<Option<usize>>,
    tracking: Cell<bool>,
}

define_class!(
    // SAFETY: NSObject has no subclass requirements. The target and delegate
    // are retained for the whole synchronous AppKit tracking session.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct MenuTarget;

    unsafe impl NSObjectProtocol for MenuTarget {}

    unsafe impl NSMenuDelegate for MenuTarget {
        #[unsafe(method(menuWillOpen:))]
        fn menu_will_open(&self, _menu: &NSMenu) {
            self.ivars().tracking.set(true);
            let _ = writeln!(std::io::stdout(), "{{\"ready\":true}}");
        }
    }

    impl MenuTarget {
        #[unsafe(method(selectItem:))]
        fn select_item(&self, sender: &NSMenuItem) {
            if sender.isEnabled() && self.ivars().selected.get().is_none() {
                self.ivars().selected.set(Some(sender.tag() as usize));
            }
        }
    }
);

impl MenuTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars {
            selected: Cell::new(None),
            tracking: Cell::new(false),
        });
        // SAFETY: NSObject's designated initialization, with ivars installed.
        unsafe { msg_send![super(this), init] }
    }
}

struct Cancellation<'a> {
    menu: &'a NSMenu,
    app: &'a NSApplication,
    cancelled: Arc<AtomicBool>,
    keys: Receiver<serde_json::Value>,
    target: &'a MenuTarget,
}

struct Presentation<'a> {
    menu: &'a NSMenu,
    app: &'a NSApplication,
    point: CGPoint,
}

struct RegisteredTimer(CFRetained<CFRunLoopTimer>);

impl Drop for RegisteredTimer {
    fn drop(&mut self) {
        // The run loop retains timers. Remove them before stack contexts drop,
        // including on startup errors after an earlier timer was registered.
        self.0.invalidate();
    }
}

unsafe extern "C-unwind" fn present_menu(_: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: run() retains this context until the one-shot timer is invalidated.
    let context = unsafe { &*(info as *const Presentation<'_>) };
    context
        .menu
        .popUpMenuPositioningItem_atLocation_inView(None, context.point, None);
    context.app.stop(None);
    let chars = NSString::from_str("");
    if let Some(event) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
        NSEventType::KeyUp, CGPoint::new(0.0, 0.0), NSEventModifierFlags::empty(),
        0.0, 0, None, &chars, &chars, false, 0,
    ) { context.app.postEvent_atStart(&event, true); }
}

unsafe extern "C-unwind" fn check_cancel(_: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: run() keeps the context alive until its timer is invalidated.
    let context = unsafe { &*(info as *const Cancellation<'_>) };
    if context.cancelled.load(Ordering::Acquire) {
        context.menu.cancelTrackingWithoutAnimation();
    }
    if !context.target.ivars().tracking.get() {
        return;
    }
    // Keys for an open menu (automation that never sent `initialize`) are real
    // NSEvents, so AppKit handles highlighting, disabled items and Escape.
    // Depends on focus; attached automation scripts its menus instead.
    if let Ok(command) = context.keys.try_recv() {
        let Some(key) = command["key"].as_str() else {
            return;
        };
        let (text, code) = match key {
            "down" => ("\u{f701}", 125),
            "up" => ("\u{f700}", 126),
            "enter" => ("\r", 36),
            "escape" => ("\u{1b}", 53),
            text if text.chars().count() == 1 => (text, 0),
            _ => return,
        };
        let mut flags = if matches!(key, "down" | "up") {
            NSEventModifierFlags::Function | NSEventModifierFlags::NumericPad
        } else {
            NSEventModifierFlags::empty()
        };
        for (field, flag) in [
            ("shift", NSEventModifierFlags::Shift),
            ("control", NSEventModifierFlags::Control),
            ("alt", NSEventModifierFlags::Option),
            ("platform", NSEventModifierFlags::Command),
        ] {
            if command[field].as_bool() == Some(true) {
                flags |= flag;
            }
        }
        let chars = NSString::from_str(text);
        if let Some(event) = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, CGPoint::new(0.0, 0.0), flags,
            NSProcessInfo::processInfo().systemUptime(), 0, None, &chars, &chars, false, code,
        ) { context.app.postEvent_atStart(&event, false); }
    }
}

pub fn run() -> Result<(), String> {
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    input.read_line(&mut line).map_err(|e| e.to_string())?;
    let request = parse_request(&line)?;
    drop(input);

    // Parent EOF is also the crash/exit cleanup path. No process-name matching,
    // polling of parent PIDs, or detached orphan menu can survive that EOF.
    let cancelled = Arc::new(AtomicBool::new(false));
    let reader_cancelled = cancelled.clone();
    let (key_sender, keys) = mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                break;
            };
            if !value["key"].is_string() {
                break;
            }
            if key_sender.send(value).is_err() {
                break;
            }
        }
        reader_cancelled.store(true, Ordering::Release);
        // Ends a scripted menu's wait for keys.
        drop(key_sender);
        // If the parent has died, nobody remains to enforce its kill deadline.
        // This thread is independent of AppKit: even stuck menu tracking must
        // release the private executable and stop after owner disconnection.
        std::thread::sleep(std::time::Duration::from_secs(1));
        crate::cleanup_staged_executable();
        std::process::exit(0);
    });

    let mtm = MainThreadMarker::new().ok_or("Menu helper must run on its main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let main_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    app.setMainMenu(Some(&main_menu));
    // A scripted menu never opens, so it leaves the frontmost app alone.
    if !request.scripted {
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
    }

    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(""));
    menu.setAutoenablesItems(false);
    let target = MenuTarget::new(mtm);
    menu.setDelegate(Some(ProtocolObject::from_ref(&*target)));
    for (index, descriptor) in request.items.iter().enumerate() {
        match descriptor {
            MenuItem::Separator { .. } => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuItem::Action {
                label,
                disabled,
                checked,
                ..
            } => {
                // SAFETY: selectItem: is implemented with the required signature
                // by the retained target immediately below.
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(sel!(selectItem:)),
                        &NSString::from_str(""),
                    )
                };
                unsafe { item.setTarget(Some(&target)) };
                item.setTag(index as isize);
                item.setEnabled(!disabled);
                item.setState(if *checked { 1 } else { 0 });
                menu.addItem(&item);
            }
        }
    }
    if request.scripted {
        let _ = writeln!(std::io::stdout(), "{{\"ready\":true}}");
        let mut navigator = Navigator::default();
        // Ends on Enter, Escape, or parent EOF (the reader drops the sender).
        while let Ok(command) = keys.recv() {
            match navigator.key(&request.items, command["key"].as_str().unwrap_or_default()) {
                Step::Continue => {}
                Step::Cancel => break,
                // Through the item's action, as a click would choose it.
                Step::Select(index) => {
                    menu.performActionForItemAtIndex(index as isize);
                    break;
                }
            }
        }
        return finish(&request, &menu, &target);
    }
    let mut cancellation = Cancellation {
        menu: &menu,
        app: &app,
        cancelled,
        keys,
        target: &target,
    };
    let mut context = CFRunLoopTimerContext {
        version: 0,
        info: (&mut cancellation as *mut Cancellation<'_>).cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };
    // The common-mode timer handles cancellation and keys during menu tracking;
    // the application's JavaScript loop runs independently in its own process.
    // SAFETY: the guard is dropped before cancellation and its borrowed values.
    let timer = RegisteredTimer(
        unsafe { CFRunLoopTimer::new(None, 0.0, 0.02, 0, 0, Some(check_cancel), &mut context) }
            .ok_or("Unable to create cancellation timer")?,
    );
    let run_loop = CFRunLoop::main().ok_or("No macOS main run loop")?;
    run_loop.add_timer(Some(&timer.0), unsafe { kCFRunLoopCommonModes });
    if !cancellation.cancelled.load(Ordering::Acquire) {
        let mut presentation = Presentation {
            menu: &menu,
            app: &app,
            point: CGPoint::new(request.x, request.y),
        };
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info: (&mut presentation as *mut Presentation<'_>).cast(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        // SAFETY: presentation outlives the timer guard and synchronous app.run.
        let present = RegisteredTimer(
            unsafe { CFRunLoopTimer::new(None, 0.0, 0.0, 0, 0, Some(present_menu), &mut context) }
                .ok_or("Unable to schedule menu")?,
        );
        run_loop.add_timer(Some(&present.0), unsafe { kCFRunLoopCommonModes });
        app.run();
    }
    drop(timer);
    finish(&request, &menu, &target)
}

fn finish(request: &MenuRequest, menu: &NSMenu, target: &MenuTarget) -> Result<(), String> {
    menu.setDelegate(None);
    let selected = target
        .ivars()
        .selected
        .get()
        .and_then(|index| match &request.items[index] {
            MenuItem::Action { id, .. } => Some(id.as_str()),
            _ => None,
        });
    let output = serde_json::to_string(&selected).map_err(|e| e.to_string())?;
    match writeln!(std::io::stdout(), "{output}") {
        // Losing the reader is ordinary parent shutdown, not a menu failure.
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        result => result.map_err(|error| error.to_string()),
    }
}
