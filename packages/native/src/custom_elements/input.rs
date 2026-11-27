//! Native single-line and multiline text editors with platform IME support.
//!
//! Caret blinking is ported from Comet's `crates/ui/src/composer.rs` (MIT).

use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, ElementInputHandler, Entity, EntityInputHandler, FocusHandle, GlobalElementId,
    KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, ScrollEvent, SharedString, Style, Task, TextRun, TextStyle, UTF16Selection,
    UnderlineStyle, Window, WrappedLine,
};
use unicode_segmentation::UnicodeSegmentation;

use super::text_editor::revision::{RebaseError, RevisionLog};
use super::text_editor::storage::{Edit, TextStorage};
use super::text_editor::undo::{EditKind, Transaction, UndoHistory};
use super::text_editor::Selection;
use super::{CustomCallContext, CustomElement, CustomElementFactory, CustomRenderContext};
use crate::element_tree::{TextCaretPayload, TextEditPayload, TextSelectionPayload};
use crate::renderer::{emit_event_full, EventCallback};
use crate::syntax::{HighlightRequest, HighlightSpan};
use crate::text::runs::runs_for_spans;
use crate::theme::{SyntaxPalette, Theme};

actions!(
    solo_text_editor,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        DocStart,
        DocEnd,
        SelectHome,
        SelectEnd,
        SelectDocStart,
        SelectDocEnd,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteToLineStart,
        DeleteToLineEnd,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Newline,
        Submit,
    ]
);

const INPUT_KEY_CONTEXT: &str = "SoloInput";
const TEXTAREA_KEY_CONTEXT: &str = "SoloTextarea";
const CARET_BLINK_MS: u64 = 500;
/// How long typing must pause before the text is parsed for highlighting.
const HIGHLIGHT_DEBOUNCE_MS: u64 = 30;

fn caret_visible(ms_since_activity: u64) -> bool {
    (ms_since_activity / CARET_BLINK_MS) % 2 == 0
}

fn utf16_offset_to_utf8(text: &str, offset: usize) -> usize {
    let mut utf8_offset = 0;
    let mut utf16_count = 0;
    for character in text.chars() {
        if utf16_count >= offset {
            break;
        }
        utf16_count += character.len_utf16();
        utf8_offset += character.len_utf8();
    }
    utf8_offset
}

/// Move the line spans of `lines` to where `edit` puts their lines, given
/// the `text` before it. Lines the edit touches lose their spans.
fn shift_line_spans(lines: &mut Vec<Vec<HighlightSpan>>, text: &str, edit: &Edit) {
    if lines.is_empty() {
        return;
    }
    let first = text[..edit.range.start].matches('\n').count();
    let last = first + text[edit.range.clone()].matches('\n').count();
    let end = (last + 1).min(lines.len());
    let added = edit.text.matches('\n').count();
    lines.splice(
        first.min(end)..end,
        std::iter::repeat_with(Vec::new).take(added + 1),
    );
}

/// The runs for an editor's `text`: each source line coloured by its spans
/// in `highlight`, then the IME composition in `marked` underlined on top.
fn editor_runs(
    text: &str,
    highlight: &[Vec<HighlightSpan>],
    palette: &SyntaxPalette,
    font: &gpui::Font,
    color: gpui::Hsla,
    marked: Option<&Range<usize>>,
) -> Vec<TextRun> {
    let plain = |len: usize| TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::new();
    if highlight.is_empty() {
        runs.push(plain(text.len()));
    } else {
        for (index, line) in text.split('\n').enumerate() {
            if index > 0 {
                runs.push(plain(1));
            }
            let spans: Vec<_> = highlight
                .get(index)
                .into_iter()
                .flatten()
                .map(|span| (span.range.clone(), palette.color(span.kind)))
                .collect();
            runs.extend(runs_for_spans(line, &spans, font, color));
        }
    }
    let Some(marked) = marked else {
        return runs;
    };
    let mut split = Vec::with_capacity(runs.len() + 2);
    let mut start = 0;
    for run in runs {
        let end = start + run.len;
        let from = marked.start.clamp(start, end);
        let to = marked.end.clamp(from, end);
        for (piece_start, piece_end, underlined) in [(start, from, false), (from, to, true), (to, end, false)] {
            if piece_end > piece_start {
                let mut piece = run.clone();
                piece.len = piece_end - piece_start;
                if underlined {
                    piece.underline = Some(UnderlineStyle {
                        color: Some(piece.color),
                        thickness: px(1.0),
                        wavy: false,
                    });
                }
                split.push(piece);
            }
        }
        start = end;
    }
    split
}

fn single_line_text(text: &str) -> String {
    text.replace("\r\n", " ").replace(['\r', '\n'], " ")
}

pub fn init(cx: &mut App) {
    let mut bindings = text_editor_bindings(INPUT_KEY_CONTEXT, false);
    bindings.extend(text_editor_bindings(TEXTAREA_KEY_CONTEXT, true));
    cx.bind_keys(bindings);
}

fn text_editor_bindings(context: &'static str, multiline: bool) -> Vec<KeyBinding> {
    let context = Some(context);
    let mut bindings = vec![
        KeyBinding::new("enter", Submit, context),
        KeyBinding::new("shift-enter", Newline, context),
        KeyBinding::new("backspace", Backspace, context),
        KeyBinding::new("delete", Delete, context),
        KeyBinding::new("left", Left, context),
        KeyBinding::new("right", Right, context),
        KeyBinding::new("shift-left", SelectLeft, context),
        KeyBinding::new("shift-right", SelectRight, context),
        KeyBinding::new("home", Home, context),
        KeyBinding::new("end", End, context),
        KeyBinding::new("shift-home", SelectHome, context),
        KeyBinding::new("shift-end", SelectEnd, context),
        KeyBinding::new("cmd-left", Home, context),
        KeyBinding::new("cmd-right", End, context),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, context),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, context),
        KeyBinding::new("cmd-up", DocStart, context),
        KeyBinding::new("cmd-down", DocEnd, context),
        KeyBinding::new("shift-cmd-left", SelectHome, context),
        KeyBinding::new("shift-cmd-right", SelectEnd, context),
        KeyBinding::new("shift-cmd-up", SelectDocStart, context),
        KeyBinding::new("shift-cmd-down", SelectDocEnd, context),
    ];
    if multiline {
        bindings.extend([
            KeyBinding::new("up", Up, context),
            KeyBinding::new("down", Down, context),
            KeyBinding::new("shift-up", SelectUp, context),
            KeyBinding::new("shift-down", SelectDown, context),
        ]);
    }

    let word_prefix = if cfg!(target_os = "macos") {
        "alt"
    } else {
        "ctrl"
    };
    bindings.extend([
        KeyBinding::new(&format!("{word_prefix}-backspace"), DeleteWordLeft, context),
        KeyBinding::new(&format!("{word_prefix}-delete"), DeleteWordRight, context),
        KeyBinding::new(&format!("{word_prefix}-left"), WordLeft, context),
        KeyBinding::new(&format!("{word_prefix}-right"), WordRight, context),
        KeyBinding::new(
            &format!("shift-{word_prefix}-left"),
            SelectWordLeft,
            context,
        ),
        KeyBinding::new(
            &format!("shift-{word_prefix}-right"),
            SelectWordRight,
            context,
        ),
    ]);
    for prefix in ["cmd", "ctrl"] {
        bindings.extend([
            KeyBinding::new(&format!("{prefix}-a"), SelectAll, context),
            KeyBinding::new(&format!("{prefix}-c"), Copy, context),
            KeyBinding::new(&format!("{prefix}-x"), Cut, context),
            KeyBinding::new(&format!("{prefix}-v"), Paste, context),
            KeyBinding::new(&format!("{prefix}-z"), Undo, context),
            KeyBinding::new(&format!("shift-{prefix}-z"), Redo, context),
        ]);
    }
    bindings
}

pub struct InputFactory;

impl CustomElementFactory for InputFactory {
    fn element_type(&self) -> &str {
        "input"
    }

    fn create(&self, _id: u64) -> Box<dyn CustomElement> {
        Box::new(TextEditorElement::new(false))
    }
}

pub struct TextareaFactory;

impl CustomElementFactory for TextareaFactory {
    fn element_type(&self) -> &str {
        "textarea"
    }

    fn create(&self, _id: u64) -> Box<dyn CustomElement> {
        Box::new(TextEditorElement::new(true))
    }
}

struct TextEditorElement {
    multiline: bool,
    value: String,
    placeholder: String,
    read_only: bool,
    min_rows: usize,
    max_rows: usize,
    last_prop_value: Option<String>,
    theme: Theme,
    /// `language` prop: a syntax alias such as `sql`.
    language: Option<String>,
    state: Option<Entity<TextEditorState>>,
}

impl TextEditorElement {
    fn new(multiline: bool) -> Self {
        Self {
            multiline,
            value: String::new(),
            placeholder: String::new(),
            read_only: false,
            min_rows: 1,
            max_rows: if multiline { 10 } else { 1 },
            last_prop_value: None,
            theme: Theme::dark(),
            language: None,
            state: None,
        }
    }

    /// The editor state, created on first use, with this element's props and
    /// listeners applied. Render and `call` both go through it, so a call made
    /// before the first paint sees the latest props too.
    fn sync_state(
        &mut self,
        id: u64,
        events: &HashSet<String>,
        callback: &Option<EventCallback>,
        focus_handle: &FocusHandle,
        cx: &mut Context<crate::renderer::SoloView>,
    ) -> Entity<TextEditorState> {
        let emits_change = events.contains("change");
        let emits_submit = events.contains("submit");
        let emits_key_down = events.contains("keyDown");
        let emits_key_up = events.contains("keyUp");
        let emits_selection_change = events.contains("selectionChange");
        let callback = callback.clone();

        let state = self
            .state
            .get_or_insert_with(|| {
                let value = self.value.clone();
                let placeholder = self.placeholder.clone();
                let multiline = self.multiline;
                let read_only = self.read_only;
                let min_rows = self.min_rows;
                let max_rows = self.max_rows;
                let caret_color = self.theme.caret;
                let syntax_palette = self.theme.syntax.clone();
                let callback = callback.clone();
                let cursor = value.len();
                let state_focus_handle = focus_handle.clone();
                cx.new(move |cx| TextEditorState {
                    element_id: id,
                    callback,
                    emits_change,
                    emits_submit,
                    emits_key_down,
                    emits_key_up,
                    emits_selection_change,
                    focus_handle: state_focus_handle,
                    content: TextStorage::new(value),
                    revisions: RevisionLog::new(),
                    history: UndoHistory::default(),
                    placeholder: placeholder.into(),
                    multiline,
                    read_only,
                    min_rows,
                    max_rows,
                    selected_range: cursor..cursor,
                    selection_reversed: false,
                    marked_range: None,
                    is_selecting: false,
                    scroll_top: 0.0,
                    scroll_left: 0.0,
                    follow_cursor: true,
                    last_lines: Vec::new(),
                    line_starts: vec![0],
                    last_bounds: None,
                    line_height: px(20.0),
                    content_height: 20.0,
                    content_width: 0.0,
                    display_is_placeholder: false,
                    caret_color,
                    blink_anchor: cx.background_executor().now(),
                    blink_task: None,
                    language: None,
                    syntax_palette,
                    highlight: Vec::new(),
                    highlight_task: None,
                    echoes: VecDeque::new(),
                    reported_selection: Selection::caret(cursor),
                })
            })
            .clone();

        let prop_changed = self.last_prop_value.as_ref() != Some(&self.value);
        state.update(cx, |state, cx| {
            state.callback = callback;
            state.emits_change = emits_change;
            state.emits_submit = emits_submit;
            state.emits_key_down = emits_key_down;
            state.emits_key_up = emits_key_up;
            state.emits_selection_change = emits_selection_change;
            state.placeholder = self.placeholder.clone().into();
            state.read_only = self.read_only;
            state.min_rows = self.min_rows.max(1);
            state.max_rows = self.max_rows.max(state.min_rows);
            if state.caret_color != self.theme.caret {
                state.caret_color = self.theme.caret;
                cx.notify();
            }
            state.syntax_palette = self.theme.syntax.clone();
            let language = self
                .language
                .clone()
                .filter(|name| crate::syntax::language_for_alias(name).is_some());
            if state.language != language {
                state.language = language;
                state.highlight.clear();
                state.schedule_highlight(cx);
                cx.notify();
            }
            if prop_changed {
                state.sync_prop_value(self.value.clone(), cx);
            }
        });
        self.last_prop_value = Some(self.value.clone());

        state
    }
}

impl CustomElement for TextEditorElement {
    fn render(
        &mut self,
        ctx: CustomRenderContext,
        _window: &mut Window,
        cx: &mut Context<crate::renderer::SoloView>,
    ) -> gpui::AnyElement {
        let focus_handle = ctx
            .focus_handle
            .cloned()
            .unwrap_or_else(|| cx.focus_handle());
        let state = self.sync_state(ctx.id, ctx.events, ctx.event_callback, &focus_handle, cx);

        let element_id = gpui::SharedString::from(format!("__solo_editor_{}", ctx.id));
        let mut editor = div()
            .id(element_id)
            .relative()
            .flex()
            .min_w_0()
            .w_full()
            .track_focus(&focus_handle)
            .child(crate::automation::bounds_tracker(ctx.id))
            .child(state);
        if let Some(style) = ctx.style {
            editor = crate::renderer::apply_styles(editor, style);
        }
        if ctx.events.contains("click") {
            let callback = ctx.event_callback.clone();
            let id = ctx.id;
            editor = editor.on_click(move |event, _window, _cx| {
                emit_event_full(&callback, id, "click", |payload| {
                    let (x, y) = crate::renderer::point_to_xy(event.position());
                    payload.x = Some(x);
                    payload.y = Some(y);
                    payload.modifiers = Some(event.modifiers().into());
                });
            });
        }
        editor.into_any_element()
    }

    fn set_prop(&mut self, key: &str, value: serde_json::Value) {
        match key {
            "value" => self.value = value.as_str().unwrap_or_default().to_string(),
            "placeholder" => self.placeholder = value.as_str().unwrap_or_default().to_string(),
            "readOnly" => self.read_only = value.as_bool().unwrap_or(false),
            "minRows" => self.min_rows = value.as_u64().unwrap_or(1) as usize,
            "maxRows" => {
                self.max_rows = value
                    .as_u64()
                    .unwrap_or(if self.multiline { 10 } else { 1 })
                    as usize
            }
            "theme" => self.theme = Theme::from_prop(Some(&value)),
            "language" => self.language = value.as_str().map(str::to_string),
            _ => {}
        }
    }

    fn supported_props(&self) -> &[&str] {
        &[
            "value",
            "placeholder",
            "readOnly",
            "minRows",
            "maxRows",
            "theme",
            "language",
        ]
    }

    fn get_prop(&self, key: &str) -> Option<serde_json::Value> {
        match key {
            "value" => Some(self.value.clone().into()),
            "placeholder" => Some(self.placeholder.clone().into()),
            "readOnly" => Some(self.read_only.into()),
            "minRows" => Some(self.min_rows.into()),
            "maxRows" => Some(self.max_rows.into()),
            "language" => self.language.clone().map(Into::into),
            _ => None,
        }
    }

    fn supported_events(&self) -> &[&str] {
        &[
            "change",
            "selectionChange",
            "submit",
            "click",
            "keyDown",
            "keyUp",
            "focus",
            "blur",
        ]
    }

    fn destroy(&mut self) {
        self.state = None;
    }

    fn call(
        &mut self,
        ctx: CustomCallContext,
        method: &str,
        args: serde_json::Value,
        _window: &mut Window,
        cx: &mut Context<crate::renderer::SoloView>,
    ) -> Result<serde_json::Value, String> {
        let focus_handle = ctx
            .focus_handle
            .cloned()
            .unwrap_or_else(|| cx.focus_handle());
        let state = self.sync_state(ctx.id, ctx.events, ctx.event_callback, &focus_handle, cx);
        state.update(cx, |state, cx| state.call(method, args, cx))
    }
}

/// What caused an edit. Reported with `change` events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditOrigin {
    User,
    Undo,
    Redo,
    /// `applyEdits` from JS.
    Api,
    /// A new `value` prop.
    Value,
}

/// A reported text, kept as its length and hash rather than a copy.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Echo {
    len: usize,
    hash: u64,
}

impl Echo {
    fn of(text: &str) -> Self {
        let mut hasher = std::hash::DefaultHasher::new();
        text.hash(&mut hasher);
        Self {
            len: text.len(),
            hash: hasher.finish(),
        }
    }
}

impl EditOrigin {
    fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Api => "api",
            Self::Value => "value",
        }
    }
}

/// An edit or selection from JS, in UTF-16 offsets.
#[derive(serde::Deserialize)]
struct EditArg {
    start: usize,
    end: usize,
    text: String,
}

#[derive(serde::Deserialize)]
struct SelectionArg {
    anchor: usize,
    head: usize,
}

#[derive(serde::Deserialize)]
struct SetSelectionsArgs {
    selections: Vec<SelectionArg>,
    #[serde(default)]
    reveal: bool,
}

#[derive(serde::Deserialize)]
struct ApplyEditsArgs {
    revision: u64,
    edits: Vec<EditArg>,
    selections: Option<Vec<SelectionArg>>,
    #[serde(default = "default_true")]
    undoable: bool,
}

fn default_true() -> bool {
    true
}

fn parse_args<T: serde::de::DeserializeOwned>(method: &str, args: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|error| format!("{method}: {error}"))
}

struct TextEditorState {
    element_id: u64,
    callback: Option<EventCallback>,
    emits_change: bool,
    emits_submit: bool,
    emits_key_down: bool,
    emits_key_up: bool,
    emits_selection_change: bool,
    focus_handle: FocusHandle,
    content: TextStorage,
    revisions: RevisionLog,
    history: UndoHistory,
    placeholder: SharedString,
    multiline: bool,
    read_only: bool,
    min_rows: usize,
    max_rows: usize,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    is_selecting: bool,
    scroll_top: f32,
    scroll_left: f32,
    follow_cursor: bool,
    last_lines: Vec<WrappedLine>,
    line_starts: Vec<usize>,
    last_bounds: Option<Bounds<Pixels>>,
    line_height: Pixels,
    content_height: f32,
    content_width: f32,
    display_is_placeholder: bool,
    caret_color: gpui::Hsla,
    blink_anchor: Instant,
    blink_task: Option<Task<()>>,
    /// The syntax to highlight, as a known alias. `None` paints plain text.
    language: Option<String>,
    syntax_palette: SyntaxPalette,
    /// Highlight spans per source line, relative to the line. Lines edited
    /// since the last parse hold none until the next parse lands.
    highlight: Vec<Vec<HighlightSpan>>,
    highlight_task: Option<Task<()>>,
    /// The texts recent `change` events reported, oldest first. A controlled
    /// `value` that matches one is that event coming back, not a new value.
    echoes: VecDeque<Echo>,
    /// The selection the last `selectionChange` reported.
    reported_selection: Selection,
}

impl TextEditorState {
    fn reset_blink(&mut self, cx: &Context<Self>) {
        self.blink_anchor = cx.background_executor().now();
    }

    /// Parse the text once typing pauses, off the main thread. The result is
    /// stamped with the revision it parsed and dropped if the text moved on;
    /// a newer edit has scheduled its own parse by then.
    fn schedule_highlight(&mut self, cx: &mut Context<Self>) {
        let Some(language) = self.language.clone() else {
            self.highlight_task = None;
            return;
        };
        // Replacing the task drops, and so cancels, the previous one.
        self.highlight_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(HIGHLIGHT_DEBOUNCE_MS))
                .await;
            let Ok((source, revision)) = this.update(cx, |state, _| {
                (state.content.as_str().to_string(), state.revisions.revision())
            }) else {
                return;
            };
            let lines = cx
                .background_executor()
                .spawn(async move {
                    crate::syntax::highlight(HighlightRequest {
                        source: &source,
                        path: None,
                        fence_tag: Some(&language),
                    })
                    .map(|document| document.lines)
                    .unwrap_or_default()
                })
                .await;
            this.update(cx, |state, cx| {
                if state.revisions.revision() == revision {
                    state.highlight = lines;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    fn caret_shown(&mut self, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.focus_handle.is_focused(window) || !window.is_window_active() {
            self.blink_task = None;
            return false;
        }
        if self.blink_task.is_none() {
            self.reset_blink(cx);
            self.blink_task = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor()
                    .timer(Duration::from_millis(CARET_BLINK_MS))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }));
        }
        caret_visible(self.blink_anchor.elapsed().as_millis() as u64)
    }

    fn selection(&self) -> Selection {
        Selection::from_range(self.selected_range.clone(), self.selection_reversed)
    }

    fn set_selection(&mut self, selection: Selection) {
        let anchor = self.content.clamp(selection.anchor);
        let head = self.content.clamp(selection.head);
        let selection = Selection { anchor, head };
        self.selected_range = selection.range();
        self.selection_reversed = selection.reversed();
    }

    /// Apply `edits` in order as one revision and return the edits that undo
    /// them, last first. Callers own the selection and the undo history.
    fn commit(&mut self, edits: Vec<Edit>, origin: EditOrigin, cx: &mut Context<Self>) -> Vec<Edit> {
        let mut changes = Vec::with_capacity(edits.len());
        let mut reported = Vec::new();
        let mut inverse = Vec::with_capacity(edits.len());
        for edit in &edits {
            // The log is in UTF-16, the unit JS holds stale offsets in.
            let change = self.content.utf16_change(edit);
            if self.emits_change {
                reported.push(TextEditPayload {
                    start: change.start as f64,
                    end: change.end() as f64,
                    text: edit.text.clone(),
                });
            }
            changes.push(change);
            shift_line_spans(&mut self.highlight, self.content.as_str(), edit);
            inverse.push(self.content.apply(edit));
        }
        inverse.reverse();
        self.revisions.push(changes);
        self.schedule_highlight(cx);
        if matches!(origin, EditOrigin::User | EditOrigin::Undo | EditOrigin::Redo) {
            self.follow_cursor = true;
        }
        self.reset_blink(cx);
        self.emit_change(reported, origin);
        cx.notify();
        inverse
    }

    /// Replace `range` with `text` as a user edit.
    fn edit_range(&mut self, range: Range<usize>, text: &str, kind: EditKind, cx: &mut Context<Self>) {
        let text = if self.multiline {
            text.to_string()
        } else {
            single_line_text(text)
        };
        let before = self.selection();
        let caret = range.start + text.len();
        let mut inverse = self.commit(vec![Edit::new(range, text)], EditOrigin::User, cx);
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        let now = cx.background_executor().now();
        self.history.record(
            inverse.pop().expect("one edit"),
            before,
            self.selection(),
            kind,
            now,
        );
    }

    /// Replace the selection, or `fallback` when the selection is empty.
    fn delete_selection_or(&mut self, fallback: Range<usize>, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        let range = if self.selected_range.is_empty() {
            fallback
        } else {
            self.selected_range.clone()
        };
        if !range.is_empty() {
            self.edit_range(range, "", EditKind::Delete, cx);
        }
    }

    fn sync_prop_value(&mut self, value: String, cx: &mut Context<Self>) {
        let echo = Echo::of(&value);
        if let Some(index) = self.echoes.iter().rposition(|pending| *pending == echo) {
            self.echoes.drain(..=index);
            return;
        }
        self.echoes.clear();
        self.set_external_text(&value, cx);
    }

    /// Take a new `value` as one edit of what differs, so the selection,
    /// scroll and undo history carry over. The edit itself is not undoable.
    fn set_external_text(&mut self, value: &str, cx: &mut Context<Self>) {
        let Some(edit) = self.content.diff(value) else {
            return;
        };
        let change = edit.change();
        let before = self.selection();
        self.commit(vec![edit], EditOrigin::Value, cx);
        self.set_selection(before.rebase(change));
        self.marked_range = None;
        self.history.rebase(change);
    }

    fn emit_change(&mut self, edits: Vec<TextEditPayload>, origin: EditOrigin) {
        if self.emits_change {
            // A `value` edit is already what the prop says.
            if origin != EditOrigin::Value {
                self.echoes.push_back(Echo::of(self.content.as_str()));
                while self.echoes.len() > 32 {
                    self.echoes.pop_front();
                }
            }
            emit_event_full(&self.callback, self.element_id, "change", |payload| {
                payload.value = Some(self.content.as_str().to_string());
                payload.revision = Some(self.revisions.revision() as f64);
                payload.edits = Some(edits);
                payload.origin = Some(origin.name().to_string());
            });
        }
    }

    /// Report the selection if it moved since the last report. Runs once per
    /// frame from prepaint, after layout, so several moves in one frame make
    /// one event and the caret position matches what is drawn.
    fn emit_selection_change(&mut self, bounds: Bounds<Pixels>) {
        let selection = self.selection();
        if selection == self.reported_selection {
            return;
        }
        self.reported_selection = selection;
        if !self.emits_selection_change {
            return;
        }
        let caret = self.point_for_index(selection.head).map(|caret| TextCaretPayload {
            x: f32::from(bounds.left() + caret.x - px(self.scroll_left)) as f64,
            y: f32::from(bounds.top() + caret.y - px(self.scroll_top)) as f64,
            height: f32::from(self.line_height) as f64,
        });
        let selections = vec![TextSelectionPayload {
            anchor: self.offset_to_utf16(selection.anchor) as f64,
            head: self.offset_to_utf16(selection.head) as f64,
        }];
        let revision = self.revisions.revision() as f64;
        emit_event_full(&self.callback, self.element_id, "selectionChange", |payload| {
            payload.revision = Some(revision);
            payload.selections = Some(selections);
            payload.caret = caret;
        });
    }

    /// Run a controller method from JS. Offsets in and out are UTF-16.
    fn call(
        &mut self,
        method: &str,
        args: serde_json::Value,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        match method {
            "revision" => Ok(self.revisions.revision().into()),
            "getText" => Ok(self.content.as_str().into()),
            "getSelections" => {
                let selection = self.selection();
                Ok(serde_json::json!([{
                    "anchor": self.offset_to_utf16(selection.anchor),
                    "head": self.offset_to_utf16(selection.head),
                }]))
            }
            "setSelections" => {
                let args: SetSelectionsArgs = parse_args(method, args)?;
                let selection = self.primary_selection(&args.selections, method)?;
                self.set_selection(selection);
                self.marked_range = None;
                self.history.break_group();
                self.follow_cursor = args.reveal;
                self.reset_blink(cx);
                cx.notify();
                Ok(serde_json::Value::Null)
            }
            "applyEdits" => {
                let args: ApplyEditsArgs = parse_args(method, args)?;
                self.apply_edits_from_js(args, cx)
            }
            _ => Err(format!("<{}> has no method {method:?}", self.element_type())),
        }
    }

    fn element_type(&self) -> &'static str {
        if self.multiline {
            "textarea"
        } else {
            "input"
        }
    }

    /// The last of `selections`, in bytes of the current text. Phase 1 keeps
    /// one selection, so the others are dropped.
    fn primary_selection(&self, selections: &[SelectionArg], method: &str) -> Result<Selection, String> {
        let primary = selections
            .last()
            .ok_or_else(|| format!("{method}: selections is empty"))?;
        Ok(Selection {
            anchor: self.offset_from_utf16(primary.anchor),
            head: self.offset_from_utf16(primary.head),
        })
    }

    /// `applyEdits`: rebase the edits from their revision to this one, then
    /// apply them as one revision. A rebase that runs into a later edit, or
    /// past the log, rejects the whole call and changes nothing.
    fn apply_edits_from_js(
        &mut self,
        args: ApplyEditsArgs,
        cx: &mut Context<Self>,
    ) -> Result<serde_json::Value, String> {
        let mut edits = args.edits;
        if let Some(edit) = edits.iter().find(|edit| edit.start > edit.end) {
            return Err(format!(
                "applyEdits: edit {}..{} ends before it starts",
                edit.start, edit.end
            ));
        }
        edits.sort_by_key(|edit| edit.start);
        if edits.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return Err("applyEdits: edits overlap".into());
        }

        let rejected = |reason: &str, revision: u64| {
            Ok(serde_json::json!({ "applied": false, "reason": reason, "revision": revision }))
        };
        let mut ranges = Vec::with_capacity(edits.len());
        for edit in &edits {
            match self.revisions.rebase_range(edit.start..edit.end, args.revision) {
                Ok(range) => ranges.push(range),
                Err(RebaseError::Conflict) => return rejected("conflict", self.revisions.revision()),
                Err(RebaseError::Expired) => return rejected("expired", self.revisions.revision()),
            }
        }
        if ranges.windows(2).any(|pair| pair[0].end > pair[1].start) {
            return rejected("conflict", self.revisions.revision());
        }
        let length = self.content.utf16_len();
        if let Some(range) = ranges.iter().find(|range| range.end > length) {
            return Err(format!(
                "applyEdits: edit {}..{} ends past the text, which is {length} long",
                range.start, range.end
            ));
        }

        // Last first, so each edit's offsets hold when it is applied.
        let edits: Vec<Edit> = ranges
            .iter()
            .zip(&edits)
            .rev()
            .map(|(range, edit)| {
                let text = if self.multiline {
                    edit.text.clone()
                } else {
                    single_line_text(&edit.text)
                };
                Edit::new(
                    self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end),
                    text,
                )
            })
            .collect();
        let before = self.selection();
        if !edits.is_empty() {
            let inverse = self.commit(edits.clone(), EditOrigin::Api, cx);
            let after = match &args.selections {
                Some(selections) => self.primary_selection(selections, "applyEdits")?,
                None => edits.iter().fold(before, |selection, edit| selection.rebase(edit.change())),
            };
            self.set_selection(after);
            self.marked_range = None;
            if args.undoable {
                self.history
                    .record_transaction(Transaction::new(inverse, before, self.selection()));
            } else {
                for edit in &edits {
                    self.history.rebase(edit.change());
                }
            }
        } else if let Some(selections) = &args.selections {
            let selection = self.primary_selection(selections, "applyEdits")?;
            self.set_selection(selection);
            self.history.break_group();
            cx.notify();
        }
        Ok(serde_json::json!({ "applied": true, "revision": self.revisions.revision() }))
    }

    fn emit_submit(&self) {
        if self.emits_submit {
            emit_event_full(&self.callback, self.element_id, "submit", |payload| {
                payload.value = Some(self.content.as_str().to_string());
            });
        }
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.content.clamp(offset);
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.history.break_group();
        self.follow_cursor = true;
        self.reset_blink(cx);
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = self.content.clamp(offset);
        self.history.break_group();
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.follow_cursor = true;
        self.reset_blink(cx);
        cx.notify();
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .as_str()
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .as_str()
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        self.content
            .as_str()
            .split_word_bound_indices()
            .rev()
            .find_map(|(index, word)| (index < offset && !word.trim().is_empty()).then_some(index))
            .unwrap_or(0)
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        self.content
            .as_str()
            .split_word_bound_indices()
            .find_map(|(index, word)| {
                let end = index + word.len();
                (end > offset && !word.trim().is_empty()).then_some(end)
            })
            .unwrap_or(self.content.len())
    }

    fn line_range_at(&self, offset: usize) -> Range<usize> {
        self.content.line_range_at(offset)
    }

    fn visual_line_boundary(&self, end: bool) -> usize {
        let Some(cursor) = self.point_for_index(self.cursor_offset()) else {
            let line = self.line_range_at(self.cursor_offset());
            return if end { line.end } else { line.start };
        };
        self.index_for_point(point(
            if end { px(1_000_000.0) } else { px(0.0) },
            cursor.y + px(0.5),
        ))
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(self.previous_boundary(cursor)..cursor, cx);
    }

    fn delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(cursor..self.next_boundary(cursor), cx);
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.previous_boundary(self.cursor_offset())
        } else {
            self.selected_range.start
        };
        self.move_to(offset, cx);
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        let offset = if self.selected_range.is_empty() {
            self.next_boundary(self.cursor_offset())
        } else {
            self.selected_range.end
        };
        self.move_to(offset, cx);
    }

    fn up(&mut self, _: &Up, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(-1.0) {
            self.move_to(offset, cx);
        }
    }

    fn down(&mut self, _: &Down, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(1.0) {
            self.move_to(offset, cx);
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(-1.0) {
            self.select_to(offset, cx);
        }
    }

    fn select_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(offset) = self.vertical_target(1.0) {
            self.select_to(offset, cx);
        }
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        self.history.break_group();
        self.reset_blink(cx);
        cx.notify();
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.visual_line_boundary(false), cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.visual_line_boundary(true), cx);
    }

    fn doc_start(&mut self, _: &DocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn doc_end(&mut self, _: &DocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_home(&mut self, _: &SelectHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.visual_line_boundary(false), cx);
    }

    fn select_end(&mut self, _: &SelectEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.visual_line_boundary(true), cx);
    }

    fn select_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word_boundary(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word_boundary(self.cursor_offset()), cx);
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(self.previous_word_boundary(cursor)..cursor, cx);
    }

    fn delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(cursor..self.next_word_boundary(cursor), cx);
    }

    fn delete_to_line_start(
        &mut self,
        _: &DeleteToLineStart,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(self.line_range_at(cursor).start..cursor, cx);
    }

    fn delete_to_line_end(&mut self, _: &DeleteToLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        self.delete_selection_or(cursor..self.line_range_at(cursor).end, cx);
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content.as_str()[self.selected_range.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if self.read_only || self.selected_range.is_empty() {
            return;
        }
        self.copy(&Copy, window, cx);
        self.edit_range(self.selected_range.clone(), "", EditKind::Other, cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            let range = self.marked_range.clone().unwrap_or(self.selected_range.clone());
            self.edit_range(range, &text, EditKind::Other, cx);
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(transaction) = self.history.pop_undo() {
            let redo = self.commit(transaction.inverse.clone(), EditOrigin::Undo, cx);
            self.marked_range = None;
            self.set_selection(transaction.before);
            self.history.push_redo(transaction.with_inverse(redo));
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.read_only {
            return;
        }
        if let Some(transaction) = self.history.pop_redo() {
            let undo = self.commit(transaction.inverse.clone(), EditOrigin::Redo, cx);
            self.marked_range = None;
            self.set_selection(transaction.after);
            self.history.push_redone(transaction.with_inverse(undo));
        }
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline && !self.read_only {
            let range = self.marked_range.clone().unwrap_or(self.selected_range.clone());
            self.edit_range(range, "\n", EditKind::Other, cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, _: &mut Context<Self>) {
        self.emit_submit();
    }

    fn vertical_target(&self, direction: f32) -> Option<usize> {
        let current = self.point_for_index(self.cursor_offset())?;
        let target_y = f32::from(current.y) + direction * f32::from(self.line_height);
        if target_y < 0.0 {
            return Some(0);
        }
        if target_y >= self.content_height {
            return Some(self.content.len());
        }
        Some(self.index_for_point(point(current.x, px(target_y))))
    }

    fn point_for_index(&self, index: usize) -> Option<Point<Pixels>> {
        for (line_index, line) in self.last_lines.iter().enumerate() {
            let line_start = *self.line_starts.get(line_index)?;
            if index < line_start || index > line_start + line.len() {
                continue;
            }
            let local = line.position_for_index(index - line_start, self.line_height)?;
            let y_offset: Pixels = self
                .last_lines
                .iter()
                .take(line_index)
                .map(|line| line.size(self.line_height).height)
                .sum();
            return Some(point(local.x, local.y + y_offset));
        }
        None
    }

    fn index_for_point(&self, position: Point<Pixels>) -> usize {
        if self.display_is_placeholder {
            return 0;
        }
        let mut y = f32::from(position.y).max(0.0);
        for (line_index, line) in self.last_lines.iter().enumerate() {
            let height = f32::from(line.size(self.line_height).height);
            let line_start = self.line_starts.get(line_index).copied().unwrap_or(0);
            if y < height || line_index + 1 == self.last_lines.len() {
                let local = point(position.x, px(y.min(height - 1.0).max(0.0)));
                let index = line
                    .closest_index_for_position(local, self.line_height)
                    .unwrap_or_else(|index| index);
                return (line_start + index).min(self.content.len());
            }
            y -= height;
        }
        self.content.len()
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        let Some(bounds) = self.last_bounds else {
            return 0;
        };
        self.index_for_point(point(
            position.x - bounds.left() + px(self.scroll_left),
            position.y - bounds.top() + px(self.scroll_top),
        ))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        self.is_selecting = true;
        let index = self.index_for_mouse_position(event.position);
        if event.modifiers.shift {
            self.select_to(index, cx);
        } else {
            self.move_to(index, cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn on_scroll_wheel(
        &mut self,
        event: &ScrollEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // .on_scroll listeners receive every wheel event in the window.
        // Only consume events that actually target this input — without a
        // position check an input with any scrollable overflow swallows
        // scrolling for the whole app (bubble order runs this handler before
        // other elements' scroll handlers).
        let Some(bounds) = self.last_bounds else {
            return;
        };
        let delta = event.delta.pixel_delta(self.line_height);
        let Some(new_scroll_top) = input_wheel_target(
            bounds,
            self.content_height,
            f32::from(self.line_height),
            self.scroll_top,
            event.position,
            f32::from(delta.y),
        ) else {
            return;
        };
        self.scroll_top = new_scroll_top;
        self.follow_cursor = false;
        cx.stop_propagation();
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        self.content.offset_from_utf16(offset)
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        self.content.offset_to_utf16(offset)
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn layout_text(&mut self, width: Pixels, style: &TextStyle, window: &mut Window) -> f32 {
        let (display, is_placeholder) = if self.content.is_empty() {
            (self.placeholder.clone(), true)
        } else {
            (SharedString::from(self.content.as_str().to_string()), false)
        };
        let font_size = style.font_size.to_pixels(window.rem_size());
        self.line_height = window.line_height();
        let color = if is_placeholder {
            gpui::rgba(0x8f8f8fff).into()
        } else {
            style.color
        };
        let runs = if is_placeholder {
            vec![TextRun {
                len: display.len(),
                font: style.font(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            }]
        } else {
            editor_runs(
                self.content.as_str(),
                &self.highlight,
                &self.syntax_palette,
                &style.font(),
                color,
                self.marked_range.as_ref(),
            )
        };
        let wrap_width = self.multiline.then_some(width);
        let lines = window
            .text_system()
            .shape_text(display, font_size, &runs, wrap_width, None)
            .map(|lines| lines.into_vec())
            .unwrap_or_default();
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut offset = 0;
        for line in &lines {
            line_starts.push(offset);
            offset += line.len() + 1;
        }
        if line_starts.is_empty() {
            line_starts.push(0);
        }
        self.content_height = lines
            .iter()
            .map(|line| f32::from(line.size(self.line_height).height))
            .sum::<f32>()
            .max(f32::from(self.line_height));
        self.content_width = lines
            .iter()
            .map(|line| f32::from(line.unwrapped_layout.width))
            .fold(0.0, f32::max);
        self.display_is_placeholder = is_placeholder;
        self.last_lines = lines;
        self.line_starts = line_starts;
        self.content_height
    }

    fn clamp_scroll(&mut self, viewport_width: f32, viewport_height: f32) {
        if self.follow_cursor {
            if let Some(cursor) = self.point_for_index(self.cursor_offset()) {
                let cursor_top = f32::from(cursor.y);
                if cursor_top < self.scroll_top {
                    self.scroll_top = cursor_top;
                } else if cursor_top + f32::from(self.line_height)
                    > self.scroll_top + viewport_height
                {
                    self.scroll_top = cursor_top + f32::from(self.line_height) - viewport_height;
                }
                if !self.multiline {
                    let cursor_left = f32::from(cursor.x);
                    if cursor_left < self.scroll_left {
                        self.scroll_left = cursor_left;
                    } else if cursor_left + 2.0 > self.scroll_left + viewport_width {
                        self.scroll_left = cursor_left + 2.0 - viewport_width;
                    }
                }
            }
        }
        self.scroll_top = self
            .scroll_top
            .clamp(0.0, (self.content_height - viewport_height).max(0.0));
        self.scroll_left = if self.multiline {
            0.0
        } else {
            self.scroll_left
                .clamp(0.0, (self.content_width + 2.0 - viewport_width).max(0.0))
        };
    }
}

impl EntityInputHandler for TextEditorState {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        self.content.as_str().get(range).map(str::to_string)
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.marked_range = None;
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let kind = if self.marked_range.is_some() {
            EditKind::Compose
        } else if new_text.contains(['\n', '\r']) || range.len() > 0 && !new_text.is_empty() {
            EditKind::Other
        } else if new_text.is_empty() {
            EditKind::Delete
        } else {
            EditKind::Insert
        };
        self.edit_range(range, new_text, kind, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.read_only {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let replacement = if self.multiline {
            new_text.to_string()
        } else {
            single_line_text(new_text)
        };
        // The first composition edit starts a transaction like typing does;
        // the rest of the composition joins it.
        let kind = if self.marked_range.is_some() {
            EditKind::Compose
        } else {
            EditKind::Insert
        };
        self.edit_range(range.clone(), &replacement, kind, cx);
        self.marked_range =
            (!replacement.is_empty()).then_some(range.start..range.start + replacement.len());
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|selected| {
                range.start + utf16_offset_to_utf8(&replacement, selected.start)
                    ..range.start + utf16_offset_to_utf8(&replacement, selected.end)
            })
            .unwrap_or_else(|| range.start + replacement.len()..range.start + replacement.len());
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let start = self.point_for_index(range.start)?;
        Some(Bounds::new(
            point(
                bounds.left() + start.x - px(self.scroll_left),
                bounds.top() + start.y - px(self.scroll_top),
            ),
            size(px(2.0), self.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        Some(self.offset_to_utf16(self.index_for_mouse_position(position)))
    }

    fn set_selected_text_range(
        &mut self,
        range_utf16: Range<usize>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = self.range_from_utf16(&range_utf16);
        self.selection_reversed = false;
        self.history.break_group();
        self.follow_cursor = true;
        self.reset_blink(cx);
        cx.notify();
    }

    fn text_length_utf16(&mut self, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        Some(self.content.utf16_len())
    }

    fn accepts_text_input(&self, _: &mut Window, _: &mut Context<Self>) -> bool {
        !self.read_only
    }
}

impl gpui::Render for TextEditorState {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let key_down_callback = self.callback.clone();
        let key_up_callback = self.callback.clone();
        let element_id = self.element_id;
        div()
            .key_context(if self.multiline {
                TEXTAREA_KEY_CONTEXT
            } else {
                INPUT_KEY_CONTEXT
            })
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::up))
            .on_action(cx.listener(Self::down))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_up))
            .on_action(cx.listener(Self::select_down))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::doc_start))
            .on_action(cx.listener(Self::doc_end))
            .on_action(cx.listener(Self::select_home))
            .on_action(cx.listener(Self::select_end))
            .on_action(cx.listener(Self::select_doc_start))
            .on_action(cx.listener(Self::select_doc_end))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_word_right))
            .on_action(cx.listener(Self::delete_to_line_start))
            .on_action(cx.listener(Self::delete_to_line_end))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::submit))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll(cx.listener(Self::on_scroll_wheel))
            .when(self.emits_key_down, move |editor| {
                editor.on_key_down(move |event, _window, _cx| {
                    emit_event_full(&key_down_callback, element_id, "keyDown", |payload| {
                        payload.key = Some(event.keystroke.key.clone());
                        payload.key_char = event.keystroke.key_char.clone();
                        payload.is_held = Some(event.is_held);
                        payload.modifiers = Some(event.keystroke.modifiers.into());
                    });
                })
            })
            .when(self.emits_key_up, move |editor| {
                editor.on_key_up(move |event, _window, _cx| {
                    emit_event_full(&key_up_callback, element_id, "keyUp", |payload| {
                        payload.key = Some(event.keystroke.key.clone());
                        payload.key_char = event.keystroke.key_char.clone();
                        payload.modifiers = Some(event.keystroke.modifiers.into());
                    });
                })
            })
            .w_full()
            .min_w_0()
            .child(EditorTextElement {
                input: cx.entity(),
                min_rows: self.min_rows,
                max_rows: self.max_rows,
            })
    }
}

struct EditorTextElement {
    input: Entity<TextEditorState>,
    min_rows: usize,
    max_rows: usize,
}

struct EditorPrepaint {
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl gpui::Element for EditorTextElement {
    type RequestLayoutState = ();
    type PrepaintState = EditorPrepaint;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        _: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        let input = self.input.clone();
        let text_style = window.text_style();
        let min_rows = self.min_rows;
        let max_rows = self.max_rows.max(min_rows);
        let layout = window.request_measured_layout(style, move |known, available, window, cx| {
            let width = known.width.unwrap_or(match available.width {
                gpui::AvailableSpace::Definite(width) => width,
                _ => px(320.0),
            });
            let (content_height, line_height) = input.update(cx, |input, _| {
                let content_height = input.layout_text(width, &text_style, window);
                (content_height, f32::from(input.line_height))
            });
            let height =
                content_height.clamp(min_rows as f32 * line_height, max_rows as f32 * line_height);
            size(width, px(height))
        });
        (layout, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        cx: &mut App,
    ) -> EditorPrepaint {
        self.input.update(cx, |input, _| {
            input.last_bounds = Some(bounds);
            input.clamp_scroll(f32::from(bounds.size.width), f32::from(bounds.size.height));
            input.emit_selection_change(bounds);
        });
        let input = self.input.read(cx);
        let origin = point(
            bounds.left() - px(input.scroll_left),
            bounds.top() - px(input.scroll_top),
        );
        let mut selection = Vec::new();
        let mut caret = None;
        if input.selected_range.is_empty() || input.display_is_placeholder {
            let caret_point = input
                .point_for_index(input.cursor_offset())
                .unwrap_or(point(px(0.0), px(0.0)));
            caret = Some(fill(
                Bounds::new(
                    point(origin.x + caret_point.x, origin.y + caret_point.y),
                    size(px(2.0), input.line_height),
                ),
                input.caret_color,
            ));
        } else if let (Some(start), Some(end)) = (
            input.point_for_index(input.selected_range.start),
            input.point_for_index(input.selected_range.end),
        ) {
            let color = gpui::rgba(0x7c86ff59);
            if start.y == end.y {
                selection.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(origin.x + end.x, origin.y + start.y + input.line_height),
                    ),
                    color,
                ));
            } else {
                selection.push(fill(
                    Bounds::from_corners(
                        point(origin.x + start.x, origin.y + start.y),
                        point(bounds.right(), origin.y + start.y + input.line_height),
                    ),
                    color,
                ));
                if end.y > start.y + input.line_height {
                    selection.push(fill(
                        Bounds::from_corners(
                            point(origin.x, origin.y + start.y + input.line_height),
                            point(bounds.right(), origin.y + end.y),
                        ),
                        color,
                    ));
                }
                selection.push(fill(
                    Bounds::from_corners(
                        point(origin.x, origin.y + end.y),
                        point(origin.x + end.x, origin.y + end.y + input.line_height),
                    ),
                    color,
                ));
            }
        }
        EditorPrepaint { caret, selection }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        prepaint: &mut EditorPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            for quad in prepaint.selection.drain(..) {
                window.paint_quad(quad);
            }
            let (lines, line_height, scroll_top, scroll_left, display) =
                self.input.update(cx, |input, _| {
                    let display = if input.content.is_empty() {
                        input.placeholder.clone()
                    } else {
                        input.content.as_str().to_string().into()
                    };
                    (
                        std::mem::take(&mut input.last_lines),
                        input.line_height,
                        input.scroll_top,
                        input.scroll_left,
                        display,
                    )
                });
            crate::text::log_painted_text(window, display);
            let mut y = bounds.top() - px(scroll_top);
            for line in &lines {
                let height = line.size(line_height).height;
                line.paint(
                    point(bounds.left() - px(scroll_left), y),
                    line_height,
                    gpui::TextAlign::Left,
                    Some(bounds),
                    window,
                    cx,
                )
                .ok();
                y += height;
            }
            self.input.update(cx, |input, _| input.last_lines = lines);
            let caret_shown = self
                .input
                .update(cx, |input, cx| input.caret_shown(window, cx));
            if caret_shown {
                if let Some(caret) = prepaint.caret.take() {
                    window.paint_quad(caret);
                }
            }
        });
    }
}

impl gpui::IntoElement for EditorTextElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::HighlightKind;

    fn span(range: Range<usize>, kind: HighlightKind) -> HighlightSpan {
        HighlightSpan { range, kind }
    }

    #[test]
    fn runs_color_each_line_and_underline_the_composition() {
        let palette = Theme::dark().syntax;
        let plain = gpui::hsla(0.0, 0.0, 1.0, 1.0);
        let font = gpui::font("Menlo");
        // " é" is three bytes: the run lengths must follow bytes, not chars.
        let text = "ab\nSELECT é";
        let highlight = vec![vec![], vec![span(0..6, HighlightKind::Keyword)]];

        let runs = editor_runs(text, &highlight, &palette, &font, plain, None);
        let lens: Vec<_> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lens, [2, 1, 6, 3]);
        assert_eq!(runs[2].color, palette.keyword);
        assert_eq!(runs[3].color, plain);

        // The composition "b\nSE" splits the first and keyword runs.
        let runs = editor_runs(text, &highlight, &palette, &font, plain, Some(&(1..5)));
        let shape: Vec<_> = runs
            .iter()
            .map(|run| (run.len, run.color == palette.keyword, run.underline.is_some()))
            .collect();
        assert_eq!(
            shape,
            [
                (1, false, false),
                (1, false, true),
                (1, false, true),
                (2, true, true),
                (4, true, false),
                (3, false, false),
            ]
        );
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
    }

    #[test]
    fn runs_without_highlight_are_one_plain_run() {
        let palette = Theme::dark().syntax;
        let plain = gpui::hsla(0.0, 0.0, 1.0, 1.0);
        let runs = editor_runs("a\nb", &[], &palette, &gpui::font("Menlo"), plain, None);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len, 3);
    }

    #[test]
    fn edits_move_untouched_line_spans_and_clear_touched_ones() {
        let keyword = |start| vec![span(start..start + 1, HighlightKind::Keyword)];
        let original = vec![keyword(0), keyword(1), keyword(2)];
        let text = "a\nbb\nccc";

        // Two lines inserted inside line 1: it goes plain, line 2 moves down.
        let mut lines = original.clone();
        shift_line_spans(&mut lines, text, &Edit::new(3..3, "x\ny\n"));
        assert_eq!(lines, [keyword(0), vec![], vec![], vec![], keyword(2)]);

        // Joining lines 0 and 1 leaves one plain line before line 2.
        let mut lines = original.clone();
        shift_line_spans(&mut lines, text, &Edit::new(1..2, ""));
        assert_eq!(lines, [vec![], keyword(2)]);

        // Without spans there is nothing to move.
        let mut lines = Vec::new();
        shift_line_spans(&mut lines, text, &Edit::new(0..0, "\n"));
        assert!(lines.is_empty());
    }

    #[test]
    fn ime_offsets_are_relative_to_replacement_text() {
        assert_eq!(utf16_offset_to_utf8("é🙂", 0), 0);
        assert_eq!(utf16_offset_to_utf8("é🙂", 1), "é".len());
        assert_eq!(utf16_offset_to_utf8("é🙂", 3), "é🙂".len());
    }

    #[test]
    fn single_line_newlines_become_one_space() {
        assert_eq!(single_line_text("a\r\nb\nc\rd"), "a b c d");
    }

    #[test]
    fn caret_blink_phase() {
        assert!(caret_visible(0));
        assert!(caret_visible(CARET_BLINK_MS - 1));
        assert!(!caret_visible(CARET_BLINK_MS));
        assert!(!caret_visible(2 * CARET_BLINK_MS - 1));
        assert!(caret_visible(2 * CARET_BLINK_MS));
    }

    #[test]
    fn caret_color_comes_from_the_input_theme() {
        let mut input = TextEditorElement::new(false);
        input.set_prop("theme", serde_json::json!({ "caret": "#22c55e" }));
        assert_eq!(input.theme.caret, gpui::rgba(0x22c55eff).into());
    }
}

/// Decide whether a wheel event belongs to this input and compute its new
/// scroll top. Returns None when the event must NOT be consumed: it targets
/// something else (position outside bounds) or there is nothing to scroll.
///
/// A free function so the guard has a direct unit test — this is exactly the
/// logic that previously swallowed wheel events app-wide on Linux.
fn input_wheel_target(
    bounds: gpui::Bounds<Pixels>,
    content_height: f32,
    line_height_px: f32,
    scroll_top: f32,
    position: gpui::Point<Pixels>,
    delta_y_pixels: f32,
) -> Option<f32> {
    if !bounds.contains(&position) {
        return None;
    }
    let viewport_height = f32::from(bounds.size.height);
    let max_scroll = (content_height - viewport_height).max(0.0);
    if max_scroll == 0.0 {
        return None;
    }
    Some((scroll_top - delta_y_pixels).clamp(0.0, max_scroll))
}

#[cfg(test)]
mod wheel_guard_tests {
    use super::input_wheel_target;
    use gpui::{point, px, Bounds, Pixels, Point};

    fn bounds(w: f32, h: f32) -> Bounds<Pixels> {
        Bounds {
            origin: point(px(100.0), px(100.0)),
            size: gpui::Size { width: px(w), height: px(h) },
        }
    }

    #[test]
    fn ignores_events_outside_the_input() {
        let b = bounds(200.0, 32.0);
        let far: Point<Pixels> = point(px(10.0), px(10.0));
        assert_eq!(
            input_wheel_target(b, 64.0, 20.0, 0.0, far, -60.0),
            None,
            "wheel over other elements must not be consumed"
        );
    }

    #[test]
    fn consumes_events_inside_a_scrollable_input() {
        let b = bounds(200.0, 32.0);
        let inside: Point<Pixels> = point(px(150.0), px(110.0));
        assert_eq!(input_wheel_target(b, 92.0, 20.0, 0.0, inside, -60.0), Some(60.0));
    }

    #[test]
    fn ignores_events_when_there_is_nothing_to_scroll() {
        let b = bounds(200.0, 32.0);
        let inside: Point<Pixels> = point(px(150.0), px(110.0));
        assert_eq!(
            input_wheel_target(b, 32.0, 20.0, 0.0, inside, -60.0),
            None,
            "single-line input has no scrollable overflow"
        );
    }

    #[test]
    fn clamps_to_scroll_range() {
        let b = bounds(200.0, 32.0);
        let inside: Point<Pixels> = point(px(150.0), px(110.0));
        // content 92, viewport 32 -> max scroll 60.
        assert_eq!(input_wheel_target(b, 92.0, 20.0, 50.0, inside, -600.0), Some(60.0));
        assert_eq!(input_wheel_target(b, 92.0, 20.0, 0.0, inside, 600.0), Some(0.0));
    }
}
