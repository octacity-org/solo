//! System notifications in the macOS Notification Center, through GPUI's
//! `UNUserNotificationCenter` support (`App::show_system_notification`), on
//! Linux over the D-Bus Desktop Notifications specification (`notify-rust`,
//! posted by Solo itself; see `linux`), and on Windows via WinRT Toast
//! Notifications.
//!
//! GPUI asks for authorization the first time a notification is posted and
//! delivers nothing once the user has denied it. It also disables
//! notifications on macOS in a process without a bundle identifier, because
//! `UNUserNotificationCenter` aborts outside an app bundle; Solo checks the
//! same condition first and throws instead of posting nothing.
//!
//! On Linux, notifications are posted over the session bus to the running
//! desktop notification daemon. If no daemon owns `org.freedesktop.Notifications`,
//! posting fails immediately with an error rather than silently dropping.
//!
//! Responses (a click on the body or on an action button) reach JS as an
//! element-0 `notificationResponse` event whose `value` is
//! `{"tag":…,"actionId":…}`. The observer is installed during startup,
//! so a response that launched the app waits here until a renderer is
//! initialized, as opened URLs do (`open_urls.rs`).

#![cfg_attr(not(any(target_os = "macos", ui_thread)), allow(dead_code))]

use napi::{Error, Result};
use napi_derive::napi;

/// A button offered on a notification.
#[napi(object)]
pub struct NotificationAction {
    /// Reported back as the response's `actionId`.
    pub id: String,
    /// The button's label.
    pub label: String,
}

/// What `showNotification` posts.
#[napi(object)]
pub struct NotificationOptions {
    /// Identifies the notification: posting the same tag replaces it, and
    /// responses carry it back.
    pub tag: String,
    pub title: String,
    pub body: Option<String>,
    pub actions: Option<Vec<NotificationAction>>,
}

/// Element-0 event: the user clicked a notification or one of its buttons.
#[cfg(any(target_os = "macos", ui_thread))]
const NOTIFICATION_RESPONSE_EVENT: &str = "notificationResponse";

/// Check the options and build GPUI's notification from them.
fn system_notification(options: NotificationOptions) -> Result<gpui::SystemNotification> {
    let NotificationOptions { tag, title, body, actions } = options;
    if tag.is_empty() {
        return Err(Error::from_reason("A notification needs a non-empty tag"));
    }
    if title.is_empty() {
        return Err(Error::from_reason("A notification needs a non-empty title"));
    }
    let actions = actions.unwrap_or_default();
    let mut ids = std::collections::HashSet::new();
    for action in &actions {
        if action.id.is_empty() || action.label.is_empty() {
            return Err(Error::from_reason(
                "A notification action needs a non-empty id and label",
            ));
        }
        if !ids.insert(action.id.as_str()) {
            return Err(Error::from_reason(format!(
                "Notification action ids must be unique; {:?} repeats",
                action.id
            )));
        }
    }
    Ok(gpui::SystemNotification {
        tag: tag.into(),
        title: title.into(),
        body: body.unwrap_or_default().into(),
        actions: actions
            .into_iter()
            .map(|action| gpui::SystemNotificationAction {
                id: action.id.into(),
                label: action.label.into(),
            })
            .collect(),
    })
}

#[cfg(target_os = "macos")]
thread_local! {
    /// Main thread only, where GPUI hands over responses and `init` runs.
    static PENDING: std::cell::RefCell<Vec<gpui::SystemNotificationResponse>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(ui_thread)]
static PENDING: std::sync::Mutex<Vec<gpui::SystemNotificationResponse>> =
    std::sync::Mutex::new(Vec::new());

/// Report notification responses to the current app renderer for the life
/// of the process. Installed once at startup, so it is in place
/// when a click on a notification launches the app.
#[cfg(target_os = "macos")]
pub(crate) fn observe(cx: &mut gpui::App) {
    cx.on_system_notification_response(|response, _cx| {
        PENDING.with(|pending| pending.borrow_mut().push(response));
        deliver_pending();
    });
}

#[cfg(ui_thread)]
pub(crate) fn observe(cx: &mut gpui::App) {
    cx.on_system_notification_response(|response, _cx| {
        PENDING.lock().unwrap().push(response);
        deliver_pending();
    });
}

#[cfg(not(any(target_os = "macos", ui_thread)))]
pub(crate) fn observe(_cx: &mut gpui::App) {}

/// Hand any held responses to the app renderer, if one is initialized.
/// Called on arrival and after `init`, including a `solo dev` reload's.
#[cfg(target_os = "macos")]
pub(crate) fn deliver_pending() {
    let Some(callback) = crate::renderer::app_event_callback() else {
        return;
    };
    let responses = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    deliver_responses(&callback, responses);
}

#[cfg(ui_thread)]
pub(crate) fn deliver_pending() {
    let Some(callback) = crate::renderer::app_event_callback() else {
        return;
    };
    let responses = std::mem::take(&mut *PENDING.lock().unwrap());
    deliver_responses(&callback, responses);
}

#[cfg(not(any(target_os = "macos", ui_thread)))]
pub(crate) fn deliver_pending() {}

#[cfg(any(target_os = "macos", ui_thread))]
fn deliver_responses(
    callback: &crate::renderer::EventCallback,
    responses: Vec<gpui::SystemNotificationResponse>,
) {
    for response in responses {
        let value = serde_json::json!({
            "tag": response.tag.as_ref(),
            "actionId": response.action_id.as_ref().map(|id| id.as_ref()),
        })
        .to_string();
        crate::renderer::emit_event_full(
            &Some(callback.clone()),
            0,
            NOTIFICATION_RESPONSE_EVENT,
            |payload| payload.value = Some(value),
        );
    }
}

/// macOS delivers notifications only to an app bundle; GPUI turns them off
/// without one, using the same test.
#[cfg(target_os = "macos")]
fn require_app_bundle(action: &str) -> Result<()> {
    if objc2_foundation::NSBundle::mainBundle()
        .bundleIdentifier()
        .is_none()
    {
        return Err(Error::from_reason(format!(
            "{action} needs a packaged .app with a CFBundleIdentifier: macOS delivers notifications only to app bundles"
        )));
    }
    Ok(())
}

/// Check that a desktop notification daemon owns `org.freedesktop.Notifications`
/// on the D-Bus session bus.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
fn check_notification_daemon() -> Result<()> {
    notify_rust::get_server_information().map_err(|error| {
        Error::from_reason(format!(
            "No notification daemon available on the session bus: {error}"
        ))
    })?;
    Ok(())
}

/// Linux posts notifications itself rather than through GPUI. GPUI's Linux
/// backend never sends a replacement id and loses the server-assigned one,
/// so it can neither replace nor close a notification by tag. Solo keeps
/// each tag's id: posting the tag again passes it as `replaces_id`, and
/// `dismissNotification` closes it.
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
pub(crate) mod linux {
    use gpui::{SharedString, SystemNotification, SystemNotificationResponse};
    use napi::{Error, Result};
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};

    /// The XDG action key for a click on the notification body.
    const DEFAULT_ACTION: &str = "default";

    /// The notification a tag shows, with what closes it (`H`).
    struct Shown<H> {
        /// Assigned by the server.
        id: u32,
        handle: H,
        /// Transport action key → the app's action id.
        actions: HashMap<String, SharedString>,
        /// A thread is waiting for this id's response.
        listening: bool,
    }

    /// Tag → the notification shown for it.
    pub(super) struct Registry<H>(HashMap<String, Shown<H>>);

    impl<H> Default for Registry<H> {
        fn default() -> Self {
            Self(HashMap::new())
        }
    }

    impl<H> Registry<H> {
        /// The id a new notification for `tag` replaces.
        pub(super) fn replaces_id(&self, tag: &str) -> Option<u32> {
            self.0.get(tag).map(|shown| shown.id)
        }

        /// Record what the server shows for `tag`. True when nothing is
        /// waiting for `id`'s response yet: the server assigned a new id, or
        /// the listener for this one has stopped.
        pub(super) fn posted(
            &mut self,
            tag: &str,
            id: u32,
            handle: H,
            actions: HashMap<String, SharedString>,
        ) -> bool {
            let listening = self
                .0
                .get(tag)
                .is_some_and(|shown| shown.id == id && shown.listening);
            self.0.insert(
                tag.to_owned(),
                Shown { id, handle, actions, listening: true },
            );
            !listening
        }

        /// The app's response to `key` on notification `id`. `None` for a
        /// button the tag no longer shows.
        pub(super) fn response(
            &self,
            tag: &str,
            id: u32,
            key: &str,
        ) -> Option<SystemNotificationResponse> {
            let action_id = if key == DEFAULT_ACTION {
                None
            } else {
                let shown = self.0.get(tag).filter(|shown| shown.id == id)?;
                Some(shown.actions.get(key)?.clone())
            };
            Some(SystemNotificationResponse { tag: tag.to_owned().into(), action_id })
        }

        /// The server closed notification `id`.
        pub(super) fn closed(&mut self, tag: &str, id: u32) {
            if self.0.get(tag).is_some_and(|shown| shown.id == id) {
                self.0.remove(tag);
            }
        }

        /// The listener for `id` returned after its one response. After a
        /// click it does not see the server close the notification, so the
        /// entry stays; replacing or closing that stale id is harmless.
        pub(super) fn stopped(&mut self, tag: &str, id: u32) {
            if let Some(shown) = self.0.get_mut(tag).filter(|shown| shown.id == id) {
                shown.listening = false;
            }
        }

        /// Forget `tag`, returning what closes its notification.
        pub(super) fn dismissed(&mut self, tag: &str) -> Option<H> {
            self.0.remove(tag).map(|shown| shown.handle)
        }
    }

    static SHOWN: LazyLock<Mutex<Registry<notify_rust::NotificationHandle>>> =
        LazyLock::new(Mutex::default);

    /// Shown by the server as the sender; set by `init`, as GPUI's was.
    static APP_NAME: Mutex<Option<String>> = Mutex::new(None);

    pub(crate) fn set_app_name(name: &str) {
        *APP_NAME.lock().unwrap() = Some(name.to_owned());
    }

    pub(super) fn show(notification: SystemNotification) -> Result<()> {
        let SystemNotification { tag, title, body, actions } = notification;
        let mut builder = notify_rust::Notification::new();
        if let Some(name) = APP_NAME.lock().unwrap().as_deref() {
            builder.appname(name);
        }
        builder
            .summary(&title)
            .body(&body)
            .action(DEFAULT_ACTION, DEFAULT_ACTION);
        // Transport keys of Solo's own, so an app's action id can never be
        // read as `default`.
        let mut keys = HashMap::new();
        for (index, action) in actions.iter().enumerate() {
            let key = format!("solo-action-{index}");
            builder.action(&key, &action.label);
            keys.insert(key, action.id.clone());
        }

        // Held across the post, so two posts of one tag cannot race.
        let mut shown = SHOWN.lock().unwrap();
        if let Some(id) = shown.replaces_id(&tag) {
            builder.id(id);
        }
        let handle = builder.show().map_err(|error| {
            Error::from_reason(format!("Could not post the notification: {error}"))
        })?;
        let id = handle.id();
        if shown.posted(&tag, id, handle, keys) {
            listen(tag, id)?;
        }
        Ok(())
    }

    /// Close the notification shown for `tag`, if any. The server may have
    /// closed it already; that is not an error.
    pub(super) fn dismiss(tag: &str) {
        let handle = SHOWN.lock().unwrap().dismissed(tag);
        if let Some(handle) = handle {
            handle.close();
        }
    }

    /// Wait on a thread of its own for notification `id`'s one response: a
    /// click, a button, or the server closing it.
    fn listen(tag: SharedString, id: u32) -> Result<()> {
        std::thread::Builder::new()
            .name("solo-notification".to_owned())
            .spawn(move || {
                let _ = notify_rust::handle_action(id, |response| {
                    let response = match response {
                        notify_rust::ActionResponse::Custom(key) => {
                            SHOWN.lock().unwrap().response(&tag, id, key)
                        }
                        notify_rust::ActionResponse::Closed(_) => {
                            SHOWN.lock().unwrap().closed(&tag, id);
                            None
                        }
                    };
                    if let Some(response) = response {
                        super::PENDING.lock().unwrap().push(response);
                        super::deliver_pending();
                    }
                });
                SHOWN.lock().unwrap().stopped(&tag, id);
            })
            .map(drop)
            .map_err(|error| {
                Error::from_reason(format!(
                    "Could not wait for notification responses: {error}"
                ))
            })
    }
}

#[cfg(not(any(target_os = "macos", ui_thread)))]
fn unsupported(action: &str) -> Error {
    Error::from_reason(format!("{action} is supported only on macOS, Linux and Windows"))
}

/// Post a notification to the Notification Center. Posting a tag that is
/// already shown replaces that notification. The first post asks the user for
/// permission; once denied, nothing is shown.
#[napi]
pub fn show_notification(options: NotificationOptions) -> Result<()> {
    let notification = system_notification(options)?;
    #[cfg(target_os = "macos")]
    {
        require_app_bundle("showNotification")?;
        crate::application::with_app("showNotification", move |cx| {
            cx.show_system_notification(notification)
        })
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        check_notification_daemon()?;
        linux::show(notification)
    }
    #[cfg(target_os = "windows")]
    {
        crate::renderer::with_ui_app("showNotification", move |cx| {
            cx.show_system_notification(notification)
        })
    }
    #[cfg(not(any(target_os = "macos", ui_thread)))]
    {
        let _ = notification;
        Err(unsupported("showNotification"))
    }
}

/// Remove the notification posted with `tag`, shown or still pending.
#[napi]
pub fn dismiss_notification(tag: String) -> Result<()> {
    if tag.is_empty() {
        return Err(Error::from_reason("A notification needs a non-empty tag"));
    }
    #[cfg(target_os = "macos")]
    {
        require_app_bundle("dismissNotification")?;
        crate::application::with_app("dismissNotification", move |cx| {
            cx.dismiss_system_notification(&tag)
        })
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        linux::dismiss(&tag);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        crate::renderer::with_ui_app("dismissNotification", move |cx| {
            cx.dismiss_system_notification(&tag)
        })
    }
    #[cfg(not(any(target_os = "macos", ui_thread)))]
    {
        Err(unsupported("dismissNotification"))
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "freebsd")))]
mod linux_tests {
    use super::linux::Registry;
    use gpui::{SharedString, SystemNotificationResponse};
    use std::collections::HashMap;

    fn actions(pairs: &[(&str, &str)]) -> HashMap<String, SharedString> {
        pairs
            .iter()
            .map(|(key, id)| (key.to_string(), SharedString::from(id.to_string())))
            .collect()
    }

    fn response(tag: &str, action_id: Option<&str>) -> Option<SystemNotificationResponse> {
        Some(SystemNotificationResponse {
            tag: tag.to_string().into(),
            action_id: action_id.map(|id| id.to_string().into()),
        })
    }

    #[test]
    fn reposting_a_tag_replaces_its_notification() {
        let mut registry = Registry::default();
        assert_eq!(registry.replaces_id("export"), None);
        assert!(registry.posted("export", 7, "first", actions(&[])));
        assert_eq!(registry.replaces_id("export"), Some(7));
        // The server kept id 7: its listener is still waiting.
        assert!(!registry.posted("export", 7, "second", actions(&[])));
        assert_eq!(registry.dismissed("export"), Some("second"));
        assert_eq!(registry.dismissed("export"), None);
    }

    #[test]
    fn a_new_server_id_or_a_stopped_listener_needs_a_listener() {
        let mut registry = Registry::default();
        registry.posted("export", 7, (), actions(&[]));
        assert!(registry.posted("export", 8, (), actions(&[])));
        registry.stopped("export", 7);
        assert!(!registry.posted("export", 8, (), actions(&[])));
        registry.stopped("export", 8);
        assert!(registry.posted("export", 8, (), actions(&[])));
    }

    #[test]
    fn maps_transport_keys_to_the_current_action_ids() {
        let mut registry = Registry::default();
        registry.posted("export", 7, (), actions(&[("solo-action-0", "open")]));
        assert_eq!(registry.response("export", 7, "default"), response("export", None));
        assert_eq!(registry.response("export", 7, "solo-action-0"), response("export", Some("open")));
        assert_eq!(registry.response("export", 7, "solo-action-1"), None);
        // A replacement with the same id brings its own buttons.
        registry.posted("export", 7, (), actions(&[("solo-action-0", "retry")]));
        assert_eq!(registry.response("export", 7, "solo-action-0"), response("export", Some("retry")));
        // A button of a notification the tag no longer shows.
        registry.posted("export", 8, (), actions(&[("solo-action-0", "open")]));
        assert_eq!(registry.response("export", 7, "solo-action-0"), None);
    }

    #[test]
    fn closing_forgets_only_the_closed_notification() {
        let mut registry = Registry::default();
        registry.posted("export", 7, (), actions(&[]));
        registry.posted("export", 8, (), actions(&[]));
        registry.closed("export", 7);
        assert_eq!(registry.replaces_id("export"), Some(8));
        registry.closed("export", 8);
        assert_eq!(registry.replaces_id("export"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(actions: &[(&str, &str)]) -> NotificationOptions {
        NotificationOptions {
            tag: "download-1".into(),
            title: "Download finished".into(),
            body: None,
            actions: Some(
                actions
                    .iter()
                    .map(|(id, label)| NotificationAction {
                        id: id.to_string(),
                        label: label.to_string(),
                    })
                    .collect(),
            ),
        }
    }

    fn reason(options: NotificationOptions) -> String {
        match system_notification(options) {
            Ok(_) => panic!("expected an error"),
            Err(error) => error.reason.clone(),
        }
    }

    #[test]
    fn builds_the_gpui_notification() {
        let notification = system_notification(options(&[("open", "Open"), ("show", "Show in Finder")]))
            .expect("valid options");
        assert_eq!(notification.tag.as_ref(), "download-1");
        assert_eq!(notification.title.as_ref(), "Download finished");
        assert_eq!(notification.body.as_ref(), "");
        let ids: Vec<&str> = notification.actions.iter().map(|a| a.id.as_ref()).collect();
        assert_eq!(ids, ["open", "show"]);
        assert_eq!(notification.actions[1].label.as_ref(), "Show in Finder");
    }

    #[test]
    fn rejects_unusable_options() {
        let mut empty_tag = options(&[]);
        empty_tag.tag.clear();
        assert!(reason(empty_tag).contains("non-empty tag"));
        let mut empty_title = options(&[]);
        empty_title.title.clear();
        assert!(reason(empty_title).contains("non-empty title"));
        assert!(reason(options(&[("", "Open")])).contains("non-empty id and label"));
        assert!(reason(options(&[("open", "")])).contains("non-empty id and label"));
        assert!(reason(options(&[("open", "Open"), ("open", "Again")])).contains("\"open\" repeats"));
    }
}
