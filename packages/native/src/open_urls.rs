//! URLs macOS asks the app to open: custom-scheme deep links such as
//! `myapp://item/42`, and files opened with the app (as `file://` URLs).
//! Also registers this app as a scheme's default handler.
//!
//! AppKit delivers URLs on the main thread through GPUI's `on_open_urls`,
//! installed in `run_main_loop` before the app Worker exists. They reach JS
//! as an element-0 `openUrls` event on the app renderer's callback. URLs that
//! arrive before a renderer is initialized (a cold launch by URL) wait here
//! until `init` hands them over.

use napi::bindgen_prelude::{AsyncTask, Task};
use napi::{Error, Result};
use napi_derive::napi;

/// Element-0 event whose `urls` lists what macOS asked the app to open.
#[cfg(target_os = "macos")]
const OPEN_URLS_EVENT: &str = "openUrls";

#[cfg(target_os = "macos")]
thread_local! {
    /// Main thread only, where AppKit delivers URLs and `init` runs.
    static PENDING: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// AppKit asked the app to open `urls`: deliver them now, or hold them until
/// an app renderer can receive them.
#[cfg(target_os = "macos")]
pub(crate) fn receive(urls: Vec<String>) {
    PENDING.with(|pending| pending.borrow_mut().extend(urls));
    deliver_pending();
}

/// Hand any held URLs to the app renderer, if one is initialized. Called on
/// arrival and after `init`, including a `solo dev` reload's.
#[cfg(target_os = "macos")]
pub(crate) fn deliver_pending() {
    let Some(callback) = crate::renderer::app_event_callback() else {
        return;
    };
    let urls = PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()));
    if urls.is_empty() {
        return;
    }
    crate::renderer::emit_event_full(&Some(callback), 0, OPEN_URLS_EVENT, |payload| {
        payload.urls = Some(urls);
    });
}

/// Schemes an app must not claim: taking them over would redirect the
/// browser, file opening or mail for the whole user account.
const RESERVED_SCHEMES: &[&str] = &["http", "https", "file", "mailto", "ftp", "data", "javascript", "about"];

/// A scheme name per RFC 3986 (`ALPHA *( ALPHA / DIGIT / "+" / "-" / "." )`),
/// without `://`, lowercased. Reserved system schemes are rejected.
fn validate_scheme(scheme: &str) -> Result<String> {
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if !valid {
        return Err(Error::from_reason(format!(
            "Invalid URL scheme {scheme:?}: use a letter followed by letters, digits, '+', '-' or '.', without \"://\""
        )));
    }
    let scheme = scheme.to_ascii_lowercase();
    if RESERVED_SCHEMES.contains(&scheme.as_str()) {
        return Err(Error::from_reason(format!(
            "The {scheme:?} URL scheme belongs to the system and cannot be registered"
        )));
    }
    Ok(scheme)
}

pub struct RegisterUrlScheme {
    scheme: String,
}

impl Task for RegisterUrlScheme {
    type Output = ();
    type JsValue = ();

    /// Runs on a libuv worker thread: starts the registration on the GPUI
    /// main loop and waits for macOS to answer.
    fn compute(&mut self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let scheme = self.scheme.clone();
            let (sender, receiver) = std::sync::mpsc::sync_channel::<std::result::Result<(), String>>(1);
            if !crate::renderer::worker_main::must_forward() {
                return Err(Error::from_reason(
                    "registerUrlScheme needs a running Solo app (worker mode)",
                ));
            }
            // A failure to reach the app drops `sender`: reported below.
            crate::renderer::on_main_async(move || {
                crate::renderer::app_update(|cx| {
                    let registration = cx.register_url_scheme(&scheme);
                    cx.spawn(async move |_cx| {
                        sender.send(registration.await.map_err(|error| error.to_string())).ok();
                    })
                    .detach();
                })
                .ok();
            })?;
            receiver
                .recv_timeout(std::time::Duration::from_secs(30))
                .map_err(|error| match error {
                    std::sync::mpsc::RecvTimeoutError::Timeout => {
                        Error::from_reason("Timed out waiting for macOS to register the URL scheme")
                    }
                    std::sync::mpsc::RecvTimeoutError::Disconnected => {
                        Error::from_reason("GPUI application is not initialized")
                    }
                })?
                .map_err(Error::from_reason)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Error::from_reason(
                "URL scheme registration is supported only on macOS",
            ))
        }
    }

    fn resolve(&mut self, _env: napi::Env, _output: ()) -> Result<()> {
        Ok(())
    }
}

/// Make this app the default handler for `scheme` (macOS 12+). The app must
/// run from a bundle with an identifier that Launch Services knows, and
/// should declare the scheme in its `Info.plist` (`CFBundleURLTypes`).
#[napi(ts_return_type = "Promise<void>")]
pub fn register_url_scheme(scheme: String) -> Result<AsyncTask<RegisterUrlScheme>> {
    let scheme = validate_scheme(&scheme)?;
    Ok(AsyncTask::new(RegisterUrlScheme { scheme }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_rfc_3986_scheme_names_and_lowercases_them() {
        for (input, expected) in [
            ("myapp", "myapp"),
            ("MyApp", "myapp"),
            ("x-solo.notes+v2", "x-solo.notes+v2"),
            ("a1", "a1"),
        ] {
            assert_eq!(validate_scheme(input).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_malformed_and_system_schemes() {
        for input in [
            "", "1app", "-app", "my app", "myapp://", "myapp:", "my_app", "app/x", "ünïcode", "HTTPS",
            "file", "mailto", "javascript",
        ] {
            assert!(validate_scheme(input).is_err(), "accepted {input:?}");
        }
    }
}
