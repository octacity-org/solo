//! Narrow desktop capabilities, independent of a mounted renderer.

use napi::{Error, Result};
use napi_derive::napi;

fn validate_browser_url(input: &str) -> Result<String> {
    let invalid = || {
        Error::from_reason(
            "Expected an absolute HTTP/HTTPS URL without credentials, whitespace, or backslashes",
        )
    };
    if input
        .chars()
        .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return Err(invalid());
    }
    // WHATWG parsing repairs forms such as https:host and https:///host. Do
    // not silently turn those ambiguous inputs into an external navigation.
    let (_, authority) = input.split_once("://").ok_or_else(invalid)?;
    if authority.is_empty() || authority.starts_with(['/', '?', '#']) {
        return Err(invalid());
    }
    let url = url::Url::parse(input).map_err(|_| invalid())?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(invalid());
    }
    Ok(url.into())
}

#[cfg(any(target_os = "macos", test))]
fn platform_result(accepted: bool, operation: &str) -> Result<()> {
    if accepted {
        Ok(())
    } else {
        Err(Error::from_reason(format!("macOS rejected {operation}")))
    }
}

/// Ask macOS to open an absolute HTTP/HTTPS URL in its default browser.
/// Success means the OS accepted the request, not that the page loaded.
#[napi]
pub fn open_external_url(url: String) -> Result<()> {
    let url = validate_browser_url(&url)?;
    #[cfg(target_os = "macos")]
    {
        if crate::renderer::worker_main::must_forward() {
            return crate::renderer::on_main(move || open_external_url(url));
        }
        macos::require_main_thread()?;
        let url =
            objc2_foundation::NSURL::URLWithString(&objc2_foundation::NSString::from_str(&url))
                .ok_or_else(|| Error::from_reason("macOS could not represent the URL"))?;
        platform_result(
            objc2_app_kit::NSWorkspace::sharedWorkspace().openURL(&url),
            "opening the browser URL",
        )
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err(Error::from_reason(
            "External browser opening is supported only on macOS",
        ))
    }
}

/// Replace the system clipboard with plain Unicode text. macOS only.
#[napi]
pub fn write_clipboard_text(text: String) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        if crate::renderer::worker_main::must_forward() {
            return crate::renderer::on_main(move || write_clipboard_text(text));
        }
        macos::require_main_thread()?;
        macos::write_text(&objc2_app_kit::NSPasteboard::generalPasteboard(), &text)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = text;
        Err(Error::from_reason(
            "Clipboard text writing is supported only on macOS",
        ))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    pub(super) fn require_main_thread() -> Result<()> {
        objc2::MainThreadMarker::new()
            .map(|_| ())
            .ok_or_else(|| Error::from_reason("Desktop actions must run on the macOS main thread"))
    }

    pub(super) fn write_text(board: &NSPasteboard, text: &str) -> Result<()> {
        let text = NSString::from_str(text);
        // GPUI's clipboard wrapper uses this same pasteboard, but discards
        // the write result. Preserve it so callers can handle a refused write.
        board.clearContents();
        // SAFETY: AppKit's process-lifetime plain-text pasteboard type constant.
        platform_result(
            board.setString_forType(&text, unsafe { NSPasteboardTypeString }),
            "writing clipboard text",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_urls_are_absolute_http_or_https_and_canonicalized() {
        for (input, expected) in [
            (
                "https://example.com/article?q=a&b=1#section",
                "https://example.com/article?q=a&b=1#section",
            ),
            ("HTTP://localhost:8000", "http://localhost:8000/"),
            (
                "https://example.com/世界",
                "https://example.com/%E4%B8%96%E7%95%8C",
            ),
            (
                "https://example.com/?q=$(echo);'",
                "https://example.com/?q=$(echo);%27",
            ),
        ] {
            assert_eq!(validate_browser_url(input).unwrap(), expected);
        }
    }

    #[test]
    fn rejects_unsupported_and_ambiguous_browser_urls() {
        for input in [
            "",
            "/article",
            "example.com",
            "//example.com",
            "https:example.com",
            "https:///example.com",
            "https://",
            "https://?q=hello",
            "https://[bad]",
            "https://example.com:99999",
            "https://example.com/a b",
            " https://example.com",
            "https://example.com\n",
            "https://example.com/\0x",
            "https:\\example.com",
            "https://user:secret@example.com",
            "file:///tmp/article",
            "javascript:alert(1)",
            "data:text/html,hi",
            "mailto:reader@example.com",
            "solo://command",
        ] {
            assert!(validate_browser_url(input).is_err(), "accepted {input:?}");
        }
    }

    #[test]
    fn platform_rejection_is_a_catchable_error_without_input_data() {
        assert!(platform_result(true, "Open URL").is_ok());
        let error = platform_result(false, "Open URL").unwrap_err();
        assert!(error.to_string().contains("Open URL"));
        assert!(error.to_string().contains("macOS"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unicode_clipboard_text_round_trips_on_a_private_pasteboard() {
        use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
        // This pasteboard is private to the test: never alter the user's clipboard.
        let board = NSPasteboard::pasteboardWithUniqueName();
        for text in [
            "Hello 世界 👋\nİstanbul",
            "",
            "https://example.com/?q=';$()",
            "a\0b",
        ] {
            macos::write_text(&board, text).unwrap();
            let actual = board
                .stringForType(unsafe { NSPasteboardTypeString })
                .unwrap();
            assert_eq!(actual.to_string(), text);
        }
        board.clearContents();
    }
}
