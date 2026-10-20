use futures::channel::oneshot;
use napi::bindgen_prelude::{AsyncTask, Error, Result, Task};
use napi_derive::napi;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Deserialize)]
pub(crate) struct OpenDialogOptions {
    #[serde(default)]
    pub(crate) multiple: bool,
    #[serde(default)]
    pub(crate) directories: bool,
    pub(crate) prompt: Option<String>,
}

impl OpenDialogOptions {
    pub(crate) fn to_path_prompt_options(&self) -> gpui::PathPromptOptions {
        gpui::PathPromptOptions {
            files: !self.directories,
            directories: self.directories,
            multiple: self.multiple,
            prompt: self.prompt.as_deref().map(gpui::SharedString::from),
        }
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct SaveDialogOptions {
    pub(crate) suggested_name: Option<String>,
    pub(crate) initial_directory: PathBuf,
}

pub(crate) fn parse_open_options(options_json: &str) -> Result<OpenDialogOptions> {
    let options: OpenDialogOptions = serde_json::from_str(options_json)
        .map_err(|error| Error::from_reason(format!("Invalid file dialog options: {error}")))?;
    if options
        .prompt
        .as_deref()
        .is_some_and(|prompt| prompt.trim().is_empty())
    {
        return Err(Error::from_reason(
            "File dialog prompt must be a non-empty string",
        ));
    }
    Ok(options)
}

pub(crate) fn parse_save_options(options_json: &str) -> Result<SaveDialogOptions> {
    let current_dir = std::env::current_dir().map_err(|error| {
        Error::from_reason(format!(
            "Unable to determine the current directory: {error}"
        ))
    })?;
    parse_save_options_with_default(options_json, &current_dir)
}

pub(crate) fn parse_save_options_with_default(
    options_json: &str,
    default_directory: &Path,
) -> Result<SaveDialogOptions> {
    #[derive(Debug, Deserialize)]
    struct RawSaveDialogOptions {
        #[serde(rename = "suggestedName")]
        suggested_name: Option<String>,
        #[serde(rename = "initialDirectory")]
        initial_directory: Option<String>,
    }

    let options: RawSaveDialogOptions = serde_json::from_str(options_json)
        .map_err(|error| Error::from_reason(format!("Invalid file dialog options: {error}")))?;

    let suggested_name = options
        .suggested_name
        .map(|name| {
            if name.trim().is_empty() {
                return Err(Error::from_reason(
                    "Suggested filename must be a non-empty string",
                ));
            }
            if name.contains(['/', '\\']) || name.contains('\0') {
                return Err(Error::from_reason(
                    "Suggested filename must not contain directory components",
                ));
            }
            Ok(name)
        })
        .transpose()?;

    let initial_directory = options
        .initial_directory
        .map(|directory| {
            if directory.trim().is_empty() {
                return Err(Error::from_reason(
                    "Initial directory must be a non-empty absolute path",
                ));
            }
            let directory = PathBuf::from(directory);
            if !directory.is_absolute() {
                return Err(Error::from_reason(
                    "Initial directory must be an absolute path",
                ));
            }
            Ok(directory)
        })
        .transpose()?
        .unwrap_or_else(|| default_directory.to_path_buf());

    Ok(SaveDialogOptions {
        suggested_name,
        initial_directory,
    })
}

pub(crate) fn path_to_string(path: PathBuf) -> Result<String> {
    path.into_os_string().into_string().map_err(|_| {
        Error::from_reason(
            "Selected path is not valid Unicode and cannot be returned to JavaScript",
        )
    })
}

pub(crate) fn paths_to_strings(paths: Vec<PathBuf>) -> Result<Vec<String>> {
    paths.into_iter().map(path_to_string).collect()
}

#[derive(Debug, Default)]
pub(crate) struct DialogState {
    active: bool,
}

#[derive(Debug)]
pub(crate) struct DialogLease {
    state: Arc<Mutex<DialogState>>,
}

impl DialogLease {
    pub(crate) fn acquire(state: Arc<Mutex<DialogState>>) -> Result<Self> {
        let mut state_guard = state
            .lock()
            .map_err(|_| Error::from_reason("File dialog state is unavailable"))?;
        if state_guard.active {
            return Err(Error::from_reason("A file dialog is already open"));
        }
        state_guard.active = true;
        drop(state_guard);
        Ok(Self { state })
    }
}

impl Drop for DialogLease {
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.active = false;
        }
    }
}

type OpenReceiver = oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>>;
type SaveReceiver = oneshot::Receiver<anyhow::Result<Option<PathBuf>>>;

pub struct OpenDialogTask {
    receiver: Option<OpenReceiver>,
    _lease: DialogLease,
}

impl OpenDialogTask {
    pub(crate) fn new(receiver: OpenReceiver, lease: DialogLease) -> AsyncTask<Self> {
        AsyncTask::new(Self {
            receiver: Some(receiver),
            _lease: lease,
        })
    }
}

#[napi]
impl Task for OpenDialogTask {
    type Output = Option<Vec<String>>;
    type JsValue = Option<Vec<String>>;

    fn compute(&mut self) -> Result<Self::Output> {
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::from_reason("File dialog task was already consumed"))?;
        let response = futures::executor::block_on(receiver)
            .map_err(|_| Error::from_reason("File dialog channel closed during shutdown"))?
            .map_err(|error| Error::from_reason(format!("File dialog failed: {error}")))?;
        response.map(paths_to_strings).transpose()
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

pub struct SaveDialogTask {
    receiver: Option<SaveReceiver>,
    _lease: DialogLease,
}

impl SaveDialogTask {
    pub(crate) fn new(receiver: SaveReceiver, lease: DialogLease) -> AsyncTask<Self> {
        AsyncTask::new(Self {
            receiver: Some(receiver),
            _lease: lease,
        })
    }
}

#[napi]
impl Task for SaveDialogTask {
    type Output = Option<String>;
    type JsValue = Option<String>;

    fn compute(&mut self) -> Result<Self::Output> {
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::from_reason("File dialog task was already consumed"))?;
        let response = futures::executor::block_on(receiver)
            .map_err(|_| Error::from_reason("File dialog channel closed during shutdown"))?
            .map_err(|error| Error::from_reason(format!("File dialog failed: {error}")))?;
        response.map(path_to_string).transpose()
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    #[test]
    fn parses_open_options_with_safe_defaults() {
        let options = parse_open_options("{}").unwrap();
        assert!(!options.multiple);
        assert_eq!(options.prompt, None);

        let options = parse_open_options(r#"{"multiple":true,"prompt":"Import OPML"}"#).unwrap();
        assert!(options.multiple);
        assert_eq!(options.prompt.as_deref(), Some("Import OPML"));

        let prompt = options.to_path_prompt_options();
        assert!(prompt.files);
        assert!(!prompt.directories);
        assert!(prompt.multiple);
        assert_eq!(
            prompt.prompt.as_deref().map(str::as_ref),
            Some("Import OPML")
        );

        let prompt = parse_open_options(r#"{"directories":true}"#)
            .unwrap()
            .to_path_prompt_options();
        assert!(!prompt.files);
        assert!(prompt.directories);
    }

    #[test]
    fn parses_save_options_and_uses_current_directory_default() {
        let options = parse_save_options_with_default(
            r#"{"suggestedName":"Notes 世界.md","initialDirectory":"/tmp/Newsprint Exports"}"#,
            Path::new("/tmp/default"),
        )
        .unwrap();
        assert_eq!(options.suggested_name.as_deref(), Some("Notes 世界.md"));
        assert_eq!(
            options.initial_directory,
            PathBuf::from("/tmp/Newsprint Exports")
        );

        let options = parse_save_options_with_default("{}", Path::new("/tmp/default")).unwrap();
        assert_eq!(options.initial_directory, PathBuf::from("/tmp/default"));
    }

    #[test]
    fn rejects_invalid_dialog_options() {
        for json in [
            r#"{"multiple":"yes"}"#,
            r#"{"directories":1}"#,
            r#"{"prompt":" "}"#,
            r#"{"prompt":1}"#,
        ] {
            assert!(parse_open_options(json).is_err(), "accepted {json}");
        }
        for json in [
            r#"{"suggestedName":" "}"#,
            r#"{"suggestedName":"folder/notes.md"}"#,
            r#"{"initialDirectory":"relative"}"#,
            r#"{"initialDirectory":" "}"#,
        ] {
            assert!(
                parse_save_options_with_default(json, Path::new("/tmp/default")).is_err(),
                "accepted {json}"
            );
        }
    }

    #[test]
    fn preserves_unicode_and_spaces_when_converting_paths() {
        let paths =
            paths_to_strings(vec![PathBuf::from("/tmp/Newsprint Archive 世界.json")]).unwrap();
        assert_eq!(paths, vec!["/tmp/Newsprint Archive 世界.json"]);
        assert_eq!(
            path_to_string(PathBuf::from("/tmp/Notes 世界.md")).unwrap(),
            "/tmp/Notes 世界.md"
        );
    }

    #[test]
    fn dialog_lease_allows_only_one_in_flight_request() {
        let state = Arc::new(Mutex::new(DialogState::default()));
        let lease = DialogLease::acquire(state.clone()).unwrap();
        let error = DialogLease::acquire(state.clone()).unwrap_err();
        assert!(error.to_string().contains("already open"));
        drop(lease);
        assert!(DialogLease::acquire(state).is_ok());
    }

    #[test]
    fn dialog_tasks_distinguish_cancellation_from_channel_failure() {
        let state = Arc::new(Mutex::new(DialogState::default()));
        let (sender, receiver) = oneshot::channel();
        sender.send(Ok(None)).unwrap();
        let lease = DialogLease::acquire(state.clone()).unwrap();
        let mut task = OpenDialogTask {
            receiver: Some(receiver),
            _lease: lease,
        };
        assert_eq!(Task::compute(&mut task).unwrap(), None);
        drop(task);

        let (sender, receiver) = oneshot::channel();
        drop(sender);
        let lease = DialogLease::acquire(state).unwrap();
        let mut task = SaveDialogTask {
            receiver: Some(receiver),
            _lease: lease,
        };
        let error = Task::compute(&mut task).unwrap_err();
        assert!(error.to_string().contains("channel closed"));
    }
}
