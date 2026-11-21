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

/// Element-0 event whose `urls` lists what the system asked the app to open.
#[cfg(any(target_os = "macos", ui_thread))]
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
        #[cfg(all(not(target_os = "macos"), unix, ui_thread))]
        {
            linux::register(&self.scheme)
        }
        #[cfg(all(target_os = "windows", ui_thread))]
        {
            windows::register(&self.scheme)
        }
        #[cfg(all(not(target_os = "macos"), not(any(unix, target_os = "windows")), ui_thread))]
        {
            Err(Error::from_reason(
                "URL scheme registration is supported only on macOS, Linux and Windows",
            ))
        }
        #[cfg(all(not(target_os = "macos"), not(ui_thread)))]
        {
            Err(Error::from_reason(
                "URL scheme registration is supported only on macOS, Linux and Windows",
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

/// Linux has no platform URL delivery: launches arrive as arguments, a second
/// launch forwards to the first over a Unix socket, and registration writes a
/// freedesktop `.desktop` entry.
#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
mod linux {
    use super::validate_scheme;
    use napi::{Error, Result};
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    /// Enough for a few thousand URLs; a request larger than this is refused.
    const MAX_REQUEST: u64 = 1_048_576;

    /// URLs waiting for a renderer that can receive them, or for the JS
    /// listener that `onOpenUrls` installs.
    static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub(super) fn receive(urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        PENDING.lock().unwrap().extend(urls);
        deliver();
    }

    /// Hand held URLs to the app renderer if one is initialized. Safe from
    /// the socket thread: the callback is threadsafe and the queue is locked.
    pub(super) fn deliver() {
        let Some(callback) = crate::renderer::app_event_callback() else {
            return;
        };
        let urls = std::mem::take(&mut *PENDING.lock().unwrap());
        if urls.is_empty() {
            return;
        }
        crate::renderer::emit_event_full(&Some(callback), 0, super::OPEN_URLS_EVENT, |payload| {
            payload.urls = Some(urls);
        });
    }

    /// The URLs and files this process was started with. A URL argument is
    /// taken as is; an absolute path that exists becomes a `file://` URL, as
    /// macOS delivers files. Flags are ignored.
    fn launch_urls() -> Vec<String> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        // Development runs a JS entry through Bun or Node: that path is the
        // script, not something the user opened.
        let script = script_argument(&arguments);
        arguments
            .iter()
            .filter(|argument| Some(argument.as_str()) != script.as_deref())
            .filter_map(|argument| argument_url(argument))
            .collect()
    }

    fn argument_url(argument: &str) -> Option<String> {
        if argument.starts_with("://") || argument.starts_with('-') {
            return None;
        }
        if let Some((scheme, _)) = argument.split_once("://") {
            // Only a real scheme counts; a bare `a://b` still needs a letter.
            return (!scheme.is_empty() && !argument.contains(char::is_whitespace))
                .then(|| argument.to_string());
        }
        let path = Path::new(argument);
        if !path.is_absolute() || !path.is_file() {
            return None;
        }
        url::Url::from_file_path(path).ok().map(|url| url.to_string())
    }

    /// Whether `executable` is the Bun or Node runtime rather than a compiled
    /// Solo app. Only a runtime takes a script as its first argument; in a
    /// compiled binary that argument is something the user passed.
    fn is_runtime(executable: &Path) -> bool {
        executable
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| matches!(stem, "bun" | "node" | "nodejs"))
    }

    fn current_is_runtime() -> bool {
        std::env::current_exe().is_ok_and(|executable| is_runtime(&executable))
    }

    fn is_script(path: &Path) -> bool {
        path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("js" | "mjs" | "cjs")
            )
    }

    /// The script argument this process runs, when Bun or Node owns the
    /// executable. A compiled app never has one, so a user's `.js` file stays
    /// a launch argument. `arguments` excludes `argv[0]`.
    fn script_argument<'a>(arguments: &'a [String]) -> Option<&'a str> {
        if !current_is_runtime() {
            return None;
        }
        let first = arguments.first()?;
        is_script(Path::new(first)).then_some(first.as_str())
    }

    /// The JS entry this process runs, when Bun or Node owns the executable.
    /// The same detection as `exec_command` uses.
    fn js_entry() -> Option<PathBuf> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let path = Path::new(script_argument(&arguments)?);
        Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
    }

    /// A stable name for this build: the executable's stem plus a hash of its
    /// path, so two different apps never share a socket or a desktop entry.
    /// Under Bun or Node the executable is the shared runtime, so the app is
    /// the JS entry it runs; hashing only the runtime would give every Solo
    /// app the same identity and let the second launch exit into the first.
    fn app_key() -> String {
        use std::hash::{Hash, Hasher};
        let executable = std::env::current_exe().unwrap_or_default();
        let script = js_entry();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        executable.hash(&mut hasher);
        if let Some(script) = &script {
            script.hash(&mut hasher);
        }
        let stem = script
            .as_deref()
            .unwrap_or(&executable)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("solo");
        let name: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("{name}-{:x}", hasher.finish())
    }

    /// The directory the single-instance socket lives in.
    ///
    /// `XDG_RUNTIME_DIR` is per-user and private. When it is unset (`su`, some
    /// ssh sessions, containers) a predictable name directly in the shared
    /// temporary directory would let another local user bind the socket first
    /// and receive this user's launch URLs, so create a per-user directory
    /// that only its owner can enter and refuse to use it otherwise.
    fn runtime_dir() -> Result<PathBuf> {
        if let Some(directory) = std::env::var_os("XDG_RUNTIME_DIR")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
        {
            return Ok(directory);
        }
        let directory = std::env::temp_dir().join(format!("solo-{}", unsafe { libc::getuid() }));
        match std::fs::create_dir(&directory) {
            Ok(()) => {
                std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                    .map_err(|error| {
                        Error::from_reason(format!(
                            "Cannot restrict {}: {error}",
                            directory.display()
                        ))
                    })?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(Error::from_reason(format!(
                    "Cannot create {}: {error}",
                    directory.display()
                )))
            }
        }
        if !is_private_directory(&directory) {
            return Err(Error::from_reason(format!(
                "{} is not a private directory owned by this user",
                directory.display()
            )));
        }
        Ok(directory)
    }

    fn is_private_directory(directory: &Path) -> bool {
        use std::os::unix::fs::MetadataExt;
        let Ok(metadata) = std::fs::symlink_metadata(directory) else {
            return false;
        };
        metadata.file_type().is_dir()
            && metadata.uid() == unsafe { libc::getuid() }
            && metadata.mode() & 0o077 == 0
    }

    fn socket_path() -> Result<PathBuf> {
        Ok(runtime_dir()?.join(format!("solo-{}.sock", app_key())))
    }

    /// True when this process owns the app. A second launch forwards its
    /// arguments to the first over the socket and reports false, so the
    /// caller can exit without opening a window.
    pub(super) fn claim_single_instance() -> Result<bool> {
        let urls = launch_urls();
        let path = socket_path()?;
        if path.exists() {
            if let Ok(mut stream) = UnixStream::connect(&path) {
                let request = serde_json::to_vec(&urls)
                    .map_err(|error| Error::from_reason(error.to_string()))?;
                stream
                    .write_all(&request)
                    .map_err(|error| Error::from_reason(format!("Cannot reach the running app: {error}")))?;
                return Ok(false);
            }
            // A crashed instance leaves its socket behind; take it over.
            let _ = std::fs::remove_file(&path);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                Error::from_reason(format!("Cannot create {}: {error}", parent.display()))
            })?;
        }
        let listener = UnixListener::bind(&path).map_err(|error| {
            Error::from_reason(format!("Cannot listen on {}: {error}", path.display()))
        })?;
        // The runtime directory is already private, but be explicit: only this
        // user may hand the app URLs.
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        std::thread::Builder::new()
            .name("solo-open-urls".to_string())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    // A client that connects and never closes its write half
                    // must not block every later deep-link delivery.
                    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(2)));
                    let mut request = Vec::new();
                    if stream.take(MAX_REQUEST).read_to_end(&mut request).is_err() {
                        continue;
                    }
                    if let Ok(urls) = serde_json::from_slice::<Vec<String>>(&request) {
                        receive(urls);
                    }
                }
            })
            .map_err(|error| {
                Error::from_reason(format!("Cannot start the open-URL listener: {error}"))
            })?;
        receive(urls);
        Ok(true)
    }

    fn data_home() -> Result<PathBuf> {
        std::env::var_os("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .ok_or_else(|| {
                Error::from_reason("Cannot locate XDG_DATA_HOME or HOME for the desktop entry")
            })
    }

    /// Write (or update) this app's `.desktop` entry so it claims
    /// `x-scheme-handler/<scheme>`, then make it the default handler.
    pub(super) fn register(scheme: &str) -> Result<()> {
        let scheme = validate_scheme(scheme)?;
        let applications = data_home()?.join("applications");
        std::fs::create_dir_all(&applications).map_err(|error| {
            Error::from_reason(format!("Cannot create {}: {error}", applications.display()))
        })?;
        let name = format!("solo-{}.desktop", app_key());
        let file = applications.join(&name);
        let executable = std::env::current_exe()
            .map_err(|error| Error::from_reason(format!("Cannot locate the app executable: {error}")))?;
        let existing = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(error) => {
                return Err(Error::from_reason(format!(
                    "Cannot read {}: {error}",
                    file.display()
                )))
            }
        };
        let contents = desktop_entry(&existing, &executable, &scheme);
        std::fs::write(&file, contents).map_err(|error| {
            Error::from_reason(format!("Cannot write {}: {error}", file.display()))
        })?;
        let handler = format!("x-scheme-handler/{scheme}");
        let status = std::process::Command::new("xdg-mime")
            .args(["default", &name, &handler])
            .status()
            .map_err(|error| {
                Error::from_reason(format!("Cannot run xdg-mime (is xdg-utils installed?): {error}"))
            })?;
        if !status.success() {
            return Err(Error::from_reason(format!(
                "xdg-mime could not make {} the default handler for {handler}",
                file.display()
            )));
        }
        Ok(())
    }

    /// Quote an argument for a desktop entry's `Exec` key.
    fn quote(argument: &str) -> String {
        let escaped = argument.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{escaped}\"")
    }

    /// The command a desktop entry runs: the executable, plus the JS entry
    /// when development runs the app through Bun or Node. Without that entry
    /// the runtime would receive the URL as a script path.
    fn exec_command(executable: &Path) -> String {
        let mut command = quote(&executable.display().to_string());
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        if let Some(script) = script_argument(&arguments) {
            let path = Path::new(script);
            if let Ok(absolute) = path.canonicalize() {
                command.push(' ');
                command.push_str(&quote(&absolute.display().to_string()));
            }
        }
        command
    }

    fn template(executable: &Path, scheme: &str) -> String {
        let stem = executable
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Solo app");
        format!(
            "[Desktop Entry]\nType=Application\nName={stem}\nExec={} %U\nTerminal=false\nNoDisplay=true\nMimeType=x-scheme-handler/{scheme};\n",
            exec_command(executable)
        )
    }

    /// The entry with `scheme` added to its `MimeType` list, keeping every
    /// other key and group. An absent or empty file gets a fresh entry.
    fn desktop_entry(existing: &str, executable: &Path, scheme: &str) -> String {
        if existing.trim().is_empty() {
            return template(executable, scheme);
        }
        let handler = format!("x-scheme-handler/{scheme}");
        // Replace an existing MimeType line inside [Desktop Entry].
        let mut lines: Vec<String> = existing.lines().map(str::to_string).collect();
        let Some(start) = lines.iter().position(|line| line.trim() == "[Desktop Entry]") else {
            return template(executable, scheme);
        };
        let end = lines
            .iter()
            .skip(start + 1)
            .position(|line| line.trim_start().starts_with('['))
            .map(|offset| start + 1 + offset)
            .unwrap_or(lines.len());
        let mime = (start + 1..end).find(|&index| lines[index].starts_with("MimeType="));
        let mut types: Vec<String> = mime
            .map(|index| {
                lines[index]["MimeType=".len()..]
                    .split(';')
                    .filter(|entry| !entry.trim().is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        if types.iter().any(|entry| entry.trim() == handler) {
            return existing.to_string();
        }
        types.push(handler);
        let line = format!("MimeType={};", types.join(";"));
        match mime {
            Some(index) => lines[index] = line,
            None => lines.insert(end, line),
        }
        let mut merged = lines.join("\n");
        if existing.ends_with('\n') {
            merged.push('\n');
        }
        merged
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn launch_arguments_become_urls_but_flags_do_not() {
            assert_eq!(argument_url("myapp://note/42").as_deref(), Some("myapp://note/42"));
            assert_eq!(argument_url("file:///tmp/a.txt").as_deref(), Some("file:///tmp/a.txt"));
            assert_eq!(argument_url("--inspect"), None);
            assert_eq!(argument_url("relative/path"), None);
            assert_eq!(argument_url("missing").as_deref(), None);
            assert_eq!(argument_url("://nope"), None);
        }

        #[test]
        fn only_bun_and_node_take_a_script_argument() {
            assert!(is_runtime(Path::new("/home/me/.bun/bin/bun")));
            assert!(is_runtime(Path::new("/usr/bin/node")));
            assert!(is_runtime(Path::new("/usr/bin/nodejs")));
            assert!(!is_runtime(Path::new("/opt/solo/SoloTasks")));
        }

        #[test]
        fn missing_entry_is_created_with_the_scheme_handler() {
            let entry = desktop_entry("", Path::new("/opt/My App/solo"), "myapp");
            assert!(entry.contains("Exec=\"/opt/My App/solo\" %U"), "{entry}");
            assert!(entry.contains("MimeType=x-scheme-handler/myapp;"), "{entry}");
            assert!(entry.contains("NoDisplay=true"), "{entry}");
        }

        #[test]
        fn existing_entry_keeps_other_keys_and_adds_the_scheme() {
            let existing = "[Desktop Entry]\nType=Application\nName=Notes\nExec=notes %U\nMimeType=text/plain;image/png;\n\n[Desktop Action new]\nName=New\n";
            let entry = desktop_entry(existing, Path::new("/opt/notes"), "notes");
            assert!(entry.contains("Name=Notes"), "{entry}");
            assert!(entry.contains("MimeType=text/plain;image/png;x-scheme-handler/notes;"), "{entry}");
            assert!(entry.contains("[Desktop Action new]"), "{entry}");
            // A second registration is a no-op.
            assert_eq!(desktop_entry(&entry, Path::new("/opt/notes"), "notes"), entry);
        }

        #[test]
        fn the_socket_fallback_directory_must_be_private() {
            let directory =
                std::env::temp_dir().join(format!("solo-open-urls-test-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&directory);
            std::fs::create_dir(&directory).unwrap();
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .unwrap();
            assert!(is_private_directory(&directory));
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755))
                .unwrap();
            assert!(!is_private_directory(&directory));
            let _ = std::fs::remove_dir_all(&directory);
        }

        #[test]
        fn missing_mime_line_is_added_to_the_main_group() {
            let existing = "[Desktop Entry]\nType=Application\nName=Notes\nExec=notes\n\n[Other]\nKey=value\n";
            let entry = desktop_entry(existing, Path::new("/opt/notes"), "notes");
            let main_group = entry.split("[Other]").next().unwrap();
            assert!(main_group.contains("MimeType=x-scheme-handler/notes;"), "{entry}");
            assert!(entry.contains("[Other]\nKey=value"), "{entry}");
        }
    }
}

/// Windows URL delivery: launches arrive as arguments, a second launch forwards
/// to the running instance over a named pipe, and registration writes
/// `HKEY_CURRENT_USER\Software\Classes\<scheme>`.
#[cfg(all(target_os = "windows", ui_thread))]
mod windows {
    use super::validate_scheme;
    use napi::{Error, Result};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    type HKEY = isize;
    type HANDLE = isize;
    type BOOL = i32;

    const HKEY_CURRENT_USER: HKEY = -2147483647;
    const KEY_WRITE: u32 = 0x20006;
    const REG_SZ: u32 = 1;
    const ERROR_SUCCESS: i32 = 0;
    const ERROR_ACCESS_DENIED: u32 = 5;
    const ERROR_PIPE_BUSY: u32 = 231;
    const ERROR_NO_DATA: u32 = 232;
    const ERROR_PIPE_CONNECTED: u32 = 535;

    const INVALID_HANDLE_VALUE: HANDLE = -1;

    const PIPE_ACCESS_INBOUND: u32 = 0x00000001;
    const FILE_FLAG_FIRST_PIPE_INSTANCE: u32 = 0x00080000;
    const PIPE_TYPE_BYTE: u32 = 0x00000000;
    const PIPE_READMODE_BYTE: u32 = 0x00000000;
    const PIPE_WAIT: u32 = 0x00000000;
    const PIPE_REJECT_REMOTE_CLIENTS: u32 = 0x00000008;
    const PIPE_UNLIMITED_INSTANCES: u32 = 255;

    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER_CLASS: u32 = 1;
    const SDDL_REVISION_1: u32 = 1;
    const SE_KERNEL_OBJECT: u32 = 6;
    const OWNER_SECURITY_INFORMATION: u32 = 1;

    #[repr(C)]
    struct SECURITY_ATTRIBUTES {
        nLength: u32,
        lpSecurityDescriptor: *mut std::ffi::c_void,
        bInheritHandle: BOOL,
    }

    #[link(name = "advapi32")]
    extern "system" {
        fn OpenProcessToken(
            ProcessHandle: HANDLE,
            DesiredAccess: u32,
            TokenHandle: *mut HANDLE,
        ) -> BOOL;

        fn GetTokenInformation(
            TokenHandle: HANDLE,
            TokenInformationClass: u32,
            TokenInformation: *mut std::ffi::c_void,
            TokenInformationLength: u32,
            ReturnLength: *mut u32,
        ) -> BOOL;

        fn ConvertSidToStringSidW(
            Sid: *mut std::ffi::c_void,
            StringSid: *mut *mut u16,
        ) -> BOOL;

        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(
            StringSecurityDescriptor: *const u16,
            StringSDRevision: u32,
            SecurityDescriptor: *mut *mut std::ffi::c_void,
            SecurityDescriptorSize: *mut u32,
        ) -> BOOL;

        fn GetSecurityInfo(
            handle: HANDLE,
            ObjectType: u32,
            SecurityInfo: u32,
            ppsidOwner: *mut *mut std::ffi::c_void,
            ppsidGroup: *mut *mut std::ffi::c_void,
            ppDacl: *mut *mut std::ffi::c_void,
            ppSacl: *mut *mut std::ffi::c_void,
            ppSecurityDescriptor: *mut *mut std::ffi::c_void,
        ) -> u32;

        fn RegCreateKeyExW(
            hKey: HKEY,
            lpSubKey: *const u16,
            Reserved: u32,
            lpClass: *const u16,
            dwOptions: u32,
            samDesired: u32,
            lpSecurityAttributes: *const std::ffi::c_void,
            phkResult: *mut HKEY,
            lpdwDisposition: *mut u32,
        ) -> i32;

        fn RegSetValueExW(
            hKey: HKEY,
            lpValueName: *const u16,
            Reserved: u32,
            dwType: u32,
            lpData: *const u8,
            cbData: u32,
        ) -> i32;

        fn RegCloseKey(hKey: HKEY) -> i32;
        fn RegDeleteTreeW(hKey: HKEY, lpSubKey: *const u16) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateNamedPipeW(
            lpName: *const u16,
            dwOpenMode: u32,
            dwPipeMode: u32,
            nMaxInstances: u32,
            nOutBufferSize: u32,
            nInBufferSize: u32,
            nDefaultTimeOut: u32,
            lpSecurityAttributes: *mut std::ffi::c_void,
        ) -> HANDLE;

        fn ConnectNamedPipe(hNamedPipe: HANDLE, lpOverlapped: *mut std::ffi::c_void) -> BOOL;
        fn CloseHandle(hObject: HANDLE) -> BOOL;
        fn GetLastError() -> u32;
        fn WaitNamedPipeW(lpNamedPipeName: *const u16, nTimeOut: u32) -> BOOL;
        fn GetCurrentProcess() -> HANDLE;
        fn LocalFree(hMem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }

    /// URLs waiting for a renderer that can receive them, or for the JS
    /// listener that `onOpenUrls` installs.
    static PENDING: Mutex<Vec<String>> = Mutex::new(Vec::new());

    pub(super) fn receive(urls: Vec<String>) {
        if urls.is_empty() {
            return;
        }
        PENDING.lock().unwrap().extend(urls);
        deliver();
    }

    /// Hand held URLs to the app renderer if one is initialized. Safe from
    /// the named pipe thread: the callback is threadsafe and the queue is locked.
    pub(super) fn deliver() {
        let Some(callback) = crate::renderer::app_event_callback() else {
            return;
        };
        let urls = std::mem::take(&mut *PENDING.lock().unwrap());
        if urls.is_empty() {
            return;
        }
        crate::renderer::emit_event_full(&Some(callback), 0, super::OPEN_URLS_EVENT, |payload| {
            payload.urls = Some(urls);
        });
    }

    /// The URLs and files this process was started with. A URL argument is
    /// taken as is; an absolute path that exists becomes a `file://` URL, as
    /// macOS delivers files. Flags are ignored.
    fn launch_urls() -> Vec<String> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let script = script_argument(&arguments);
        arguments
            .iter()
            .filter(|argument| Some(argument.as_str()) != script.as_deref())
            .filter_map(|argument| argument_url(argument))
            .collect()
    }

    fn argument_url(argument: &str) -> Option<String> {
        if argument.starts_with("://") || argument.starts_with('-') {
            return None;
        }
        if let Some((scheme, _)) = argument.split_once("://") {
            return (!scheme.is_empty() && !argument.contains(char::is_whitespace))
                .then(|| argument.to_string());
        }
        let path = Path::new(argument);
        if !path.is_absolute() || !path.is_file() {
            return None;
        }
        url::Url::from_file_path(path).ok().map(|url| url.to_string())
    }

    fn is_runtime(executable: &Path) -> bool {
        executable
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| matches!(stem, "bun" | "node" | "nodejs"))
    }

    fn current_is_runtime() -> bool {
        std::env::current_exe().is_ok_and(|executable| is_runtime(&executable))
    }

    fn is_script(path: &Path) -> bool {
        path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("js" | "mjs" | "cjs")
            )
    }

    fn script_argument<'a>(arguments: &'a [String]) -> Option<&'a str> {
        if !current_is_runtime() {
            return None;
        }
        let first = arguments.first()?;
        is_script(Path::new(first)).then_some(first.as_str())
    }

    fn js_entry() -> Option<PathBuf> {
        let arguments: Vec<String> = std::env::args().skip(1).collect();
        let path = Path::new(script_argument(&arguments)?);
        Some(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
    }

    fn app_key() -> String {
        use std::hash::{Hash, Hasher};
        let executable = std::env::current_exe().unwrap_or_default();
        let script = js_entry();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        executable.hash(&mut hasher);
        if let Some(script) = &script {
            script.hash(&mut hasher);
        }
        let stem = script
            .as_deref()
            .unwrap_or(&executable)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("solo");
        let name: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        format!("{name}-{:x}", hasher.finish())
    }

    /// The largest request a second launch may send.
    const MAX_REQUEST: u64 = 1_048_576;

    fn sid_to_string(sid: *mut std::ffi::c_void) -> Option<String> {
        let mut string_sid: *mut u16 = std::ptr::null_mut();
        let ok = unsafe { ConvertSidToStringSidW(sid, &mut string_sid) };
        if ok == 0 || string_sid.is_null() {
            return None;
        }
        let mut len = 0;
        unsafe {
            while *string_sid.add(len) != 0 {
                len += 1;
            }
        }
        let text = String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(string_sid, len) });
        unsafe { LocalFree(string_sid as *mut std::ffi::c_void) };
        Some(text)
    }

    fn current_user_sid() -> Option<String> {
        let mut token: HANDLE = 0;
        let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) };
        if ok == 0 || token == 0 {
            return None;
        }

        let mut needed = 0u32;
        unsafe {
            GetTokenInformation(token, TOKEN_USER_CLASS, std::ptr::null_mut(), 0, &mut needed);
        }
        if needed == 0 {
            unsafe { CloseHandle(token) };
            return None;
        }

        // TOKEN_USER starts with a pointer, so the buffer must be pointer-aligned.
        let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        let ok = unsafe {
            GetTokenInformation(
                token,
                TOKEN_USER_CLASS,
                buffer.as_mut_ptr() as *mut std::ffi::c_void,
                needed,
                &mut needed,
            )
        };
        unsafe { CloseHandle(token) };
        if ok == 0 {
            return None;
        }

        let sid = buffer[0] as *mut std::ffi::c_void;
        if sid.is_null() {
            return None;
        }
        sid_to_string(sid)
    }

    /// The SID that owns the object behind `handle`, read from its security
    /// descriptor.
    fn owner_sid(handle: HANDLE) -> Option<String> {
        let mut owner: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut descriptor: *mut std::ffi::c_void = std::ptr::null_mut();
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_KERNEL_OBJECT,
                OWNER_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut descriptor,
            )
        };
        let sid = (status == 0 && !owner.is_null()).then(|| sid_to_string(owner)).flatten();
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor) };
        }
        sid
    }

    /// A security descriptor that makes `sid` the pipe's owner and gives
    /// only that user access. Owning it explicitly matters: an elevated
    /// process would otherwise make Administrators the owner, and the
    /// client's owner check would then reject its own user's app.
    struct PipeSecurityDescriptor {
        sa: SECURITY_ATTRIBUTES,
    }

    impl PipeSecurityDescriptor {
        fn for_user(sid: &str) -> Result<Self> {
            let sddl = format!("O:{sid}D:(A;;GA;;;{sid})");
            let wide_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
            let mut descriptor = std::ptr::null_mut();
            let ok = unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide_sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || descriptor.is_null() {
                return Err(Error::from_reason(format!(
                    "Cannot build the named pipe's security descriptor: Windows error {}",
                    unsafe { GetLastError() }
                )));
            }
            Ok(PipeSecurityDescriptor {
                sa: SECURITY_ATTRIBUTES {
                    nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor,
                    bInheritHandle: 0,
                },
            })
        }
    }

    impl Drop for PipeSecurityDescriptor {
        fn drop(&mut self) {
            unsafe { LocalFree(self.sa.lpSecurityDescriptor) };
        }
    }

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// One pipe per user and app. The user's SID is in the name, so another
    /// user's app never collides with it; the owner check in
    /// `forward_to_running_instance` covers a name taken on purpose.
    fn pipe_name(sid: &str) -> String {
        format!(r"\\.\pipe\solo-open-urls-{sid}-{}", app_key())
    }

    pub(crate) fn command_line(executable: &Path, script: Option<&Path>) -> String {
        let exe = executable.to_string_lossy();
        if let Some(script) = script {
            format!("\"{exe}\" \"{}\" \"%1\"", script.to_string_lossy())
        } else {
            format!("\"{exe}\" \"%1\"")
        }
    }

    fn open_pipe(pipe: &str) -> std::io::Result<std::fs::File> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            match std::fs::OpenOptions::new().write(true).open(pipe) {
                Ok(file) => return Ok(file),
                Err(error) if error.raw_os_error() == Some(ERROR_PIPE_BUSY as i32) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(error);
                    }
                    unsafe { WaitNamedPipeW(wide(pipe).as_ptr(), 1000) };
                }
                result => return result,
            }
        }
    }

    /// Send `urls` to the instance listening on `pipe`. `Ok(false)` means
    /// nothing is listening. A pipe that another user owns is an error:
    /// sending to it would hand that user the URLs, and treating it as
    /// absent would let it block this app from listening.
    fn forward_to_running_instance(pipe: &str, sid: &str, urls: &[String]) -> Result<bool> {
        let Ok(mut file) = open_pipe(pipe) else {
            return Ok(false);
        };
        use std::os::windows::io::AsRawHandle;
        if owner_sid(file.as_raw_handle() as HANDLE).as_deref() != Some(sid) {
            return Err(Error::from_reason(format!(
                "Cannot claim single-instance launches: {pipe} is owned by another user"
            )));
        }
        let request =
            serde_json::to_vec(&urls).map_err(|error| Error::from_reason(error.to_string()))?;
        use std::io::Write;
        file.write_all(&request)
            .map_err(|error| Error::from_reason(format!("Cannot reach the running app: {error}")))?;
        Ok(true)
    }

    fn forward_to_running_instance_with_retry(pipe: &str, sid: &str, urls: &[String]) -> Result<bool> {
        for attempt in 0..10 {
            if attempt > 0 {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            if forward_to_running_instance(pipe, sid, urls)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Create one server instance of `pipe`. The first instance fails with
    /// `ERROR_ACCESS_DENIED` when the pipe already exists.
    fn create_pipe_instance(pipe: &str, sid: &str, first: bool) -> std::result::Result<HANDLE, u32> {
        let mut security = PipeSecurityDescriptor::for_user(sid).map_err(|_| 0u32)?;
        let first_flag = if first { FILE_FLAG_FIRST_PIPE_INSTANCE } else { 0 };
        let handle = unsafe {
            CreateNamedPipeW(
                wide(pipe).as_ptr(),
                PIPE_ACCESS_INBOUND | first_flag,
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                PIPE_UNLIMITED_INSTANCES,
                0,
                4096,
                0,
                &mut security.sa as *mut _ as *mut std::ffi::c_void,
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(unsafe { GetLastError() });
        }
        Ok(handle)
    }

    fn read_request(handle: HANDLE) {
        use std::io::Read;
        use std::os::windows::io::FromRawHandle;
        // The File closes the instance when it drops. std reads a closed
        // client end (ERROR_BROKEN_PIPE) as end of file.
        let file = unsafe { std::fs::File::from_raw_handle(handle as _) };
        let mut request = Vec::new();
        if file.take(MAX_REQUEST).read_to_end(&mut request).is_err() {
            return;
        }
        if let Ok(urls) = serde_json::from_slice::<Vec<String>>(&request) {
            receive(urls);
        }
    }

    enum PipeListenerResult {
        Started,
        AlreadyExists,
        Failed(String),
    }

    fn start_named_pipe_listener(pipe: &str, sid: &str) -> PipeListenerResult {
        if let Err(error) = PipeSecurityDescriptor::for_user(sid) {
            return PipeListenerResult::Failed(error.reason.clone());
        }
        let first = match create_pipe_instance(pipe, sid, true) {
            Ok(handle) => handle,
            Err(ERROR_ACCESS_DENIED | ERROR_PIPE_BUSY) => return PipeListenerResult::AlreadyExists,
            Err(error) => {
                return PipeListenerResult::Failed(format!(
                    "Cannot create named pipe listener: Windows error {error}"
                ))
            }
        };

        let pipe = pipe.to_string();
        let sid = sid.to_string();
        let spawned = std::thread::Builder::new()
            .name("solo-open-urls".into())
            .spawn(move || {
                let mut server = first;
                loop {
                    let ok = unsafe { ConnectNamedPipe(server, std::ptr::null_mut()) } != 0;
                    let error = if ok { 0 } else { unsafe { GetLastError() } };
                    let connected = ok || error == ERROR_PIPE_CONNECTED;
                    // Create the next instance before giving this one away,
                    // so the pipe never has zero instances: that would
                    // delete the name and let anyone take it.
                    let next = loop {
                        match create_pipe_instance(&pipe, &sid, false) {
                            Ok(handle) => break handle,
                            Err(_) => std::thread::sleep(std::time::Duration::from_millis(50)),
                        }
                    };
                    if connected {
                        // Each client gets its own reader, so one that
                        // connects and never closes cannot hold up later
                        // deliveries.
                        let handle = server;
                        if std::thread::Builder::new()
                            .name("solo-open-urls-read".into())
                            .spawn(move || read_request(handle))
                            .is_err()
                        {
                            unsafe { CloseHandle(handle) };
                        }
                    } else {
                        unsafe { CloseHandle(server) };
                        // ERROR_NO_DATA is a client that left before we saw
                        // it; anything else should not spin.
                        if error != ERROR_NO_DATA {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                    }
                    server = next;
                }
            });

        match spawned {
            Ok(_) => PipeListenerResult::Started,
            Err(e) => {
                unsafe { CloseHandle(first) };
                PipeListenerResult::Failed(format!("Cannot spawn open-urls thread: {e}"))
            }
        }
    }

    pub(super) fn claim_single_instance() -> Result<bool> {
        let urls = launch_urls();
        let sid = current_user_sid()
            .ok_or_else(|| Error::from_reason("Cannot read the current user's SID"))?;
        let pipe = pipe_name(&sid);

        if forward_to_running_instance(&pipe, &sid, &urls)? {
            return Ok(false);
        }

        match start_named_pipe_listener(&pipe, &sid) {
            PipeListenerResult::Started => {
                receive(urls);
                Ok(true)
            }
            PipeListenerResult::AlreadyExists => {
                if forward_to_running_instance_with_retry(&pipe, &sid, &urls)? {
                    Ok(false)
                } else {
                    Err(Error::from_reason(
                        "Another instance owns the named pipe, but forwarding failed",
                    ))
                }
            }
            PipeListenerResult::Failed(reason) => Err(Error::from_reason(reason)),
        }
    }

    fn set_reg_string(hkey: HKEY, subkey: &str, value_name: Option<&str>, value: &str) -> Result<()> {
        let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let mut key: HKEY = 0;
        let mut disp: u32 = 0;
        let status = unsafe {
            RegCreateKeyExW(
                hkey,
                subkey_wide.as_ptr(),
                0,
                std::ptr::null(),
                0,
                KEY_WRITE,
                std::ptr::null(),
                &mut key,
                &mut disp,
            )
        };
        if status != ERROR_SUCCESS {
            return Err(Error::from_reason(format!(
                "Failed to open or create registry key '{subkey}': Windows error {status}"
            )));
        }

        let val_wide: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
        let name_wide: Option<Vec<u16>> = value_name.map(|name| {
            name.encode_utf16().chain(std::iter::once(0)).collect()
        });
        let name_ptr = name_wide.as_ref().map_or(std::ptr::null(), |v| v.as_ptr());

        let set_status = unsafe {
            RegSetValueExW(
                key,
                name_ptr,
                0,
                REG_SZ,
                val_wide.as_ptr() as *const u8,
                (val_wide.len() * 2) as u32,
            )
        };
        unsafe { RegCloseKey(key) };

        if set_status != ERROR_SUCCESS {
            return Err(Error::from_reason(format!(
                "Failed to set registry value in '{subkey}': Windows error {set_status}"
            )));
        }
        Ok(())
    }

    pub(super) fn register(scheme: &str) -> Result<()> {
        let scheme = validate_scheme(scheme)?;
        let executable = std::env::current_exe()
            .map_err(|error| Error::from_reason(format!("Cannot determine current executable: {error}")))?;
        let script = js_entry();
        let cmd = command_line(&executable, script.as_deref());

        let root_subkey = format!(r"Software\Classes\{scheme}");
        let cmd_subkey = format!(r"Software\Classes\{scheme}\shell\open\command");

        set_reg_string(HKEY_CURRENT_USER, &root_subkey, None, &format!("URL:{scheme}"))?;
        set_reg_string(HKEY_CURRENT_USER, &root_subkey, Some("URL Protocol"), "")?;
        set_reg_string(HKEY_CURRENT_USER, &cmd_subkey, None, &cmd)?;

        Ok(())
    }

    #[cfg(test)]
    pub(super) fn delete_reg_scheme(scheme: &str) -> Result<()> {
        let subkey = format!(r"Software\Classes\{scheme}");
        let subkey_wide: Vec<u16> = subkey.encode_utf16().chain(std::iter::once(0)).collect();
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, subkey_wide.as_ptr()) };
        if status != ERROR_SUCCESS && status != 2 {
            return Err(Error::from_reason(format!(
                "Failed to delete registry key '{subkey}': Windows error {status}"
            )));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn launch_arguments_become_urls_but_flags_do_not() {
            assert_eq!(argument_url("myapp://note/42").as_deref(), Some("myapp://note/42"));
            assert_eq!(argument_url("file:///C:/tmp/a.txt").as_deref(), Some("file:///C:/tmp/a.txt"));
            assert_eq!(argument_url("--inspect"), None);
            assert_eq!(argument_url("relative/path"), None);
            assert_eq!(argument_url("missing").as_deref(), None);
            assert_eq!(argument_url("://nope"), None);
        }

        #[test]
        fn only_bun_and_node_take_a_script_argument() {
            assert!(is_runtime(Path::new(r"C:\Users\me\.bun\bin\bun.exe")));
            assert!(is_runtime(Path::new(r"C:\Program Files\nodejs\node.exe")));
            assert!(is_runtime(Path::new(r"C:\Program Files\nodejs\nodejs.exe")));
            assert!(!is_runtime(Path::new(r"C:\Program Files\SoloTasks\SoloTasks.exe")));
        }

        #[test]
        fn command_line_formats_standalone_and_script_targets() {
            let exe = Path::new(r"C:\Program Files\SoloTasks\SoloTasks.exe");
            assert_eq!(
                command_line(exe, None),
                r#""C:\Program Files\SoloTasks\SoloTasks.exe" "%1""#
            );

            let bun = Path::new(r"C:\Users\User\.bun\bin\bun.exe");
            let script = Path::new(r"C:\Users\User\app\src\index.js");
            assert_eq!(
                command_line(bun, Some(script)),
                r#""C:\Users\User\.bun\bin\bun.exe" "C:\Users\User\app\src\index.js" "%1""#
            );
        }

        #[test]
        fn register_and_delete_scheme_in_registry() {
            let scheme = "solo-windows-deep-link-test";
            register(scheme).unwrap();
            delete_reg_scheme(scheme).unwrap();
        }

        #[test]
        fn current_user_sid_is_valid() {
            let sid = current_user_sid();
            assert!(sid.is_some());
            let sid_str = sid.unwrap();
            assert!(sid_str.starts_with("S-1-"));
        }

        fn test_pipe(label: &str) -> String {
            format!(r"\\.\pipe\solo-test-{label}-{}", std::process::id())
        }

        fn wait_for_pending(url: &str) -> bool {
            for _ in 0..250 {
                if PENDING.lock().unwrap().iter().any(|pending| pending == url) {
                    return true;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            false
        }

        #[test]
        fn only_the_first_instance_claims_the_pipe() {
            let sid = current_user_sid().unwrap();
            let pipe = test_pipe("first-instance");
            let first = create_pipe_instance(&pipe, &sid, true).unwrap();
            assert_eq!(create_pipe_instance(&pipe, &sid, true), Err(ERROR_ACCESS_DENIED));
            let second = create_pipe_instance(&pipe, &sid, false).unwrap();
            unsafe {
                CloseHandle(second);
                CloseHandle(first);
            }
        }

        #[test]
        fn pipe_is_owned_by_the_current_user() {
            let sid = current_user_sid().unwrap();
            let pipe = test_pipe("owner");
            let server = create_pipe_instance(&pipe, &sid, true).unwrap();
            assert_eq!(owner_sid(server).as_deref(), Some(sid.as_str()));
            unsafe { CloseHandle(server) };
        }

        #[test]
        fn forwards_urls_and_refuses_a_pipe_owned_by_someone_else() {
            let sid = current_user_sid().unwrap();
            let pipe = test_pipe("forward");
            assert!(matches!(start_named_pipe_listener(&pipe, &sid), PipeListenerResult::Started));
            assert!(matches!(
                start_named_pipe_listener(&pipe, &sid),
                PipeListenerResult::AlreadyExists
            ));

            // A client that connects and never writes must not hold up the next one.
            let stalled = open_pipe(&pipe).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(50));
            let url = "solo-test://forwarded".to_string();
            assert!(forward_to_running_instance(&pipe, &sid, &[url.clone()]).unwrap());
            assert!(wait_for_pending(&url));
            drop(stalled);

            // LocalSystem stands in for another user: the pipe's owner does not match.
            let error = forward_to_running_instance(&pipe, "S-1-5-18", &[url]).unwrap_err();
            assert!(error.reason.contains("owned by another user"), "{}", error.reason);
        }

        #[test]
        fn nothing_listening_is_not_an_error() {
            let sid = current_user_sid().unwrap();
            let pipe = test_pipe("absent");
            assert!(!forward_to_running_instance(&pipe, &sid, &["solo-test://x".into()]).unwrap());
        }
    }
}

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn claim_single_instance() -> Result<bool> {
    linux::claim_single_instance()
}

#[cfg(all(target_os = "windows", ui_thread))]
pub(crate) fn claim_single_instance() -> Result<bool> {
    windows::claim_single_instance()
}

#[cfg(all(not(target_os = "macos"), not(any(unix, target_os = "windows")), ui_thread))]
pub(crate) fn claim_single_instance() -> Result<bool> {
    Ok(true)
}

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn deliver_pending() {
    linux::deliver()
}

#[cfg(all(target_os = "windows", ui_thread))]
pub(crate) fn deliver_pending() {
    windows::deliver()
}

#[cfg(all(not(target_os = "macos"), not(any(unix, target_os = "windows")), ui_thread))]
pub(crate) fn deliver_pending() {}

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn receive(urls: Vec<String>) {
    linux::receive(urls)
}

#[cfg(all(target_os = "windows", ui_thread))]
pub(crate) fn receive(urls: Vec<String>) {
    windows::receive(urls)
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
