//! Solo's own in-window prompt renderer.
//!
//! `PlatformWindow::prompt` returns `None` on Linux, so `Window::prompt` falls
//! back to gpui's `FallbackPromptRenderer`. That renderer draws clickable
//! buttons but handles no keys: an alert can be clicked, never answered with
//! Enter or Escape.
//!
//! Solo installs this renderer instead (`init_threaded` and the test renderer).
//! It keeps gpui's overlay contract — a full-window view that emits
//! `PromptResponse(index)` — and adds:
//!
//! - Enter answers the first button that is not the cancel button.
//! - Escape answers the cancel button; with no cancel button, the only button.
//!
//! Text goes through [`crate::text::chrome_text`], so the message, detail and
//! button labels land in the automation paint log (`getPaintedText`) and in
//! the screenshot the same frame paints them.

use gpui::prelude::*;
use gpui::{
    div, hsla, px, App, Context, EventEmitter, FocusHandle, Focusable, KeyDownEvent, PromptButton,
    PromptHandle, PromptLevel, PromptResponse, Render, RenderablePromptHandle, Window,
};

use crate::text::chrome_text;
use crate::theme::{neutral, Theme};

/// Install as the app's prompt builder; see [`crate::renderer::init_key_bindings`].
pub(crate) fn renderer(
    _level: PromptLevel,
    message: &str,
    detail: Option<&str>,
    buttons: &[PromptButton],
    handle: PromptHandle,
    window: &mut Window,
    cx: &mut App,
) -> RenderablePromptHandle {
    let view = cx.new(|cx| SoloPrompt {
        message: message.to_owned(),
        detail: detail.map(str::to_owned),
        buttons: buttons.to_vec(),
        focus: cx.focus_handle(),
    });
    handle.with_view(view, window, cx)
}

struct SoloPrompt {
    message: String,
    detail: Option<String>,
    buttons: Vec<PromptButton>,
    focus: FocusHandle,
}

impl SoloPrompt {
    fn answer(&self, index: usize, cx: &mut Context<Self>) {
        cx.emit(PromptResponse(index));
        cx.stop_propagation();
    }
}

/// The button Enter activates: the first one that is not the cancel button.
fn default_button(buttons: &[PromptButton]) -> usize {
    buttons
        .iter()
        .position(|button| !button.is_cancel())
        .unwrap_or(0)
}

/// The button Escape activates. `None` means Escape does nothing, which keeps
/// a multi-button alert from silently choosing a destructive action.
fn cancel_button(buttons: &[PromptButton]) -> Option<usize> {
    buttons
        .iter()
        .position(PromptButton::is_cancel)
        .or_else(|| (buttons.len() == 1).then_some(0))
}

impl EventEmitter<PromptResponse> for SoloPrompt {}

impl Focusable for SoloPrompt {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for SoloPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::dark();
        let default = default_button(&self.buttons);
        let detail = self.detail.clone();
        let buttons = self.buttons.clone();

        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .bg(hsla(0.0, 0.0, 0.0, 0.45))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                let index = match event.keystroke.key.as_str() {
                    "enter" => default_button(&this.buttons),
                    "escape" => match cancel_button(&this.buttons) {
                        Some(index) => index,
                        None => return,
                    },
                    _ => return,
                };
                this.answer(index, cx);
            }))
            .child(
                div()
                    .w(px(384.0))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_4()
                    .rounded_lg()
                    .bg(theme.bg)
                    .border_1()
                    .border_color(theme.border)
                    .font_family(theme.font_sans.clone())
                    .text_color(theme.text)
                    .text_size(px(14.0))
                    .child(div().w_full().child(chrome_text(self.message.clone().into(), None)))
                    .children(detail.map(|detail| {
                        div()
                            .w_full()
                            .text_size(px(12.0))
                            .text_color(theme.text_muted)
                            .child(chrome_text(detail.into(), None))
                    }))
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_row()
                            .justify_end()
                            .gap_2()
                            .children(buttons.iter().enumerate().map(|(index, button)| {
                                let is_default = index == default;
                                button_style(
                                    div()
                                        .id(index)
                                        .px(px(12.0))
                                        .py(px(6.0))
                                        .rounded_md()
                                        .cursor_pointer()
                                        .text_size(px(13.0))
                                        .when(is_default, |this| {
                                            this.bg(theme.accent)
                                                .text_color(gpui::white())
                                                .hover(|style| style.opacity(0.9))
                                        })
                                        .when(!is_default, |this| {
                                            this.bg(neutral(0.20))
                                                .text_color(theme.text)
                                                .hover(|style| style.bg(neutral(0.26)))
                                        })
                                        .child(chrome_text(button.label().clone(), None)),
                                    index,
                                    cx,
                                )
                            })),
                    ),
            )
    }
}

/// Attach the click handler. A free function keeps the `map` closure small
/// enough that the borrow of `cx` stays inside it.
fn button_style(
    button: gpui::Stateful<gpui::Div>,
    index: usize,
    cx: &mut Context<SoloPrompt>,
) -> gpui::Stateful<gpui::Div> {
    button.on_click(cx.listener(move |this, _, _, cx| {
        this.answer(index, cx);
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buttons(labels: &[&str]) -> Vec<PromptButton> {
        labels
            .iter()
            .map(|label| {
                if label.eq_ignore_ascii_case("cancel") {
                    PromptButton::cancel(*label)
                } else {
                    PromptButton::new(*label)
                }
            })
            .collect()
    }

    #[test]
    fn enter_answers_the_first_non_cancel_button() {
        assert_eq!(default_button(&buttons(&["OK"])), 0);
        assert_eq!(default_button(&buttons(&["Delete", "Cancel"])), 0);
        assert_eq!(default_button(&buttons(&["Cancel", "Save"])), 1);
        assert_eq!(default_button(&buttons(&["Cancel", "CANCEL"])), 0);
    }

    #[test]
    fn escape_answers_the_cancel_button() {
        assert_eq!(cancel_button(&buttons(&["Delete", "Cancel"])), Some(1));
        assert_eq!(cancel_button(&buttons(&["Cancel", "Save"])), Some(0));
    }

    #[test]
    fn escape_answers_a_single_button_alert() {
        assert_eq!(cancel_button(&buttons(&["OK"])), Some(0));
    }

    #[test]
    fn escape_ignores_an_alert_without_a_cancel_button() {
        assert_eq!(cancel_button(&buttons(&["Save", "Don't Save"])), None);
    }
}
