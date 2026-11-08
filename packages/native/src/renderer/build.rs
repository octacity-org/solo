//! Builds GPUI elements from retained elements, once per frame.

use super::*;

/// Everything `build_element` threads through the tree.
///
/// Split into a struct because the recursion needs eight-plus shared references
/// and adding one more to every call site is how this file rots. `window` and
/// `cx` stay separate parameters: they are `&mut` and gpui reborrows them.
pub(crate) struct BuildCtx<'a> {
    pub tree: &'a RetainedTree,
    pub event_callback: &'a Option<EventCallback>,
    pub focus_handles: &'a HashMap<u64, gpui::FocusHandle>,
    pub scroll_handles: &'a mut HashMap<u64, gpui::ScrollHandle>,
    pub custom_registry: &'a mut CustomElementRegistry,
    pub(super) virtual_lists: &'a mut HashMap<u64, VirtualListEntry>,
    pub motion_states: &'a mut HashMap<u64, crate::motion::MotionState>,
    pub now: std::time::Instant,
    pub motion_active: &'a mut bool,
    pub selection: SharedSelection,
    /// Inherited text state, resolved the way CSS inherits it. The renderer's
    /// own theme only seeds the root selection wash; custom elements resolve
    /// their own theme from their `theme` prop.
    pub inherited: Inherited,
    /// The retained-tree root element, when this build renders from it. The
    /// root div is created without styles by the JS side, so it must be sized
    /// to the window here — otherwise every percentage height below it
    /// resolves against an auto-height parent and collapses to content size,
    /// which makes scroll containers non-scrollable (scroll_max = 0).
    pub root_id: Option<u64>,
}

/// Style properties that cascade into descendants.
#[derive(Clone, Copy)]
pub(crate) struct Inherited {
    /// False once an ancestor sets `userSelect: "none"`.
    pub selectable: bool,
    /// Selection wash colour for this subtree.
    pub selection_wash: gpui::Hsla,
}

impl Inherited {
    pub(super) fn root(theme: &Theme) -> Self {
        let mut wash = theme.accent;
        wash.a = 0.35;
        Self {
            selectable: true,
            selection_wash: wash,
        }
    }

    /// Apply the inheritable parts of `style` for the subtree below it.
    fn descend(mut self, style: Option<&StyleDesc>) -> Self {
        let Some(style) = style else { return self };
        match style.user_select.as_deref() {
            Some("none") => self.selectable = false,
            Some("text") | Some("auto") => self.selectable = true,
            _ => {}
        }
        if let Some(hex) = style.selection_color.as_ref().and_then(|c| c.hex()) {
            self.selection_wash = gpui::rgba(hex).into();
        }
        self
    }
}

// ── Element builders ─────────────────────────────────────────────────

pub(crate) fn build_element(
    id: u64,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::IntoElement;

    let Some(element) = ctx.tree.elements.get(&id) else {
        return gpui::Empty.into_any_element();
    };

    let animated_style = if let Some(source) = element.custom_props.get("motion") {
        let state = match ctx.motion_states.entry(id) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                match crate::motion::MotionState::new(source, ctx.now) {
                    Ok(state) => entry.insert(state),
                    Err(error) => {
                        log::warn!("Invalid motion description for element {id}: {error}");
                        entry.insert(crate::motion::MotionState::invalid(source, ctx.now))
                    }
                }
            }
        };
        if let Err(error) = state.sync(source, ctx.now) {
            log::warn!("Invalid motion update for element {id}: {error}");
        }
        state.is_valid().then(|| {
            let (resolved, active) =
                state.resolve(element.style.as_ref(), element.revision, ctx.now);
            *ctx.motion_active |= active;
            resolved
        })
    } else {
        ctx.motion_states.remove(&id);
        None
    };
    let style = animated_style.as_deref().or(element.style.as_ref());

    // Inheritable style resolves once here so both built-ins and custom
    // elements see the same cascade.
    let parent_inherited = ctx.inherited;
    ctx.inherited = parent_inherited.descend(style);

    let built = match element.element_type.as_str() {
        "div" => {
            ctx.custom_registry.destroy(id);
            build_div(element, style, ctx, window, cx)
        }
        "text" => {
            ctx.custom_registry.destroy(id);
            build_text(element, style, ctx, window, cx)
        }
        "virtual-list" => {
            ctx.custom_registry.destroy(id);
            build_virtual_list(element, ctx, window, cx)
        }

        // Polymorphic dispatch for all custom elements.
        custom_type => {
            let tree = ctx.tree;
            let custom_children: Vec<gpui::AnyElement> = element
                .children
                .iter()
                .copied()
                .filter(|child_id| tree.elements.contains_key(child_id))
                .map(|child_id| build_element(child_id, ctx, window, cx))
                .collect();

            let selection = ctx.selection.clone();
            let inherited = ctx.inherited;
            if let Some((instance, events)) = ctx.custom_registry.sync(element) {
                let render_ctx = CustomRenderContext {
                    id,
                    events,
                    event_callback: ctx.event_callback,
                    focus_handle: ctx.focus_handles.get(&id),
                    style,
                    children: custom_children,
                    selection,
                    selectable: inherited.selectable,
                    selection_wash: inherited.selection_wash,
                };

                instance.render(render_ctx, window, cx)
            } else {
                log::warn!("Unknown element type: {}", custom_type);
                gpui::Empty.into_any_element()
            }
        }
    };

    ctx.inherited = parent_inherited;
    built
}

pub(crate) fn build_div(
    element: &crate::retained_tree::RetainedElement,
    style: Option<&StyleDesc>,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    let mut el = gpui::div().id(gpui::ElementId::NamedInteger(
        gpui::SharedString::new_static("__solo"),
        element.id,
    ));

    // The retained-tree root div is created unstyled by the JS side. Size it
    // to the window so percentage heights below it resolve against a definite
    // parent; without this, an app whose root uses height: "100%" collapses
    // to content height and its scroll containers never overflow.
    if ctx.root_id == Some(element.id) {
        el = el.size_full();
    }

    if let Some(style) = style {
        el = apply_styles(el, style);

        // ── Pseudo-selector styles (hover / active) ──────────────────
        // GPUI's .hover() and .active() take a closure that receives a
        // StyleRefinement and returns it with modifications. Since
        // StyleRefinement implements Styled, we can reuse apply_styles().
        if let Some(ref hover_style) = style.hover {
            el = el.hover(|refinement| apply_styles(refinement, hover_style));
        }
        if let Some(ref active_style) = style.active {
            el = el.active(|refinement| apply_styles(refinement, active_style));
        }
        // Files dragged in from outside the app arrive as a GPUI drag of
        // `ExternalPaths`; GPUI applies this while one hovers the element.
        if let Some(ref drag_style) = style.file_drag_over {
            let refinement = apply_styles(gpui::StyleRefinement::default(), drag_style);
            el = el.drag_over::<gpui::ExternalPaths>(move |_, _, _, _| refinement.clone());
        }

        if crate::style::should_occlude(style) {
            // BlockMouse (occlude) stops the hit test. The parent scroller
            // then never sees the wheel. In-flow fills must use
            // BlockMouseExceptScroll. Keep occlude for overlays that steal
            // the pointer: absolute, fixed, or pointerEvents: "auto".
            let steal_scroll = matches!(style.position.as_deref(), Some("absolute") | Some("fixed"))
                || style.pointer_events.as_deref() == Some("auto");
            el = if steal_scroll {
                el.occlude()
            } else {
                el.block_mouse_except_scroll()
            };
        }
    }

    // ── Overflow: scroll ─────────────────────────────────────────────
    // overflow_scroll() requires StatefulInteractiveElement (only on Stateful<Div>),
    // so we handle it here rather than in apply_styles (which takes E: Styled).
    //
    // CSS precedence: axis-specific props (overflowX/Y) override the shorthand
    // (overflow). E.g. { overflow: "scroll", overflowY: "hidden" } → scroll X only.
    //
    // overflow-x only works as a flex viewport. Default display is Block, so a
    // wide child fills the parent instead of overflowing. Zed's code-block path:
    // flex + min_w_0 on the scroller, flex_none on the child.
    let mut overflow_x_only = false;
    if let Some(style) = style {
        // Resolve each axis: axis-specific overrides shorthand.
        let resolved_x = style.overflow_x.as_deref().or(style.overflow.as_deref());
        let resolved_y = style.overflow_y.as_deref().or(style.overflow.as_deref());

        let needs_scroll_x = resolved_x == Some("scroll");
        let needs_scroll_y = resolved_y == Some("scroll");
        let contains_scroll_x = needs_scroll_x
            && style
                .overscroll_behavior_x
                .as_deref()
                .or(style.overscroll_behavior.as_deref())
                == Some("contain");
        let contains_scroll_y = needs_scroll_y
            && style
                .overscroll_behavior_y
                .as_deref()
                .or(style.overscroll_behavior.as_deref())
                == Some("contain");

        // Scroll containers must be allowed to shrink BELOW their content
        // size. Taffy's automatic minimum size (min-height/min-width: auto)
        // otherwise pins a flex item to its content extent, so the viewport
        // grows with the children, nothing overflows, and every wheel delta
        // clamps straight back to zero (observed on Linux: bounds ==
        // content_size, scroll_max = 0).
        if needs_scroll_x && needs_scroll_y {
            el = el
                .overflow_scroll()
                .min_w(gpui::px(0.))
                .min_h(gpui::px(0.));
        } else if needs_scroll_x {
            overflow_x_only = true;
            el = el
                .flex()
                .min_w_0()
                .overflow_x_scroll()
                .restrict_scroll_to_axis();
        } else if needs_scroll_y {
            el = el
                .overflow_y_scroll()
                .min_h(gpui::px(0.));
        }

        // Attach a persistent ScrollHandle when scrolling is enabled.
        // The handle persists across renders (stored in SoloView::scroll_handles)
        // so GPUI maintains the scroll offset between frames.
        if needs_scroll_x || needs_scroll_y {
            let handle = ctx
                .scroll_handles
                .entry(element.id)
                .or_insert_with(gpui::ScrollHandle::new);
            el = el.track_scroll(handle);

            if contains_scroll_x || contains_scroll_y {
                // GPUI registers this element's built-in scroll handler after
                // custom wheel handlers, then dispatches bubble handlers in
                // reverse order. The element therefore consumes its delta
                // first; stopping here prevents an ancestor from consuming the
                // same gesture even when this scroller is already at a boundary.
                el = el.on_scroll(move |event, window, cx| {
                    let delta = event.delta.pixel_delta(window.line_height());
                    let contains_x = contains_scroll_x && delta.x != gpui::px(0.0);
                    let contains_y = contains_scroll_y && delta.y != gpui::px(0.0);
                    if contains_x || contains_y {
                        cx.stop_propagation();
                    }
                });
            }
        } else {
            // Element is no longer scrollable — remove stale handle.
            ctx.scroll_handles.remove(&element.id);
        }
    } else {
        // No style at all — remove stale handle if it existed.
        ctx.scroll_handles.remove(&element.id);
    }

    // If a FocusHandle was pre-created for this element (by sync_focus_handles),
    // attach it via track_focus. This makes the element focusable — clicking it
    // or tabbing to it gives it keyboard focus. The handle persists across renders
    // because it's stored in SoloView::focus_handles.
    if style.and_then(|style| style.position.as_deref()).is_none() {
        el = el.relative();
    }
    if tracks_bounds(element, style, ctx.tree) {
        el = el.child(crate::automation::bounds_tracker(element.id));
    }

    if let Some(handle) = ctx.focus_handles.get(&element.id) {
        el = el.track_focus(handle);
    }
    if let Some(tab_index) = element
        .custom_props
        .get("tabIndex")
        .and_then(|value| value.as_i64())
        .and_then(|index| isize::try_from(index).ok())
    {
        el = el.tab_index(tab_index).tab_stop(tab_index >= 0);
    }

    // Wire up events.
    // Some events (on_hover, on_click) require a stateful element (.id()),
    // which we already set above. Others (on_mouse_down, on_key_down) work
    // on any InteractiveElement.
    for event_type in &element.events {
        let id = element.id;
        let callback = ctx.event_callback.clone();
        match event_type.as_str() {
            // ── Click ────────────────────────────────────────────
            "click" => {
                el = el.on_click(move |click_event, _window, _cx| {
                    emit_event_full(&callback, id, "click", |p| {
                        let (x, y) = point_to_xy(click_event.position());
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(click_event.modifiers().into());
                        p.click_count = Some(click_event.click_count() as u32);
                        p.is_right_click = Some(click_event.is_right_click());
                    });
                });
            }

            // ── Mouse down (all buttons) ─────────────────────────
            "mouseDown" => {
                // Wire all three buttons so JS gets right-click, middle-click, etc.
                for &button in &[
                    gpui::MouseButton::Left,
                    gpui::MouseButton::Middle,
                    gpui::MouseButton::Right,
                ] {
                    let callback = callback.clone();
                    el = el.on_mouse_down(button, move |mouse_event, _window, _cx| {
                        emit_event_full(&callback, id, "mouseDown", |p| {
                            let (x, y) = point_to_xy(mouse_event.position);
                            p.x = Some(x);
                            p.y = Some(y);
                            p.button = Some(mouse_button_to_u32(mouse_event.button));
                            p.click_count = Some(mouse_event.click_count as u32);
                            p.modifiers = Some(mouse_event.modifiers.into());
                        });
                    });
                }
            }

            // ── Mouse up (all buttons) ───────────────────────────
            "mouseUp" => {
                for &button in &[
                    gpui::MouseButton::Left,
                    gpui::MouseButton::Middle,
                    gpui::MouseButton::Right,
                ] {
                    let callback = callback.clone();
                    el = el.on_mouse_up(button, move |mouse_event, _window, _cx| {
                        emit_event_full(&callback, id, "mouseUp", |p| {
                            let (x, y) = point_to_xy(mouse_event.position);
                            p.x = Some(x);
                            p.y = Some(y);
                            p.button = Some(mouse_button_to_u32(mouse_event.button));
                            p.click_count = Some(mouse_event.click_count as u32);
                            p.modifiers = Some(mouse_event.modifiers.into());
                        });
                    });
                }
            }

            // ── Mouse move ───────────────────────────────────────
            "mouseMove" => {
                el = el.on_mouse_move(move |mouse_event, _window, _cx| {
                    emit_event_full(&callback, id, "mouseMove", |p| {
                        let (x, y) = point_to_xy(mouse_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(mouse_event.modifiers.into());
                        p.pressed_button = mouse_event.pressed_button.map(mouse_button_to_u32);
                    });
                });
            }

            // ── Hover (mouseEnter + mouseLeave) ──────────────────
            // GPUI's on_hover fires with true on enter, false on leave.
            // We split into two distinct event types for the Solid side.
            "mouseEnter" | "mouseLeave" => {
                // Only wire once even if both mouseEnter and mouseLeave are registered.
                // Check if we already wired on_hover via the other event.
                let has_enter = element.events.contains("mouseEnter");
                let has_leave = element.events.contains("mouseLeave");
                // Wire on first encounter (mouseEnter sorts before mouseLeave).
                if event_type.as_str() == "mouseEnter" || !has_enter {
                    let callback_enter = if has_enter {
                        ctx.event_callback.clone()
                    } else {
                        None
                    };
                    let callback_leave = if has_leave {
                        ctx.event_callback.clone()
                    } else {
                        None
                    };
                    el = el.on_hover(move |&is_hovered, _window, _cx| {
                        if is_hovered {
                            emit_event_full(&callback_enter, id, "mouseEnter", |p| {
                                p.hovered = Some(true);
                            });
                        } else {
                            emit_event_full(&callback_leave, id, "mouseLeave", |p| {
                                p.hovered = Some(false);
                            });
                        }
                    });
                }
            }

            // ── Mouse down outside ───────────────────────────────
            // Fires when the user clicks OUTSIDE this element.
            // Critical for "click outside to close" pattern (dropdowns, modals).
            "mouseDownOutside" => {
                el = el.on_mouse_down_out(move |mouse_event, _window, _cx| {
                    emit_event_full(&callback, id, "mouseDownOutside", |p| {
                        let (x, y) = point_to_xy(mouse_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.button = Some(mouse_button_to_u32(mouse_event.button));
                        p.modifiers = Some(mouse_event.modifiers.into());
                    });
                });
            }

            // ── Scroll wheel ─────────────────────────────────────
            "scroll" => {
                el = el.on_scroll(move |scroll_event, _window, _cx| {
                    emit_event_full(&callback, id, "scroll", |p| {
                        let (x, y) = point_to_xy(scroll_event.position);
                        p.x = Some(x);
                        p.y = Some(y);
                        p.modifiers = Some(scroll_event.modifiers.into());
                        p.precise = Some(scroll_event.delta.precise());

                        // Convert ScrollDelta to pixel values.
                        // For Lines delta, we use a default line height of 20px.
                        let line_height = gpui::px(20.0);
                        let pixel_delta = scroll_event.delta.pixel_delta(line_height);
                        p.delta_x = Some(f64::from(f32::from(pixel_delta.x)));
                        p.delta_y = Some(f64::from(f32::from(pixel_delta.y)));

                        p.touch_phase = Some(match scroll_event.touch_phase {
                            gpui::TouchPhase::Started => "started".to_string(),
                            gpui::TouchPhase::Moved => "moved".to_string(),
                            gpui::TouchPhase::Ended => "ended".to_string(),
                            gpui::TouchPhase::Cancelled => "cancelled".to_string(),
                        });
                    });
                });
            }

            // ── Key down ─────────────────────────────────────────
            // Requires .focusable() (set above). Element must be focused
            // (clicked or tabbed to) for these to fire.
            "keyDown" => {
                el = el.on_key_down(move |key_event, _window, _cx| {
                    emit_event_full(&callback, id, "keyDown", |p| {
                        p.key = Some(key_event.keystroke.key.clone());
                        p.key_char = key_event.keystroke.key_char.clone();
                        p.is_held = Some(key_event.is_held);
                        p.modifiers = Some(key_event.keystroke.modifiers.into());
                    });
                });
            }

            // ── Key up ───────────────────────────────────────────
            "keyUp" => {
                el = el.on_key_up(move |key_event, _window, _cx| {
                    emit_event_full(&callback, id, "keyUp", |p| {
                        p.key = Some(key_event.keystroke.key.clone());
                        p.key_char = key_event.keystroke.key_char.clone();
                        p.modifiers = Some(key_event.keystroke.modifiers.into());
                    });
                });
            }

            // ── File drop ────────────────────────────────────────
            // GPUI turns a platform file drag (Finder) into a drag of
            // `ExternalPaths`; the innermost element with this listener
            // under the pointer takes the drop.
            "fileDrop" => {
                el = el.on_drop(move |external: &gpui::ExternalPaths, window, _cx| {
                    emit_event_full(&callback, id, "fileDrop", |p| {
                        let (x, y) = point_to_xy(window.mouse_position());
                        p.x = Some(x);
                        p.y = Some(y);
                        p.paths = Some(
                            external
                                .paths()
                                .iter()
                                .map(|path| path.to_string_lossy().into_owned())
                                .collect(),
                        );
                    });
                });
            }

            // ── Focus / Blur ─────────────────────────────────────
            // Event emission is handled by FocusHandle subscriptions
            // set up in SoloView::sync_focus_handles(). The handle is
            // attached to this element via .track_focus() above.
            "focus" | "blur" => {}

            _ => {}
        }
    }

    // Text content — selectable, same as a <text> leaf.
    if let Some(ref content) = element.content {
        el = el.child(text_content(element.id, content, ctx));
    }

    // Children
    for &child_id in &element.children {
        let child = build_element(child_id, ctx, window, cx);
        el = if overflow_x_only {
            el.child(gpui::div().flex_none().child(child))
        } else {
            el.child(child)
        };
    }

    el.into_any_element()
}

/// Whether `style` makes an element a scroll container on either axis.
pub(super) fn scrolls(style: &StyleDesc) -> bool {
    let shorthand = style.overflow.as_deref();
    style.overflow_x.as_deref().or(shorthand) == Some("scroll")
        || style.overflow_y.as_deref().or(shorthand) == Some("scroll")
}

/// Whether to record this element's painted bounds this frame.
///
/// Automation reads every element's bounds. Without it, only three readers
/// remain: context menus anchor to an element with listeners, and
/// `scrollToItem` measures a scroll container against its children. Release
/// builds skip the tracker everywhere else, one canvas per element.
pub(super) fn tracks_bounds(
    element: &crate::retained_tree::RetainedElement,
    style: Option<&StyleDesc>,
    tree: &RetainedTree,
) -> bool {
    cfg!(feature = "automation")
        || !element.events.is_empty()
        || style.is_some_and(scrolls)
        || element
            .parent
            .and_then(|parent| tree.elements.get(&parent))
            .and_then(|parent| parent.style.as_ref())
            .is_some_and(scrolls)
}

/// A selectable text run owned by `element_id`. Runs are left to gpui so the
/// text keeps inheriting colour, weight and family from ancestor styles.
pub(super) fn text_content(
    element_id: u64,
    content: &gpui::SharedString,
    ctx: &BuildCtx,
) -> gpui::AnyElement {
    if !ctx.inherited.selectable {
        // Still logged: `getPaintedText()` promises every painted string, and a
        // `userSelect: "none"` label is exactly the chrome tests want to assert.
        return crate::text::chrome_text(content.clone(), None);
    }
    selectable_text(crate::text::SelectableText::new(
        content.clone(),
        None,
        selection_key(element_id, 0),
        ctx.selection.clone(),
        ctx.inherited.selection_wash,
    ))
}

pub(crate) fn build_text(
    element: &crate::retained_tree::RetainedElement,
    style: Option<&StyleDesc>,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    // Fast path: plain text leaf without style. It still goes through
    // `text_content` so the glyphs land in the selection registry — the old
    // raw-string return was the reason text was not selectable.
    if style.is_none() && element.children.is_empty() {
        let content = element.content.clone().unwrap_or_default();
        let mut el = gpui::div().relative();
        if tracks_bounds(element, None, ctx.tree) {
            el = el.child(crate::automation::bounds_tracker(element.id));
        }
        return el
            .child(text_content(element.id, &content, ctx))
            .into_any_element();
    }

    // The full style set, exactly as `<div>` gets it. `<text>` used to apply a
    // text-only subset, so `padding`, `width` and every layout prop on a text
    // node were silently dropped — a hole with no error and no warning.
    let mut el = gpui::div();
    if let Some(style) = style {
        el = apply_styles(el, style);
    }
    if style.and_then(|style| style.position.as_deref()).is_none() {
        el = el.relative();
    }
    if tracks_bounds(element, style, ctx.tree) {
        el = el.child(crate::automation::bounds_tracker(element.id));
    }

    if let Some(ref content) = element.content {
        el = el.child(text_content(element.id, content, ctx));
    }

    for &child_id in &element.children {
        el = el.child(build_element(child_id, ctx, window, cx));
    }

    el.into_any_element()
}
