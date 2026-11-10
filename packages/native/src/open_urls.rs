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
        #[cfg(all(not(target_os = "macos"), not(unix), ui_thread))]
        {
            Err(Error::from_reason(
                "URL scheme registration is supported only on macOS and Linux",
            ))
        }
        #[cfg(all(not(target_os = "macos"), not(ui_thread)))]
        {
            Err(Error::from_reason(
                "URL scheme registration is supported only on macOS and Linux",
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

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn claim_single_instance() -> Result<bool> {
    linux::claim_single_instance()
}

/// Not Unix: nothing delivers URLs, but initialization runs unchanged.
#[cfg(all(not(target_os = "macos"), not(unix), ui_thread))]
pub(crate) fn claim_single_instance() -> Result<bool> {
    Ok(true)
}

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn deliver_pending() {
    linux::deliver()
}

#[cfg(all(not(target_os = "macos"), not(unix), ui_thread))]
pub(crate) fn deliver_pending() {}

#[cfg(all(not(target_os = "macos"), unix, ui_thread))]
pub(crate) fn receive(urls: Vec<String>) {
    linux::receive(urls)
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
