//! macOS status item for opt-in background applications.

use napi::bindgen_prelude::{Error, Result};
use objc2::rc::Retained;
use objc2::{
    define_class, msg_send, sel, AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_app_kit::{
    NSControlStateValueOff, NSControlStateValueOn, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::{NSObject, NSObjectProtocol, NSString};

use super::{parse_update, MenuBarItem, MENU_BAR_ACTION_EVENT};
use crate::renderer::EventCallback;

struct TargetIvars;

struct ActionTargetIvars {
    token: String,
    callback: Option<EventCallback>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class owns no
    // resources. AppKit invokes menu actions on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = TargetIvars]
    struct SoloMenuBarTarget;

    unsafe impl NSObjectProtocol for SoloMenuBarTarget {}

    impl SoloMenuBarTarget {
        #[unsafe(method(openWindow:))]
        fn open_window(&self, _sender: &NSMenuItem) {
            if let Err(error) = crate::renderer::show_macos_window() {
                log::error!("Failed to show the Solo window: {error}");
            }
        }

        #[unsafe(method(quitApplication:))]
        fn quit_application(&self, _sender: &NSMenuItem) {
            if let Err(error) = crate::renderer::quit_macos_application() {
                log::error!("Failed to quit the Solo application: {error}");
            }
        }
    }
);

define_class!(
    // SAFETY: the Rust ivars own the callback and token for the lifetime of
    // this AppKit target. NSMenu dispatches actions on the main thread.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = ActionTargetIvars]
    struct SoloMenuBarActionTarget;

    unsafe impl NSObjectProtocol for SoloMenuBarActionTarget {}

    impl SoloMenuBarActionTarget {
        #[unsafe(method(invoke:))]
        fn invoke(&self, _sender: &NSMenuItem) {
            crate::renderer::emit_event_full(
                &self.ivars().callback,
                0,
                MENU_BAR_ACTION_EVENT,
                |event| event.value = Some(self.ivars().token.clone()),
            );
        }
    }
);

impl SoloMenuBarTarget {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(TargetIvars);
        unsafe { msg_send![super(this), init] }
    }
}

impl SoloMenuBarActionTarget {
    fn new(
        mtm: MainThreadMarker,
        token: String,
        callback: Option<EventCallback>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ActionTargetIvars { token, callback });
        unsafe { msg_send![super(this), init] }
    }
}

/// Owns every weakly referenced AppKit object for one status item. The item
/// stays in the status bar until the process exits.
pub(crate) struct MenuBar {
    status_item: Retained<NSStatusItem>,
    target: Retained<SoloMenuBarTarget>,
    action_targets: Vec<Retained<SoloMenuBarActionTarget>>,
    application_title: String,
    initial_icon_path: String,
    initial_tooltip: Option<String>,
}

impl MenuBar {
    pub(crate) fn new(
        options: &crate::renderer::MenuBarOptions,
        application_title: &str,
    ) -> Result<Self> {
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::from_reason("Menu-bar items must be created on the main thread")
        })?;
        let image = load_image(&options.icon_path)?;

        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let button = status_item
            .button(mtm)
            .ok_or_else(|| Error::from_reason("macOS did not create a menu-bar button"))?;
        button.setImage(Some(&image));
        if let Some(tooltip) = options.tooltip.as_deref() {
            button.setToolTip(Some(&NSString::from_str(tooltip)));
        }

        let target = SoloMenuBarTarget::new(mtm);
        let (menu, action_targets) = build_menu(mtm, application_title, &target, &[], None);
        status_item.setMenu(Some(&menu));

        Ok(Self {
            status_item,
            target,
            action_targets,
            application_title: application_title.to_string(),
            initial_icon_path: options.icon_path.clone(),
            initial_tooltip: options.tooltip.clone(),
        })
    }

    pub(crate) fn update(
        &mut self,
        json: Option<&str>,
        callback: Option<EventCallback>,
    ) -> Result<()> {
        let update = json.map(parse_update).transpose()?;
        let mtm = MainThreadMarker::new().ok_or_else(|| {
            Error::from_reason("Menu-bar items must be updated on the main thread")
        })?;
        let icon_path = update
            .as_ref()
            .and_then(|update| update.icon_path.as_deref())
            .unwrap_or(&self.initial_icon_path);
        let image = load_image(icon_path)?;
        let tooltip = update
            .as_ref()
            .and_then(|update| update.tooltip.as_ref())
            .cloned()
            .unwrap_or_else(|| self.initial_tooltip.clone());
        let items = update
            .as_ref()
            .map(|update| update.items.as_slice())
            .unwrap_or_default();
        let (menu, action_targets) =
            build_menu(mtm, &self.application_title, &self.target, items, callback);

        let button = self
            .status_item
            .button(mtm)
            .ok_or_else(|| Error::from_reason("macOS menu-bar button is unavailable"))?;
        button.setImage(Some(&image));
        button.setToolTip(tooltip.as_deref().map(NSString::from_str).as_deref());
        self.status_item.setMenu(Some(&menu));
        self.action_targets = action_targets;
        Ok(())
    }
}

fn load_image(path: &str) -> Result<Retained<NSImage>> {
    let image_path = NSString::from_str(path);
    let image = NSImage::initWithContentsOfFile(NSImage::alloc(), &image_path)
        .ok_or_else(|| Error::from_reason(format!("Failed to load menu-bar icon at {path:?}")))?;
    image.setTemplate(true);
    Ok(image)
}

fn build_menu(
    mtm: MainThreadMarker,
    application_title: &str,
    target: &SoloMenuBarTarget,
    items: &[MenuBarItem],
    callback: Option<EventCallback>,
) -> (Retained<NSMenu>, Vec<Retained<SoloMenuBarActionTarget>>) {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(application_title));
    // Preserve application-provided disabled state. AppKit otherwise asks the
    // target whether it implements the selector and silently re-enables it.
    menu.setAutoenablesItems(false);
    let empty = NSString::from_str("");
    let mut action_targets = Vec::new();
    for item in items {
        match item {
            MenuBarItem::Separator => menu.addItem(&NSMenuItem::separatorItem(mtm)),
            MenuBarItem::Status { label } => {
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        None,
                        &empty,
                    )
                };
                item.setEnabled(false);
                menu.addItem(&item);
            }
            MenuBarItem::Action {
                token,
                label,
                enabled,
                checked,
            } => {
                let action_target =
                    SoloMenuBarActionTarget::new(mtm, token.clone(), callback.clone());
                let item = unsafe {
                    NSMenuItem::initWithTitle_action_keyEquivalent(
                        NSMenuItem::alloc(mtm),
                        &NSString::from_str(label),
                        Some(sel!(invoke:)),
                        &empty,
                    )
                };
                unsafe { item.setTarget(Some(&action_target)) };
                item.setEnabled(*enabled);
                item.setState(if *checked {
                    NSControlStateValueOn
                } else {
                    NSControlStateValueOff
                });
                menu.addItem(&item);
                action_targets.push(action_target);
            }
        }
    }
    if !items.is_empty() {
        menu.addItem(&NSMenuItem::separatorItem(mtm));
    }
    let open = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(&format!("Open {application_title}")),
            Some(sel!(openWindow:)),
            &empty,
        )
    };
    unsafe { open.setTarget(Some(target)) };
    menu.addItem(&open);
    menu.addItem(&NSMenuItem::separatorItem(mtm));
    let quit = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(&format!("Quit {application_title}")),
            Some(sel!(quitApplication:)),
            &empty,
        )
    };
    unsafe { quit.setTarget(Some(target)) };
    menu.addItem(&quit);
    (menu, action_targets)
}
