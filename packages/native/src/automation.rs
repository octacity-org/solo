//! Shared automation host: paint bounds and a controllable motion clock.
//!
//! Record bounds during **paint**, not prepaint. The frame reset canvas
//! clears the map in paint, and GPUI prepaint runs for the whole tree
//! before any paint. A prepaint recorder would be wiped by the reset.
//!
//! TestSoloRenderer and SoloRenderer both use this so locators, screenshots,
//! and clock control do not fork between headless tests and a live window.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;
#[cfg(feature = "automation")]
use std::time::Duration;

use gpui::{canvas, px, Bounds, IntoElement, Pixels, Styled, Window, WindowId};
#[cfg(feature = "automation")]
use gpui::{
    point, App, InputEvent, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollEvent, ScrollSource, TouchPhase,
};

#[derive(Clone, Copy, Debug)]
pub struct ElementBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ElementBounds {
    fn from_gpui(bounds: Bounds<Pixels>) -> Self {
        Self {
            x: f64::from(f32::from(bounds.origin.x)),
            y: f64::from(f32::from(bounds.origin.y)),
            width: f64::from(f32::from(bounds.size.width)),
            height: f64::from(f32::from(bounds.size.height)),
        }
    }

}

thread_local! {
    /// Per window, so one window's frame does not drop another's bounds.
    /// Element IDs are unique across windows, so lookups search them all.
    static BOUNDS: RefCell<HashMap<WindowId, HashMap<u64, ElementBounds>>> = RefCell::new(HashMap::new());
}

/// Zero-size canvas. Paint it with the selection reset, before any content.
pub fn bounds_frame_reset() -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let id = window.window_handle().window_id();
            BOUNDS.with(|cell| cell.borrow_mut().entry(id).or_default().clear());
        },
    )
    .absolute()
    .w(px(0.0))
    .h(px(0.0))
}

pub fn record_bounds(window: &Window, id: u64, bounds: Bounds<Pixels>) {
    let window = window.window_handle().window_id();
    BOUNDS.with(|cell| {
        cell.borrow_mut()
            .entry(window)
            .or_default()
            .insert(id, ElementBounds::from_gpui(bounds));
    });
}

pub fn get_bounds(id: u64) -> Option<ElementBounds> {
    BOUNDS.with(|cell| cell.borrow().values().find_map(|bounds| bounds.get(&id).copied()))
}

#[cfg(feature = "automation")]
pub fn all_bounds() -> HashMap<u64, ElementBounds> {
    BOUNDS.with(|cell| {
        cell.borrow()
            .values()
            .flat_map(|bounds| bounds.iter().map(|(&id, &rect)| (id, rect)))
            .collect()
    })
}

/// Drop a closed window's bounds.
pub fn forget_window(id: WindowId) {
    BOUNDS.with(|cell| cell.borrow_mut().remove(&id));
}

pub fn bounds_tracker(id: u64) -> impl IntoElement {
    canvas(
        |bounds, _, _| bounds,
        move |bounds, _, window, _| {
            record_bounds(window, id, bounds);
        },
    )
    .absolute()
    .size_full()
}

enum ClockMode {
    Live,
    #[cfg(feature = "automation")]
    Frozen { now: Instant },
}

struct ClockInner {
    origin: Instant,
    mode: ClockMode,
}

#[derive(Clone)]
pub struct AutomationClock {
    inner: Arc<Mutex<ClockInner>>,
}

impl Default for AutomationClock {
    fn default() -> Self {
        Self::new()
    }
}

impl AutomationClock {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ClockInner {
                origin: Instant::now(),
                mode: ClockMode::Live,
            })),
        }
    }

    pub fn now(&self) -> Instant {
        let inner = self.inner.lock().unwrap();
        match inner.mode {
            ClockMode::Live => Instant::now(),
            #[cfg(feature = "automation")]
            ClockMode::Frozen { now } => now,
        }
    }

    #[allow(dead_code)]
    pub fn now_ms(&self) -> f64 {
        let inner = self.inner.lock().unwrap();
        current_instant(&inner)
            .saturating_duration_since(inner.origin)
            .as_secs_f64()
            * 1000.0
    }

    #[cfg(feature = "automation")]
    pub fn pause(&self) -> f64 {
        let mut inner = self.inner.lock().unwrap();
        let now = current_instant(&inner);
        inner.mode = ClockMode::Frozen { now };
        now.saturating_duration_since(inner.origin).as_secs_f64() * 1000.0
    }

    #[cfg(feature = "automation")]
    pub fn set_ms(&self, now_ms: f64) -> f64 {
        let mut inner = self.inner.lock().unwrap();
        let now = inner.origin + duration_ms(now_ms);
        inner.mode = ClockMode::Frozen { now };
        now_ms
    }

    #[cfg(feature = "automation")]
    pub fn fast_forward_ms(&self, delta_ms: f64) -> f64 {
        let mut inner = self.inner.lock().unwrap();
        let now = current_instant(&inner) + duration_ms(delta_ms);
        inner.mode = ClockMode::Frozen { now };
        now.saturating_duration_since(inner.origin).as_secs_f64() * 1000.0
    }

    #[cfg(feature = "automation")]
    pub fn resume(&self) -> f64 {
        let mut inner = self.inner.lock().unwrap();
        let elapsed = current_instant(&inner).saturating_duration_since(inner.origin);
        inner.origin = Instant::now() - elapsed;
        inner.mode = ClockMode::Live;
        elapsed.as_secs_f64() * 1000.0
    }
}

fn current_instant(inner: &ClockInner) -> Instant {
    match inner.mode {
        ClockMode::Live => Instant::now(),
        #[cfg(feature = "automation")]
        ClockMode::Frozen { now } => now,
    }
}

#[cfg(feature = "automation")]
fn duration_ms(ms: f64) -> Duration {
    Duration::from_secs_f64((ms / 1000.0).max(0.0))
}

#[cfg(feature = "automation")]
pub fn mouse_button(button: u32) -> MouseButton {
    match button {
        1 => MouseButton::Middle,
        2 => MouseButton::Right,
        _ => MouseButton::Left,
    }
}

#[cfg(feature = "automation")]
pub fn dispatch_click(window: &mut Window, cx: &mut App, x: f64, y: f64, button: u32) {
    let position = point(px(x as f32), px(y as f32));
    let button = mouse_button(button);
    window.dispatch_event(
        MouseDownEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
    window.dispatch_event(
        MouseUpEvent {
            button,
            position,
            modifiers: Modifiers::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
}

#[cfg(feature = "automation")]
pub fn dispatch_mouse_down(window: &mut Window, cx: &mut App, x: f64, y: f64, button: u32) {
    window.dispatch_event(
        MouseDownEvent {
            button: mouse_button(button),
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        }
        .to_platform_input(),
        cx,
    );
}

#[cfg(feature = "automation")]
pub fn dispatch_mouse_up(window: &mut Window, cx: &mut App, x: f64, y: f64, button: u32) {
    window.dispatch_event(
        MouseUpEvent {
            button: mouse_button(button),
            position: point(px(x as f32), px(y as f32)),
            modifiers: Modifiers::default(),
            click_count: 1,
        }
        .to_platform_input(),
        cx,
    );
}

/// delta_x/delta_y are in pixels; negative moves up/left.
#[cfg(feature = "automation")]
pub fn dispatch_scroll_wheel(
    window: &mut Window,
    cx: &mut App,
    x: f64,
    y: f64,
    delta_x: f64,
    delta_y: f64,
) {
    window.dispatch_event(
        ScrollEvent {
            position: point(px(x as f32), px(y as f32)),
            delta: ScrollDelta::Pixels(point(px(delta_x as f32), px(delta_y as f32))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
            source: ScrollSource::Pointer,
        }
        .to_platform_input(),
        cx,
    );
}

#[cfg(feature = "automation")]
pub fn dispatch_mouse_move(
    window: &mut Window,
    cx: &mut App,
    x: f64,
    y: f64,
    pressed_button: Option<u32>,
) {
    window.dispatch_event(
        MouseMoveEvent {
            position: point(px(x as f32), px(y as f32)),
            pressed_button: pressed_button.map(mouse_button),
            modifiers: Modifiers::default(),
        }
        .to_platform_input(),
        cx,
    );
}

// ── Keyboard ────────────────────────────────────────────────────────

/// Parse one keystroke, e.g. `"a"`, `"enter"`, `"cmd-shift-p"`.
///
/// Returns a human-readable message on failure: `Keystroke::parse` owns the
/// grammar and its error carries no detail beyond the offending string.
#[cfg(feature = "automation")]
pub fn parse_keystroke(keystroke: &str) -> Result<Keystroke, String> {
    Keystroke::parse(keystroke)
        .map_err(|error| format!("Invalid keystroke '{keystroke}': {error}"))
}

/// Parse a space-separated keystroke string, e.g. `"a enter cmd-shift-p"`.
///
/// Mirrors gpui's own `VisualTestAppContext::simulate_keystrokes` grammar.
/// Empty tokens are dropped so a trailing or repeated space is not an error.
#[cfg(feature = "automation")]
pub fn parse_keystrokes(keystrokes: &str) -> Result<Vec<Keystroke>, String> {
    keystrokes
        .split(' ')
        .filter(|token| !token.is_empty())
        .map(parse_keystroke)
        .collect()
}

/// Type a sequence of keystrokes, as though the user had typed them.
///
/// Routes through `Window::dispatch_keystroke` rather than a bare
/// `KeyDownEvent`. That distinction is the whole point: `dispatch_keystroke`
/// fills in `key_char` via `Keystroke::with_simulated_ime` and then hands it
/// to the focused element's input handler, which is what actually inserts
/// text. Dispatching `KeyDownEvent` directly fires the element's `onKeyDown`
/// listener but never reaches the text buffer.
///
/// Modifier keys (`cmd-a`, `backspace`) have no `key_char`, so they fall
/// through to gpui's key bindings and run as actions instead.
#[cfg(feature = "automation")]
pub fn dispatch_keystrokes(window: &mut Window, cx: &mut App, keystrokes: &[Keystroke]) {
    for keystroke in keystrokes {
        window.dispatch_keystroke(keystroke.clone(), cx);
    }
}

/// A key down with no matching key up, for holding a key or testing the
/// down/up pair separately.
///
/// Deliberately does **not** go through `dispatch_keystroke`, so it never
/// inserts text — matching `TestSoloRenderer::simulate_key_down`. Use
/// [`dispatch_keystrokes`] to type; use this to observe key events.
#[cfg(feature = "automation")]
pub fn dispatch_key_down(
    window: &mut Window,
    cx: &mut App,
    keystroke: &Keystroke,
    is_held: bool,
) {
    window.dispatch_event(
        KeyDownEvent {
            keystroke: keystroke.clone(),
            is_held,
            prefer_character_input: false,
        }
        .to_platform_input(),
        cx,
    );
}

#[cfg(feature = "automation")]
pub fn dispatch_key_up(window: &mut Window, cx: &mut App, keystroke: &Keystroke) {
    window.dispatch_event(
        KeyUpEvent {
            keystroke: keystroke.clone(),
        }
        .to_platform_input(),
        cx,
    );
}

#[cfg(all(test, feature = "automation"))]
mod tests {
    use super::*;

    #[test]
    fn frozen_clock_holds_and_fast_forwards() {
        let clock = AutomationClock::new();
        clock.set_ms(0.0);
        assert!((clock.now_ms() - 0.0).abs() < 0.001);
        clock.fast_forward_ms(150.0);
        assert!((clock.now_ms() - 150.0).abs() < 0.001);
        let later = clock.now();
        clock.fast_forward_ms(150.0);
        assert_eq!(
            clock.now().saturating_duration_since(later),
            Duration::from_millis(150)
        );
    }

    /// `Locator.fill` encodes text as space-separated keystrokes, so the split
    /// is load-bearing: the grammar here is the grammar of the automation API.
    #[test]
    fn parse_keystrokes_splits_on_spaces() {
        let parsed = parse_keystrokes("a b enter").expect("valid keystrokes");
        let keys: Vec<&str> = parsed.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "b", "enter"]);
    }

    /// A trailing or repeated space is not a keystroke. `Keystroke::parse("")`
    /// is an error, so without this a harmless string would fail the whole
    /// command.
    #[test]
    fn parse_keystrokes_ignores_empty_tokens() {
        let parsed = parse_keystrokes("  a   enter  ").expect("valid keystrokes");
        let keys: Vec<&str> = parsed.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(keys, vec!["a", "enter"]);

        assert_eq!(parse_keystrokes("   ").expect("empty is empty"), vec![]);
    }

    /// `fill` sends `cmd-a` on macOS to select all, and `press` sends named
    /// keys straight through. Both must survive parsing as modifiers.
    #[test]
    fn parse_keystrokes_accepts_modifiers() {
        let parsed = parse_keystrokes("cmd-a").expect("cmd-a parses");
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].modifiers.platform, "cmd must set platform");
        assert_eq!(parsed[0].key, "a");

        // `super` and `win` are aliases for the platform modifier.
        for alias in ["super-a", "win-a"] {
            let parsed = parse_keystrokes(alias).unwrap_or_else(|e| panic!("{alias}: {e}"));
            assert!(parsed[0].modifiers.platform, "{alias} must set platform");
        }
    }

    /// Modifiers must not be silently dropped when several are combined.
    #[test]
    fn parse_keystrokes_accepts_modifier_chords() {
        let parsed = parse_keystrokes("cmd-shift-p").expect("chord parses");
        let modifiers = &parsed[0].modifiers;
        assert!(modifiers.platform && modifiers.shift);
        assert!(!modifiers.control && !modifiers.alt);
        assert_eq!(parsed[0].key, "p");
    }

    /// `fill` maps a space to the literal token "space" and a newline to
    /// "enter", so those names must round-trip.
    #[test]
    fn parse_keystrokes_accepts_named_keys() {
        for name in ["space", "enter", "tab", "backspace", "escape", "up"] {
            let parsed = parse_keystrokes(name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(parsed.len(), 1);
            assert_eq!(parsed[0].key, name);
        }
    }

    /// The issue's regression case is Unicode text. Multi-byte characters must
    /// survive `Keystroke::parse`, which branches on byte length and could
    /// otherwise lowercase or reject them.
    #[test]
    fn parse_keystrokes_preserves_unicode() {
        let parsed = parse_keystrokes("İ s t a n b u l space 世 界").expect("unicode parses");
        let keys: Vec<&str> = parsed.iter().map(|k| k.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["İ", "s", "t", "a", "n", "b", "u", "l", "space", "世", "界"]
        );
    }

    /// A bad token must be reported with the token in the message, because the
    /// caller is an automation controller that only sees the string.
    #[test]
    fn parse_keystrokes_reports_the_offending_token() {
        let error = parse_keystrokes("a enter-totally-bogus").expect_err("must fail");
        assert!(
            error.contains("enter-totally-bogus"),
            "error should name the token, got {error:?}"
        );
    }
}
