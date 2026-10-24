use futures::channel::oneshot;
use napi::bindgen_prelude::{AsyncTask, Error, Result, Task};
use napi_derive::napi;
use serde::Deserialize;

use crate::file_dialogs::DialogLease;

#[derive(Debug, PartialEq)]
pub(crate) struct AlertOptions {
    pub(crate) level: gpui::PromptLevel,
    pub(crate) message: String,
    pub(crate) detail: Option<String>,
    pub(crate) buttons: Vec<String>,
}

impl AlertOptions {
    /// Labels become GPUI buttons unchanged. A "Cancel" label (any case) is
    /// the cancel button, which macOS binds to Escape.
    pub(crate) fn prompt_buttons(&self) -> Vec<gpui::PromptButton> {
        self.buttons
            .iter()
            .map(|label| {
                if label.eq_ignore_ascii_case("cancel") {
                    gpui::PromptButton::cancel(label.clone())
                } else {
                    gpui::PromptButton::new(label.clone())
                }
            })
            .collect()
    }

    pub(crate) fn show(&self, window: &mut gpui::Window, cx: &mut gpui::App) -> AlertReceiver {
        window.prompt(
            self.level,
            &self.message,
            self.detail.as_deref(),
            &self.prompt_buttons(),
            cx,
        )
    }
}

pub(crate) fn parse_alert_options(options_json: &str) -> Result<AlertOptions> {
    #[derive(Debug, Deserialize)]
    struct RawAlertOptions {
        message: String,
        detail: Option<String>,
        level: Option<String>,
        buttons: Option<Vec<String>>,
    }

    let options: RawAlertOptions = serde_json::from_str(options_json)
        .map_err(|error| Error::from_reason(format!("Invalid alert options: {error}")))?;
    if options.message.trim().is_empty() {
        return Err(Error::from_reason("Alert message must be a non-empty string"));
    }
    if options
        .detail
        .as_deref()
        .is_some_and(|detail| detail.trim().is_empty())
    {
        return Err(Error::from_reason("Alert detail must be a non-empty string"));
    }
    let level = match options.level.as_deref() {
        None | Some("info") => gpui::PromptLevel::Info,
        Some("warning") => gpui::PromptLevel::Warning,
        Some("critical") => gpui::PromptLevel::Critical,
        Some(level) => {
            return Err(Error::from_reason(format!(
                "Alert level must be info, warning or critical, not {level:?}"
            )));
        }
    };
    let buttons = options.buttons.unwrap_or_else(|| vec!["OK".to_string()]);
    if buttons.is_empty() {
        return Err(Error::from_reason("An alert needs at least one button"));
    }
    if buttons.iter().any(|label| label.trim().is_empty()) {
        return Err(Error::from_reason(
            "Alert button labels must be non-empty strings",
        ));
    }
    Ok(AlertOptions {
        level,
        message: options.message,
        detail: options.detail,
        buttons,
    })
}

pub(crate) const ALERT_ALREADY_OPEN: &str = "An alert is already open";

pub(crate) type AlertReceiver = oneshot::Receiver<usize>;

pub struct AlertTask {
    receiver: Option<AlertReceiver>,
    _lease: DialogLease,
}

impl AlertTask {
    pub(crate) fn new(receiver: AlertReceiver, lease: DialogLease) -> AsyncTask<Self> {
        AsyncTask::new(Self {
            receiver: Some(receiver),
            _lease: lease,
        })
    }
}

#[napi]
impl Task for AlertTask {
    type Output = u32;
    type JsValue = u32;

    fn compute(&mut self) -> Result<Self::Output> {
        let receiver = self
            .receiver
            .take()
            .ok_or_else(|| Error::from_reason("Alert task was already consumed"))?;
        let index = futures::executor::block_on(receiver).map_err(|_| {
            Error::from_reason("The alert closed without an answer (window closed or shutdown)")
        })?;
        u32::try_from(index).map_err(|_| Error::from_reason("Alert answer is out of range"))
    }

    fn resolve(&mut self, _env: napi::Env, output: Self::Output) -> Result<Self::JsValue> {
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_dialogs::DialogState;
    use std::sync::{Arc, Mutex};

    #[test]
    fn parses_alert_options_with_defaults() {
        let options = parse_alert_options(r#"{"message":"Saved"}"#).unwrap();
        assert_eq!(
            options,
            AlertOptions {
                level: gpui::PromptLevel::Info,
                message: "Saved".into(),
                detail: None,
                buttons: vec!["OK".into()],
            }
        );

        let options = parse_alert_options(
            r#"{"message":"Delete 3 files?","detail":"İstanbul 世界","level":"critical","buttons":["Delete","Cancel"]}"#,
        )
        .unwrap();
        assert_eq!(options.level, gpui::PromptLevel::Critical);
        assert_eq!(options.detail.as_deref(), Some("İstanbul 世界"));
        assert_eq!(
            options.prompt_buttons(),
            vec![
                gpui::PromptButton::new("Delete"),
                gpui::PromptButton::cancel("Cancel"),
            ]
        );
        assert_eq!(
            parse_alert_options(r#"{"message":"m","level":"warning"}"#)
                .unwrap()
                .level,
            gpui::PromptLevel::Warning
        );
    }

    #[test]
    fn keeps_button_labels_as_written() {
        let options = parse_alert_options(r#"{"message":"m","buttons":["ok","CANCEL"]}"#).unwrap();
        assert_eq!(
            options.prompt_buttons(),
            vec![
                gpui::PromptButton::new("ok"),
                gpui::PromptButton::cancel("CANCEL"),
            ]
        );
    }

    #[test]
    fn rejects_invalid_alert_options() {
        for json in [
            r#"{}"#,
            r#"{"message":" "}"#,
            r#"{"message":1}"#,
            r#"{"message":"m","detail":""}"#,
            r#"{"message":"m","level":"error"}"#,
            r#"{"message":"m","buttons":[]}"#,
            r#"{"message":"m","buttons":["OK"," "]}"#,
            r#"{"message":"m","buttons":["OK",1]}"#,
        ] {
            assert!(parse_alert_options(json).is_err(), "accepted {json}");
        }
    }

    #[test]
    fn alert_task_returns_the_answer_and_reports_a_dropped_alert() {
        let state = Arc::new(Mutex::new(DialogState::default()));
        let (sender, receiver) = oneshot::channel();
        sender.send(1).unwrap();
        let lease = DialogLease::acquire_as(state.clone(), ALERT_ALREADY_OPEN).unwrap();
        let error = DialogLease::acquire_as(state.clone(), ALERT_ALREADY_OPEN).unwrap_err();
        assert!(error.to_string().contains(ALERT_ALREADY_OPEN));
        let mut task = AlertTask {
            receiver: Some(receiver),
            _lease: lease,
        };
        assert_eq!(Task::compute(&mut task).unwrap(), 1);
        drop(task);

        let (sender, receiver) = oneshot::channel::<usize>();
        drop(sender);
        let lease = DialogLease::acquire_as(state, ALERT_ALREADY_OPEN).unwrap();
        let mut task = AlertTask {
            receiver: Some(receiver),
            _lease: lease,
        };
        let error = Task::compute(&mut task).unwrap_err();
        assert!(error.to_string().contains("without an answer"));
    }
}
