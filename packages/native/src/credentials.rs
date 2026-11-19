//! Secure credential storage for Solo applications.
//!
//! - Linux: backed by the Secret Service API (GNOME Keyring, KWallet) over D-Bus
//!   via the `oo7` crate (Issue #128). Every item is labeled with the app's identifier
//!   and keyed with `solo-app` and `url` attributes so that separate Solo apps and
//!   Zed credentials never collide or overwrite each other.
//! - macOS: backed by the macOS Keychain (Issue #43). Items are internet
//!   passwords whose server is `solo:<app_id>:<url>`, partitioned like Windows.
//!
//! All operations run asynchronously off the UI thread via napi `AsyncTask`
//! so the desktop stays responsive. Secrets are never logged or included in error
//! messages.

use napi::bindgen_prelude::{AsyncTask, Result, Task};
use napi::Error;
use napi_derive::napi;

/// A stored credential record. Secrets are never printed in Debug logs.
#[derive(Clone)]
#[napi(object)]
pub struct CredentialResult {
    pub username: String,
    pub secret: String,
}

impl std::fmt::Debug for CredentialResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialResult")
            .field("username", &self.username)
            .field("secret", &"[REDACTED]")
            .finish()
    }
}

fn validate_url(url: &str) -> Result<()> {
    if url.trim().is_empty() {
        return Err(Error::from_reason("Credential URL must not be empty"));
    }
    Ok(())
}

fn validate_username(username: &str) -> Result<()> {
    if username.trim().is_empty() {
        return Err(Error::from_reason("Credential username must not be empty"));
    }
    Ok(())
}

fn resolve_app_id(app_id: Option<String>) -> Result<String> {
    app_id
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| Error::from_reason("Credential operations require an app identifier"))
}

// ---------------------------------------------------------------------------
// Linux / FreeBSD (Secret Service via oo7)
// ---------------------------------------------------------------------------

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
mod linux {
    use super::*;

    fn map_oo7_error(err: oo7::Error, url: &str) -> Error {
        let msg = err.to_string();
        if msg.contains("ServiceUnknown")
            || msg.contains("org.freedesktop.secrets")
            || msg.contains("No such interface")
            || msg.contains("no secret service")
            || msg.contains("NoService")
        {
            Error::from_reason("No Secret Service provider available on the session bus")
        } else if msg.contains("dismissed")
            || msg.contains("cancelled")
            || msg.contains("canceled")
            || msg.contains("Prompt")
            || msg.contains("Dismissed")
            || msg.contains("Cancelled")
        {
            Error::from_reason("Keyring unlock prompt was dismissed")
        } else {
            Error::from_reason(format!("Keyring operation failed for {url}: {msg}"))
        }
    }

    pub fn write(url: &str, username: &str, secret: &[u8], app_id: &str) -> Result<()> {
        futures::executor::block_on(async {
            let keyring = oo7::Keyring::new()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            keyring
                .unlock()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            let label = format!("{app_id} ({url})");
            let attributes = [
                ("solo-app", app_id),
                ("url", url),
                ("username", username),
            ];

            // Create or replace the item with the exact attributes first
            keyring
                .create_item(&label, &attributes, secret, true)
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            // Clean up any old items for this (app_id, url) whose username differs
            let search_attributes = [
                ("solo-app", app_id),
                ("url", url),
            ];
            if let Ok(existing) = keyring.search_items(&search_attributes).await {
                for item in existing {
                    if let Ok(attrs) = item.attributes().await {
                        if attrs.get("username").map(|s| s.as_str()) != Some(username) {
                            let _ = item.delete().await;
                        }
                    }
                }
            }

            Ok(())
        })
    }

    pub fn read(url: &str, app_id: &str) -> Result<Option<CredentialResult>> {
        futures::executor::block_on(async {
            let keyring = oo7::Keyring::new()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            keyring
                .unlock()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            let attributes = [
                ("solo-app", app_id),
                ("url", url),
            ];

            let items = keyring
                .search_items(&attributes)
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            for item in items {
                let attrs = match item.attributes().await {
                    Ok(a) => a,
                    Err(_) => continue,
                };

                if attrs.get("solo-app").map(|s| s.as_str()) != Some(app_id) {
                    continue;
                }
                if attrs.get("url").map(|s| s.as_str()) != Some(url) {
                    continue;
                }

                let username = match attrs.get("username") {
                    Some(u) => u.clone(),
                    None => continue,
                };

                if let Err(err) = item.unlock().await {
                    return Err(map_oo7_error(err, url));
                }

                let secret_bytes = item
                    .secret()
                    .await
                    .map_err(|err| map_oo7_error(err, url))?;

                let secret = String::from_utf8(secret_bytes.to_vec())
                    .map_err(|_| Error::from_reason("Stored credential secret is not valid UTF-8"))?;

                return Ok(Some(CredentialResult { username, secret }));
            }

            Ok(None)
        })
    }

    pub fn delete(url: &str, app_id: &str) -> Result<bool> {
        futures::executor::block_on(async {
            let keyring = oo7::Keyring::new()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            keyring
                .unlock()
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            let attributes = [
                ("solo-app", app_id),
                ("url", url),
            ];

            let items = keyring
                .search_items(&attributes)
                .await
                .map_err(|err| map_oo7_error(err, url))?;

            let mut deleted_any = false;
            for item in items {
                let attrs = match item.attributes().await {
                    Ok(a) => a,
                    Err(_) => continue,
                };

                if attrs.get("solo-app").map(|s| s.as_str()) != Some(app_id) {
                    continue;
                }
                if attrs.get("url").map(|s| s.as_str()) != Some(url) {
                    continue;
                }

                item.delete().await.map_err(|err| map_oo7_error(err, url))?;
                deleted_any = true;
            }

            Ok(deleted_any)
        })
    }
}

// ---------------------------------------------------------------------------
// macOS (Keychain via gpui_apple)
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use gpui_apple::keychain::{self, KeychainError};

    const ERR_SEC_USER_CANCELED: i32 = -128;
    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;

    /// The Keychain server attribute. The app identifier partitions items the
    /// way Windows' target name does, so two Solo apps, or Zed, storing the
    /// same URL never overwrite each other.
    pub(super) fn server(app_id: &str, url: &str) -> String {
        format!("solo:{app_id}:{url}")
    }

    pub(super) fn map_error(err: KeychainError, url: &str) -> Error {
        match err {
            KeychainError::Security {
                status: ERR_SEC_USER_CANCELED,
                ..
            } => Error::from_reason("Keychain access was denied"),
            err => Error::from_reason(format!("Keychain operation failed for {url}: {err}")),
        }
    }

    pub fn write(url: &str, username: &str, secret: &[u8], app_id: &str) -> Result<()> {
        keychain::write_credentials(&server(app_id, url), username, secret)
            .map_err(|err| map_error(err, url))
    }

    /// A missing item and a denied access prompt both read as `None`:
    /// `gpui_apple` reports `errSecUserCanceled` on read as absence.
    pub fn read(url: &str, app_id: &str) -> Result<Option<CredentialResult>> {
        let Some((username, bytes)) =
            keychain::read_credentials(&server(app_id, url)).map_err(|err| map_error(err, url))?
        else {
            return Ok(None);
        };
        let secret = String::from_utf8(bytes)
            .map_err(|_| Error::from_reason("Stored credential secret is not valid UTF-8"))?;
        Ok(Some(CredentialResult { username, secret }))
    }

    pub fn delete(url: &str, app_id: &str) -> Result<bool> {
        match keychain::delete_credentials(&server(app_id, url)) {
            Ok(()) => Ok(true),
            Err(KeychainError::Security {
                status: ERR_SEC_ITEM_NOT_FOUND,
                ..
            }) => Ok(false),
            Err(err) => Err(map_error(err, url)),
        }
    }
}

// ---------------------------------------------------------------------------
// Windows (Windows Credential Manager)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod windows {
    use super::*;

    const CRED_TYPE_GENERIC: u32 = 1;
    const CRED_PERSIST_LOCAL_MACHINE: u32 = 2;
    const ERROR_NOT_FOUND: u32 = 1168;

    #[repr(C)]
    struct CREDENTIALW {
        flags: u32,
        r#type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: [u32; 2],
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut std::ffi::c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CREDENTIALW, flags: u32) -> i32;
        fn CredReadW(
            target_name: *const u16,
            r#type: u32,
            flags: u32,
            credential: *mut *mut CREDENTIALW,
        ) -> i32;
        fn CredDeleteW(target_name: *const u16, r#type: u32, flags: u32) -> i32;
        fn CredFree(buffer: *mut std::ffi::c_void);
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetLastError() -> u32;
    }

    const CRED_MAX_CREDENTIAL_BLOB_SIZE: usize = 5 * 512; // 2560 bytes

    pub(super) fn target_name(app_id: &str, url: &str) -> Vec<u16> {
        let name = format!("solo:{app_id}:{url}");
        name.encode_utf16().chain(Some(0)).collect()
    }

    fn to_wide_null(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub fn write(url: &str, username: &str, secret: &[u8], app_id: &str) -> Result<()> {
        if secret.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE {
            return Err(Error::from_reason(format!(
                "Secret length ({} bytes) exceeds Windows Credential Manager limit of {} bytes",
                secret.len(),
                CRED_MAX_CREDENTIAL_BLOB_SIZE
            )));
        }

        let mut target = target_name(app_id, url);
        let mut user = to_wide_null(username);
        let cred = CREDENTIALW {
            flags: 0,
            r#type: CRED_TYPE_GENERIC,
            target_name: target.as_mut_ptr(),
            comment: std::ptr::null_mut(),
            last_written: [0, 0],
            credential_blob_size: secret.len() as u32,
            credential_blob: secret.as_ptr() as *mut u8,
            persist: CRED_PERSIST_LOCAL_MACHINE,
            attribute_count: 0,
            attributes: std::ptr::null_mut(),
            target_alias: std::ptr::null_mut(),
            user_name: user.as_mut_ptr(),
        };

        let success = unsafe { CredWriteW(&cred, 0) };
        if success != 0 {
            Ok(())
        } else {
            let err = unsafe { GetLastError() };
            Err(Error::from_reason(format!(
                "Failed to write credentials for {url}: Windows error {err}"
            )))
        }
    }

    pub fn read(url: &str, app_id: &str) -> Result<Option<CredentialResult>> {
        let target = target_name(app_id, url);
        let mut cred_ptr: *mut CREDENTIALW = std::ptr::null_mut();
        let success = unsafe {
            CredReadW(
                target.as_ptr(),
                CRED_TYPE_GENERIC,
                0,
                &mut cred_ptr,
            )
        };

        if success == 0 {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(Error::from_reason(format!(
                "Failed to read credentials for {url}: Windows error {err}"
            )));
        }

        if cred_ptr.is_null() {
            return Ok(None);
        }

        let cred = unsafe { &*cred_ptr };
        let username = if cred.user_name.is_null() {
            String::new()
        } else {
            let len = unsafe {
                let mut p = cred.user_name;
                let mut count = 0;
                while *p != 0 {
                    p = p.add(1);
                    count += 1;
                }
                count
            };
            let slice = unsafe { std::slice::from_raw_parts(cred.user_name, len) };
            String::from_utf16_lossy(slice)
        };

        let secret = if cred.credential_blob.is_null() || cred.credential_blob_size == 0 {
            String::new()
        } else {
            let slice = unsafe {
                std::slice::from_raw_parts(cred.credential_blob, cred.credential_blob_size as usize)
            };
            std::str::from_utf8(slice)
                .map_err(|_| Error::from_reason("Stored credential secret is not valid UTF-8"))?
                .to_string()
        };

        unsafe { CredFree(cred_ptr as *mut std::ffi::c_void) };

        Ok(Some(CredentialResult { username, secret }))
    }

    pub fn delete(url: &str, app_id: &str) -> Result<bool> {
        let target = target_name(app_id, url);
        let success = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if success != 0 {
            Ok(true)
        } else {
            let err = unsafe { GetLastError() };
            if err == ERROR_NOT_FOUND {
                Ok(false)
            } else {
                Err(Error::from_reason(format!(
                    "Failed to delete credentials for {url}: Windows error {err}"
                )))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tasks for libuv worker pool
// ---------------------------------------------------------------------------

pub struct WriteCredentialsTask {
    url: String,
    username: String,
    secret: String,
    app_id: String,
}

#[napi]
impl Task for WriteCredentialsTask {
    type Output = ();
    type JsValue = ();

    fn compute(&mut self) -> Result<Self::Output> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            linux::write(&self.url, &self.username, self.secret.as_bytes(), &self.app_id)
        }
        #[cfg(target_os = "macos")]
        {
            macos::write(&self.url, &self.username, self.secret.as_bytes(), &self.app_id)
        }
        #[cfg(target_os = "windows")]
        {
            windows::write(&self.url, &self.username, self.secret.as_bytes(), &self.app_id)
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
        {
            Err(Error::from_reason(
                "Secure credential storage is supported only on macOS, Linux and Windows",
            ))
        }
    }

    fn resolve(&mut self, _env: napi::Env, _output: Self::Output) -> Result<Self::JsValue> {
        Ok(())
    }
}

pub struct ReadCredentialsTask {
    url: String,
    app_id: String,
}

#[napi]
impl Task for ReadCredentialsTask {
    type Output = Option<CredentialResult>;
    type JsValue = Option<CredentialResult>;

    fn compute(&mut self) -> Result<Self::Output> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            linux::read(&self.url, &self.app_id)
        }
        #[cfg(target_os = "macos")]
        {
            macos::read(&self.url, &self.app_id)
        }
        #[cfg(target_os = "windows")]
        {
            windows::read(&self.url, &self.app_id)
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
        {
            Err(Error::from_reason(
                "Secure credential storage is supported only on macOS, Linux and Windows",
            ))
        }
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct DeleteCredentialsTask {
    url: String,
    app_id: String,
}

#[napi]
impl Task for DeleteCredentialsTask {
    type Output = bool;
    type JsValue = bool;

    fn compute(&mut self) -> Result<Self::Output> {
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            linux::delete(&self.url, &self.app_id)
        }
        #[cfg(target_os = "macos")]
        {
            macos::delete(&self.url, &self.app_id)
        }
        #[cfg(target_os = "windows")]
        {
            windows::delete(&self.url, &self.app_id)
        }
        #[cfg(not(any(target_os = "linux", target_os = "freebsd", target_os = "macos", target_os = "windows")))]
        {
            Err(Error::from_reason(
                "Secure credential storage is supported only on macOS, Linux and Windows",
            ))
        }
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

// ---------------------------------------------------------------------------
// Napi exports
// ---------------------------------------------------------------------------

/// Store a secret in the operating system's secure credential store.
/// Rejects with an Error if the OS rejects the write, or off macOS, Linux and Windows.
#[napi]
pub fn write_credentials(
    url: String,
    username: String,
    secret: String,
    app_id: Option<String>,
) -> Result<AsyncTask<WriteCredentialsTask>> {
    validate_url(&url)?;
    validate_username(&username)?;
    let app_id = resolve_app_id(app_id)?;
    Ok(AsyncTask::new(WriteCredentialsTask {
        url,
        username,
        secret,
        app_id,
    }))
}

/// Retrieve a secret from the operating system's secure credential store.
/// Returns null when no credential matches `url`.
#[napi]
pub fn read_credentials(
    url: String,
    app_id: Option<String>,
) -> Result<AsyncTask<ReadCredentialsTask>> {
    validate_url(&url)?;
    let app_id = resolve_app_id(app_id)?;
    Ok(AsyncTask::new(ReadCredentialsTask { url, app_id }))
}

/// Remove a secret from the operating system's secure credential store.
/// Returns true if an item was removed, false if no matching credential existed.
#[napi]
pub fn delete_credentials(
    url: String,
    app_id: Option<String>,
) -> Result<AsyncTask<DeleteCredentialsTask>> {
    validate_url(&url)?;
    let app_id = resolve_app_id(app_id)?;
    Ok(AsyncTask::new(DeleteCredentialsTask { url, app_id }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_empty_url_and_username() {
        assert!(validate_url("").is_err());
        assert!(validate_url("   ").is_err());
        assert!(validate_url("https://example.com").is_ok());

        assert!(validate_username("").is_err());
        assert!(validate_username("   ").is_err());
        assert!(validate_username("alice").is_ok());
    }

    #[test]
    fn resolves_default_app_id() {
        assert!(resolve_app_id(None).is_err());
        assert!(resolve_app_id(Some("".into())).is_err());
        assert_eq!(resolve_app_id(Some("com.example.app".into())).unwrap(), "com.example.app");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_server_is_partitioned_by_app() {
        assert_eq!(
            macos::server("my.app", "https://api.github.com"),
            "solo:my.app:https://api.github.com"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_denied_prompt_rejects_clearly() {
        use gpui_apple::keychain::{KeychainError, KeychainOperation};
        let denied = KeychainError::Security { operation: KeychainOperation::Update, status: -128 };
        assert_eq!(macos::map_error(denied, "https://a.test").reason, "Keychain access was denied");
        let failed = KeychainError::Security { operation: KeychainOperation::Create, status: -25293 };
        assert_eq!(
            macos::map_error(failed, "https://a.test").reason,
            "Keychain operation failed for https://a.test: keychain Create failed with OSStatus -25293"
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_target_name_format() {
        let name = windows::target_name("my.app", "https://api.github.com");
        let decoded = String::from_utf16_lossy(&name[..name.len() - 1]);
        assert_eq!(decoded, "solo:my.app:https://api.github.com");
    }
}
