//! Native menus track in a small AppKit helper so neither JS nor GPUI is
//! suspended inside NSMenu's modal loop. Each session owns one child process.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{c_void, CStr};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use napi::bindgen_prelude::{Error, Result, Task};
use napi_derive::napi;
use objc2_app_kit::NSView;
use objc2_core_foundation::CGPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

#[path = "../context-menu-host/src/protocol.rs"]
mod protocol;

struct Session {
    cancelled: AtomicBool,
    completed: AtomicBool,
    keys: Mutex<VecDeque<String>>,
}
struct ActiveMenu {
    id: u64,
    owner: u64,
    session: Weak<Session>,
}
thread_local! { static ACTIVE: RefCell<Option<ActiveMenu>> = const { RefCell::new(None) }; }
// Replaced/dismissed sessions can still be reaping a child. Track all of them,
// not just the menu currently accepting input.
thread_local! { static PENDING: RefCell<Vec<Weak<Session>>> = const { RefCell::new(Vec::new()) }; }

pub(crate) fn has_pending_cleanup() -> bool {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        pending.retain(|session| {
            session
                .upgrade()
                .is_some_and(|session| !session.completed.load(Ordering::Acquire))
        });
        !pending.is_empty()
    })
}

struct Completion<'a>(&'a Session);

impl Drop for Completion<'_> {
    fn drop(&mut self) {
        self.0.completed.store(true, Ordering::Release);
        // Retry a quit that was deferred until menu helpers finished.
        if crate::renderer::worker_main::enabled() {
            let _ = crate::renderer::on_main_async(|| {
                if let Err(error) = crate::macos_shutdown::poll() {
                    log::error!("Deferred quit failed: {error}");
                }
            });
        }
    }
}

pub(crate) fn cancel(request_id: u64) {
    // Menu state lives on the main thread, where `show` ran.
    if crate::renderer::worker_main::must_forward() {
        forward(move || cancel(request_id));
        return;
    }
    ACTIVE.with(|cell| {
        if let Some(active) = cell.borrow().as_ref() {
            if active.id == request_id {
                if let Some(session) = active.session.upgrade() {
                    session.cancelled.store(true, Ordering::Release);
                }
            }
        }
    });
}

pub(crate) fn cancel_all() {
    PENDING.with(|pending| {
        for session in pending.borrow().iter().filter_map(Weak::upgrade) {
            session.cancelled.store(true, Ordering::Release);
        }
    });
    ACTIVE.with(|cell| {
        if let Some(active) = cell.borrow_mut().take() {
            if let Some(session) = active.session.upgrade() {
                session.cancelled.store(true, Ordering::Release);
            }
        }
    });
}

pub(crate) fn cancel_destroyed(ids: &[u64]) {
    if crate::renderer::worker_main::must_forward() {
        let ids = ids.to_vec();
        forward(move || cancel_destroyed(&ids));
        return;
    }
    let should_cancel = ACTIVE.with(|cell| {
        cell.borrow()
            .as_ref()
            .is_some_and(|a| ids.contains(&a.owner))
    });
    if should_cancel {
        cancel_all();
    }
}

/// Automation follows the native menu's keyboard focus while it is tracking.
#[cfg(feature = "automation")]
pub(crate) fn dispatch_keys(keys: &[gpui::Keystroke]) -> bool {
    if crate::renderer::worker_main::must_forward() {
        let keys = keys.to_vec();
        return crate::renderer::on_main(move || Ok(dispatch_keys(&keys))).unwrap_or(false);
    }
    ACTIVE.with(|cell| {
        let active = cell.borrow();
        let Some(session) = active.as_ref().and_then(|a| a.session.upgrade()) else {
            return false;
        };
        if session.cancelled.load(Ordering::Acquire) {
            return false;
        }
        let mut queue = session.keys.lock().unwrap();
        for key in keys {
            queue.push_back(
                serde_json::json!({ "key": key.key, "shift": key.modifiers.shift,
                "control": key.modifiers.control, "alt": key.modifiers.alt,
                "platform": key.modifiers.platform })
                .to_string(),
            );
        }
        true
    })
}

fn forward(f: impl FnOnce() + Send + 'static) {
    if let Err(error) = crate::renderer::on_main_async(f) {
        log::error!("Context menu update failed: {error}");
    }
}

fn helper_path() -> Result<PathBuf> {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    };
    let name = format!("solo-context-menu.darwin-{arch}");
    let executable = std::env::current_exe().map_err(|e| Error::from_reason(e.to_string()))?;
    if let Some(directory) = executable.parent() {
        // Bun extracts embedded .node files into temporary storage. A packaged
        // app must use its signed Resources companion, never a temp lookup.
        if directory.file_name().is_some_and(|name| name == "MacOS") {
            if let Some(contents) = directory
                .parent()
                .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
            {
                return Ok(contents.join("Resources").join(&name));
            }
        }
        let sibling = directory.join(&name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    // Unbundled Node/Bun development: locate the original .node's companion.
    // There is no cwd/PATH-based search and no application-controlled command.
    let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
    let address = helper_path as *const () as *const c_void;
    // SAFETY: dladdr initializes info on success and dli_fname is a loaded
    // image's process-lifetime NUL-terminated path. This addon stays loaded.
    let found = unsafe { libc::dladdr(address, info.as_mut_ptr()) };
    if found == 0 {
        return Err(Error::from_reason("Cannot locate the native menu helper"));
    }
    let info = unsafe { info.assume_init() };
    if info.dli_fname.is_null() {
        return Err(Error::from_reason("Native image has no path"));
    }
    let path = unsafe { CStr::from_ptr(info.dli_fname) }
        .to_str()
        .map_err(|_| Error::from_reason("Native image path is not Unicode"))?;
    let parent = std::path::Path::new(path)
        .parent()
        .ok_or_else(|| Error::from_reason("Native image has no directory"))?;
    Ok(parent.join(name))
}

pub(crate) fn show(
    json: &str,
    tree: &crate::retained_tree::RetainedTree,
    window: &gpui::Window,
) -> Result<ContextMenuTask> {
    if crate::macos_shutdown::is_quitting() {
        return Err(Error::from_reason("Application is quitting"));
    }
    if json.len() > 1_048_576 {
        return Err(Error::from_reason("Context menu request is too large"));
    }
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| Error::from_reason(e.to_string()))?;
    let id = crate::renderer::to_element_id(value["requestId"].as_f64().unwrap_or(f64::NAN))?;
    let owner = crate::renderer::to_element_id(value["elementId"].as_f64().unwrap_or(f64::NAN))?;
    if !tree.elements.contains_key(&owner) {
        return Err(Error::from_reason(
            "Context menu owner element is unmounted",
        ));
    }
    let bounds = crate::automation::get_bounds(owner)
        .ok_or_else(|| Error::from_reason("Context menu owner element has not been painted"))?;
    let (x, y) = match (value.get("x"), value.get("y")) {
        (None, None) => (bounds.x, bounds.y + bounds.height),
        (Some(x), Some(y)) => (
            x.as_f64().unwrap_or(f64::NAN),
            y.as_f64().unwrap_or(f64::NAN),
        ),
        _ => return Err(Error::from_reason("Both pointer coordinates are required")),
    };
    if !x.is_finite() || !y.is_finite() {
        return Err(Error::from_reason("Pointer coordinates must be finite"));
    }
    let handle =
        HasWindowHandle::window_handle(window).map_err(|e| Error::from_reason(e.to_string()))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err(Error::from_reason("Context menu requires an AppKit window"));
    };
    // SAFETY: Window owns this NSView for this synchronous main-thread call.
    let view = unsafe { &*handle.ns_view.as_ptr().cast::<NSView>() };
    let host_window = view
        .window()
        .ok_or_else(|| Error::from_reason("Context menu window is closed"))?;
    let point = CGPoint::new(
        x,
        if view.isFlipped() {
            y
        } else {
            view.bounds().size.height - y
        },
    );
    let point = host_window.convertPointToScreen(view.convertPoint_toView(point, None));
    value["x"] = point.x.into();
    value["y"] = point.y.into();
    let request = protocol::parse_request(&value.to_string()).map_err(Error::from_reason)?;
    let payload = serde_json::to_string(&request).map_err(|e| Error::from_reason(e.to_string()))?;
    let helper = helper_path()?;
    if !helper.is_file() {
        return Err(Error::from_reason(format!("Native menu helper is missing: {}. Rebuild @solo/native and ship the helper beside its .node file.", helper.display())));
    }
    cancel_all();
    let session = Arc::new(Session {
        cancelled: AtomicBool::new(false),
        completed: AtomicBool::new(false),
        keys: Mutex::new(VecDeque::new()),
    });
    has_pending_cleanup(); // prune completed weak entries on ordinary use too
    PENDING.with(|pending| pending.borrow_mut().push(Arc::downgrade(&session)));
    ACTIVE.with(|cell| {
        *cell.borrow_mut() = Some(ActiveMenu {
            id,
            owner,
            session: Arc::downgrade(&session),
        })
    });
    Ok(ContextMenuTask {
        payload,
        helper,
        session,
    })
}

pub struct ContextMenuTask {
    payload: String,
    helper: PathBuf,
    session: Arc<Session>,
}

#[napi]
impl Task for ContextMenuTask {
    type Output = Option<String>;
    type JsValue = Option<String>;

    fn compute(&mut self) -> Result<Self::Output> {
        // Declared first, dropped last: completion includes child reap AND
        // TempDir destruction, including every early-return/error path.
        let _completion = Completion(&self.session);
        if self.session.cancelled.load(Ordering::Acquire) {
            return Ok(None);
        }
        // AppKit keyboard tracking differs when this executable inherits an
        // enclosing .app's bundle identity. Launch an exact private copy outside
        // the bundle. TempDir is mode 0700 and removes the copy after child reap;
        // TMPDIR controls placement (including external disks).
        let directory = tempfile::Builder::new()
            .prefix("solo-context-menu-")
            .tempdir()
            .map_err(|e| {
                Error::from_reason(format!("Cannot create private menu directory: {e}"))
            })?;
        let executable = directory.path().join("solo-context-menu");
        std::fs::copy(&self.helper, &executable)
            .map_err(|e| Error::from_reason(format!("Cannot prepare menu helper: {e}")))?;
        let mut child = Command::new(&executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::from_reason(format!("Cannot start native menu helper: {e}")))?;
        let write_result = child
            .stdin
            .as_mut()
            .ok_or_else(|| Error::from_reason("Menu input pipe is unavailable"))
            .and_then(|input| {
                writeln!(input, "{}", self.payload).map_err(|e| Error::from_reason(e.to_string()))
            });
        if let Err(error) = write_result {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        let mut cancellation_started = None;
        let status = loop {
            if self.session.cancelled.load(Ordering::Acquire) && cancellation_started.is_none() {
                child.stdin.take(); // EOF lets AppKit dismiss and release normally.
                cancellation_started = Some(Instant::now());
            }
            if cancellation_started
                .is_some_and(|start: Instant| start.elapsed() > Duration::from_secs(1))
            {
                let _ = child.kill(); // Bounded cleanup even if AppKit fails to cancel.
            }
            if let Some(input) = child.stdin.as_mut() {
                for key in self.session.keys.lock().unwrap().drain(..) {
                    // A closed input pipe is handled by try_wait below.
                    if writeln!(input, "{key}").is_err() {
                        break;
                    }
                }
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(Error::from_reason(error.to_string()));
                }
            }
        };
        if self.session.cancelled.load(Ordering::Acquire) {
            return Ok(None);
        }
        if !status.success() {
            return Err(Error::from_reason(format!(
                "Native menu helper exited with {status}"
            )));
        }
        let mut output = String::new();
        child
            .stdout
            .take()
            .ok_or_else(|| Error::from_reason("Menu output pipe is unavailable"))?
            .take(65_536)
            .read_to_string(&mut output)
            .map_err(|e| Error::from_reason(e.to_string()))?;
        let result = output
            .lines()
            .last()
            .ok_or_else(|| Error::from_reason("Native menu helper returned no selection"))?;
        serde_json::from_str(result)
            .map_err(|e| Error::from_reason(format!("Invalid native menu result: {e}")))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(if self.session.cancelled.load(Ordering::Acquire) {
            None
        } else {
            output
        })
    }
}

#[cfg(test)]
mod shutdown_tests {
    use super::*;

    #[test]
    fn shutdown_waits_for_replaced_and_current_workers() {
        let sessions: Vec<_> = (0..2)
            .map(|_| {
                Arc::new(Session {
                    cancelled: AtomicBool::new(false),
                    completed: AtomicBool::new(false),
                    keys: Mutex::new(VecDeque::new()),
                })
            })
            .collect();
        PENDING.with(|pending| *pending.borrow_mut() = sessions.iter().map(Arc::downgrade).collect());
        ACTIVE.with(|active| {
            *active.borrow_mut() = Some(ActiveMenu {
                id: 2,
                owner: 1,
                session: Arc::downgrade(&sessions[1]),
            });
        });
        cancel_all();
        assert!(sessions
            .iter()
            .all(|session| session.cancelled.load(Ordering::Acquire)));
        assert!(has_pending_cleanup());
        drop(Completion(&sessions[1]));
        assert!(has_pending_cleanup(), "replaced worker still owns cleanup");
        drop(Completion(&sessions[0]));
        assert!(!has_pending_cleanup());
    }
}
