//! `SoloView`: the GPUI view that paints a window from the retained tree.

use super::*;

// ── GPUI View ────────────────────────────────────────────────────────

/// Scroll a retained child into view using its painted bounds.
///
/// GPUI's `ScrollHandle::scroll_to_item` only tracks layout children that its
/// div records internally. Solo also injects absolute automation canvases, so
/// retained child indexes are not guaranteed to match that internal list.
pub(super) fn scroll_retained_child_into_view(
    tree: &Arc<Mutex<RetainedTree>>,
    handle: &gpui::ScrollHandle,
    parent_id: u64,
    index: usize,
) -> bool {
    let child_id = {
        let tree = tree.lock().unwrap();
        tree.elements
            .get(&parent_id)
            .and_then(|parent| parent.children.get(index))
            .copied()
    };
    let Some(child_id) = child_id else {
        return false;
    };
    let Some(parent) = crate::automation::get_bounds(parent_id) else {
        return false;
    };
    let Some(child) = crate::automation::get_bounds(child_id) else {
        return false;
    };

    let offset = handle.offset();
    let mut x = f64::from(f32::from(offset.x));
    let mut y = f64::from(f32::from(offset.y));
    if child.x < parent.x {
        x += parent.x - child.x;
    } else if child.x + child.width > parent.x + parent.width {
        x += parent.x + parent.width - child.x - child.width;
    }
    if child.y < parent.y {
        y += parent.y - child.y;
    } else if child.y + child.height > parent.y + parent.height {
        y += parent.y + parent.height - child.y - child.height;
    }
    handle.set_offset(gpui::point(gpui::px(x as f32), gpui::px(y as f32)));
    true
}

/// The root element a `WindowTarget` addresses, `None` for the main window.
#[cfg(any(target_os = "macos", ui_thread))]
pub(super) fn tab_root(target: super::WindowTarget) -> Option<u64> {
    match target {
        super::WindowTarget::Main => None,
        super::WindowTarget::Root(root) => Some(root),
    }
}

/// `title`, prefixed with `*` while the document has unsaved changes. No Linux
/// window protocol carries an edited flag, so the title is the only place the
/// window manager can show one.
#[cfg(not(target_os = "macos"))]
fn marked_title(title: &str, edited: bool) -> String {
    if edited {
        format!("*{title}")
    } else {
        title.to_string()
    }
}

/// One window hosted by a native window: the root it paints and the window
/// state the app can set for it.
///
/// A native window owns one tab until another window with the same tabbing
/// identifier joins it; Linux has no native tabs, so Solo draws the strip (see
/// `renderer::tabs`).
pub(crate) struct Tab {
    /// The element this tab paints, `None` for the main window's tree root.
    pub(crate) root: Option<u64>,
    pub(crate) title: String,
    /// `setDocumentEdited`.
    pub(crate) document_edited: bool,
    /// `setDocumentPath`.
    pub(crate) document_path: Option<String>,
    /// The app handles close requests: the close button and `performClose:`
    /// leave the window open and emit `windowCloseRequested` instead.
    pub(crate) close_guarded: bool,
}

impl Tab {
    fn new(root: Option<u64>, title: String) -> Self {
        Self {
            root,
            title,
            document_edited: false,
            document_path: None,
            close_guarded: false,
        }
    }

    /// The title the platform shows: [`marked_title`] off macOS. The tab strip
    /// shows the same string, so a window's tab says what its title says.
    pub(super) fn native_title(&self) -> String {
        #[cfg(target_os = "macos")]
        return self.title.clone();

        #[cfg(not(target_os = "macos"))]
        marked_title(&self.title, self.document_edited)
    }
}

pub(crate) struct SoloView {
    pub(crate) tree: Arc<Mutex<RetainedTree>>,
    pub(crate) event_callback: Option<EventCallback>,
    /// The windows this native window hosts, in tab-strip order. Always at
    /// least one.
    pub(crate) tabs: Vec<Tab>,
    /// The tab whose tree paints.
    pub(super) active: usize,
    /// The title last given to the platform window.
    applied_title: Option<String>,
    /// The tree generation, command state and root that focus handles and
    /// per-element state were last synced against; unchanged, render skips it.
    synced: Option<(u64, bool, Option<u64>)>,
    /// Persistent FocusHandles keyed by element ID.
    /// Created lazily for elements with keyboard or focus/blur listeners.
    /// Handles persist across renders so GPUI maintains focus state.
    pub(crate) focus_handles: HashMap<u64, gpui::FocusHandle>,
    /// Active focus/blur subscriptions keyed by element and event type.
    pub(crate) focus_subscriptions: HashMap<(u64, String), gpui::Subscription>,
    /// Registry for custom element types (input, editor, diff, etc.).
    /// Stores factories (one per type) and live instances (one per element ID).
    pub(crate) custom_registry: CustomElementRegistry,
    /// Persistent ScrollHandles keyed by element ID.
    /// Created lazily for elements with overflow: "scroll" (or per-axis scroll).
    /// Handles persist across renders so GPUI maintains scroll offset state.
    pub(crate) scroll_handles: HashMap<u64, gpui::ScrollHandle>,
    /// Native animation clocks keyed by retained element ID.
    pub(crate) motion_states: HashMap<u64, crate::motion::MotionState>,
    /// Live text selection, shared with the paint closures and the napi methods.
    pub(crate) selection: SharedSelection,
    /// Persistent measurement and scroll state for Solid-backed virtual lists.
    virtual_lists: HashMap<u64, VirtualListEntry>,
    /// Motion / review clock. Live wall time unless automation freezes it.
    pub(crate) clock: crate::automation::AutomationClock,
    /// The single live system-appearance observer, when subscribed by Solid.
    /// Replacing this subscription drops the previous observer first.
    pub(super) system_appearance_subscription: Option<gpui::Subscription>,
    /// Releases custom-element entities on app quit. Without this, gpui's
    /// exit leak check panics because quitting does not drop root views.
    quit_hook: Option<gpui::Subscription>,
    /// The app draws its own titlebar (`titlebarTransparent`), so the Linux
    /// client-decoration titlebar stays off.
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    pub(super) titlebar_hidden: bool,
    /// The window's close button asked its guarded tabs: the native window
    /// closes once the last of them has, including any unguarded tabs it also
    /// hosts. Cleared when a later close request replaces it.
    #[cfg_attr(not(ui_thread), allow(dead_code))]
    close_pending: std::cell::Cell<bool>,
}

impl SoloView {
    pub(crate) fn new(
        tree: Arc<Mutex<RetainedTree>>,
        event_callback: Option<EventCallback>,
        window_title: String,
        selection: SharedSelection,
    ) -> Self {
        Self {
            tree,
            event_callback,
            tabs: vec![Tab::new(None, window_title)],
            active: 0,
            applied_title: None,
            synced: None,
            focus_handles: HashMap::new(),
            focus_subscriptions: HashMap::new(),
            custom_registry: CustomElementRegistry::with_defaults(),
            scroll_handles: HashMap::new(),
            motion_states: HashMap::new(),
            selection,
            virtual_lists: HashMap::new(),
            clock: crate::automation::AutomationClock::new(),
            system_appearance_subscription: None,
            quit_hook: None,
            titlebar_hidden: false,
            close_pending: std::cell::Cell::new(false),
        }
    }

    /// The tab painting this native window.
    pub(super) fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }

    /// The tab a window-scoped call acts on: the one addressed by `target`,
    /// or the active one when the window is not a tab.
    pub(super) fn tab_for(&self, target: super::WindowTarget) -> &Tab {
        self.tabs
            .iter()
            .find(|tab| tab.root == tab_root(target))
            .unwrap_or_else(|| self.tab())
    }

    /// Mutable [`SoloView::tab_for`].
    pub(crate) fn tab_for_mut(&mut self, target: super::WindowTarget) -> &mut Tab {
        match self.tabs.iter().position(|tab| tab.root == tab_root(target)) {
            Some(index) => &mut self.tabs[index],
            None => &mut self.tabs[self.active],
        }
    }

    /// The title the platform shows, for the tab it paints.
    pub(crate) fn native_title(&self) -> String {
        self.tab().native_title()
    }

    /// Add `root` as a tab of this native window and select it. The app title
    /// of a tab is the tab strip's text; the native title, the tab's.
    pub(super) fn add_tab(&mut self, root: u64, title: String) {
        self.tabs.push(Tab::new(Some(root), title));
        self.active = self.tabs.len() - 1;
    }

    /// Select the tab painting `root`.
    pub(super) fn activate_tab(&mut self, root: u64) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.root == Some(root)) {
            self.active = index;
        }
    }

    /// Take the tab painting `root` out of this window, selecting a neighbour.
    /// Returns false when it was not hosted here.
    pub(super) fn remove_tab(&mut self, root: u64) -> bool {
        let Some(index) = self.tabs.iter().position(|tab| tab.root == Some(root)) else {
            return false;
        };
        self.tabs.remove(index);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        true
    }

    /// `titlebarTransparent`: the app paints its own titlebar, so Solo draws
    /// none around a client-decorated window.
    #[cfg(any(target_os = "macos", ui_thread))]
    #[cfg_attr(not(any(target_os = "linux", target_os = "freebsd")), allow(dead_code))]
    pub(crate) fn with_titlebar_hidden(mut self, hidden: bool) -> Self {
        self.titlebar_hidden = hidden;
        self
    }

    /// Paint `root` instead of the tree's main root, sharing `clock`.
    #[cfg(any(target_os = "macos", ui_thread))]
    pub(crate) fn for_window_root(
        mut self,
        root: u64,
        clock: Option<crate::automation::AutomationClock>,
    ) -> Self {
        self.tabs[0].root = Some(root);
        if let Some(clock) = clock {
            self.clock = clock;
        }
        self
    }

    /// Whether a close request may close the window now. A guarded window
    /// stays open and emits `windowCloseRequested` on its root element, or on
    /// element 0 for the main window.
    ///
    /// The window's close button closes the whole native window, so every
    /// guarded tab is asked, not just the selected one: a background tab with
    /// unsaved work must not be discarded silently.
    #[cfg(any(target_os = "macos", ui_thread))]
    pub(crate) fn allow_close(&self) -> bool {
        let mut guarded = false;
        for tab in &self.tabs {
            if !tab.close_guarded {
                continue;
            }
            guarded = true;
            super::emit_event_full(
                &self.event_callback,
                tab.root.unwrap_or(0),
                super::WINDOW_CLOSE_REQUESTED_EVENT,
                |_| {},
            );
        }
        // Remember the intent: confirming every prompt closes the window with
        // whatever tabs it still hosts, not just the one whose prompt came
        // last. A new request recomputes it.
        self.close_pending.set(guarded);
        !guarded
    }

    /// Whether a window-level close is waiting for its guarded tabs.
    #[cfg(any(ui_thread, test))]
    pub(super) fn pending_close(&self) -> bool {
        self.close_pending.get()
    }

    /// Drop a window-level close intent, e.g. when the user asks for one tab
    /// specifically instead.
    #[cfg(any(ui_thread, test))]
    pub(super) fn clear_pending_close(&self) {
        self.close_pending.set(false);
    }

    /// After a tab closed: whether this completes a pending window close,
    /// because every tab the close asked is gone and only unguarded tabs
    /// remain. The window must then close as a whole.
    #[cfg(any(ui_thread, test))]
    pub(super) fn pending_close_complete(&self) -> bool {
        if !self.close_pending.get() || self.tabs.iter().any(|tab| tab.close_guarded) {
            return false;
        }
        self.close_pending.set(false);
        true
    }

    /// Ask the tab `target` addresses to close: a guarded tab stays open and
    /// emits `windowCloseRequested` on its root element. True when it may
    /// close now.
    #[cfg(any(target_os = "macos", ui_thread))]
    pub(super) fn request_close(&self, target: super::WindowTarget) -> bool {
        let tab = self.tab_for(target);
        if !tab.close_guarded {
            return true;
        }
        super::emit_event_full(
            &self.event_callback,
            tab.root.unwrap_or(0),
            super::WINDOW_CLOSE_REQUESTED_EVENT,
            |_| {},
        );
        false
    }

    /// The element window-scoped events are reported on: the window's own
    /// root, or element 0 for the main window.
    #[cfg(any(target_os = "macos", ui_thread))]
    pub(super) fn root_for_events(&self) -> u64 {
        self.tab().root.unwrap_or(0)
    }

    fn root_id(&self, tree: &RetainedTree) -> Option<u64> {
        self.tab().root.or(tree.root_id)
    }

    pub(crate) fn set_system_appearance_subscription(
        &mut self,
        token: Option<String>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> Result<String> {
        if token.as_deref().is_some_and(str::is_empty) {
            return Err(Error::from_reason(
                "System appearance subscription token must not be empty",
            ));
        }

        self.system_appearance_subscription.take();
        let current = window.appearance();

        if let Some(token) = token {
            self.system_appearance_subscription = Some(cx.observe_window_appearance(
                window,
                move |view, window, _cx| {
                    let value = crate::system_appearance::appearance_event_value(
                        &token,
                        window.appearance(),
                    );
                    let appearance = crate::system_appearance::normalized_appearance(
                        window.appearance(),
                    )
                    .to_string();
                    record_system_appearance(&appearance);
                    emit_event_full(
                        &view.event_callback,
                        0,
                        "systemAppearanceChange",
                        |payload| payload.value = Some(value),
                    );
                },
            ));
        }

        Ok(crate::system_appearance::normalized_appearance(current).to_string())
    }

    pub(super) fn build_virtual_child(
        &mut self,
        list_id: u64,
        index: usize,
        expected_child_id: u64,
        inherited: Inherited,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::AnyElement {
        use gpui::prelude::*;

        let row_focus_handle = self.virtual_lists.get_mut(&list_id).and_then(|entry| {
            entry.seen_rows.insert(expected_child_id);
            entry.row_focus_handle(index, expected_child_id)
        });

        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        // Rows can move between this list's snapshot and its layout. Build the
        // snapshot's row while it is still in the list: an empty stand-in is
        // measured as zero height and keeps it until the row is laid out again.
        let child_in_list = tree
            .elements
            .get(&expected_child_id)
            .is_some_and(|child| child.parent == Some(list_id));
        if !child_in_list {
            return gpui::Empty.into_any_element();
        }

        let callback = self.event_callback.clone();
        let now = self.clock.now();
        let mut motion_active = false;
        let mut build_ctx = BuildCtx {
            tree: &tree,
            event_callback: &callback,
            focus_handles: &self.focus_handles,
            scroll_handles: &mut self.scroll_handles,
            custom_registry: &mut self.custom_registry,
            virtual_lists: &mut self.virtual_lists,
            motion_states: &mut self.motion_states,
            now,
            motion_active: &mut motion_active,
            selection: self.selection.clone(),
            inherited,
            root_id: None,
        };
        let child = build_element(expected_child_id, &mut build_ctx, window, cx);
        if motion_active {
            window.request_animation_frame();
        }
        let Some(focus_handle) = row_focus_handle else {
            return child;
        };
        gpui::div()
            .id(gpui::SharedString::from(format!(
                "__solo_virtual_row_{}_{}",
                list_id, expected_child_id
            )))
            .w_full()
            .track_focus(&focus_handle)
            .child(child)
            .into_any_element()
    }

    fn scroll_virtual_list_to_item(&self, id: u64, index: usize) -> bool {
        let Some(entry) = self.virtual_lists.get(&id) else {
            return false;
        };
        entry.state.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
        true
    }

    fn set_virtual_list_offset(&self, id: u64, x: f32, y: f32) -> bool {
        let Some(entry) = self.virtual_lists.get(&id) else {
            return false;
        };
        entry
            .state
            .set_offset_from_scrollbar(gpui::point(gpui::px(x), gpui::px(y)));
        true
    }

    fn virtual_list_offset(&self, id: u64) -> Option<[f64; 2]> {
        let offset = self
            .virtual_lists
            .get(&id)?
            .state
            .scroll_px_offset_for_scrollbar();
        Some([
            f64::from(f32::from(offset.x)),
            f64::from(f32::from(offset.y)),
        ])
    }

    pub(crate) fn reveal_virtual_list_ancestor(&self, id: u64) -> bool {
        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        let mut current = id;
        let location = loop {
            let Some(parent_id) = tree
                .elements
                .get(&current)
                .and_then(|element| element.parent)
            else {
                break None;
            };
            if let Some(entry) = self.virtual_lists.get(&parent_id) {
                // A data-driven list places each row by its item index.
                let index = if entry.data.is_some() {
                    tree.elements
                        .get(&current)
                        .and_then(|row| row.custom_props.get("itemIndex"))
                        .and_then(serde_json::Value::as_u64)
                        .and_then(|index| usize::try_from(index).ok())
                } else {
                    tree.elements
                        .get(&parent_id)
                        .and_then(|parent| parent.children.iter().position(|child| *child == current))
                };
                break index.map(|index| (parent_id, index));
            }
            current = parent_id;
        };
        drop(tree);

        let Some((list_id, index)) = location else {
            return false;
        };
        self.scroll_virtual_list_to_item(list_id, index)
    }

    // One implementation of the scroll and focus calls for both renderers.
    // Callers repaint afterwards.

    /// Set the offset of a virtual list or scroll container.
    pub(crate) fn scroll_to(&self, id: u64, x: f32, y: f32) {
        if self.set_virtual_list_offset(id, x, y) {
            return;
        }
        if let Some(handle) = self.scroll_handles.get(&id) {
            handle.set_offset(gpui::point(gpui::px(x), gpui::px(y)));
        }
    }

    /// Bring child `index` of a virtual list or scroll container into view.
    pub(crate) fn scroll_to_item(&self, id: u64, index: usize) {
        if self.scroll_virtual_list_to_item(id, index) {
            return;
        }
        if let Some(handle) = self.scroll_handles.get(&id) {
            if !scroll_retained_child_into_view(&self.tree, handle, id, index) {
                handle.scroll_to_item(index);
            }
        }
    }

    pub(crate) fn scroll_offset(&self, id: u64) -> Option<[f64; 2]> {
        self.virtual_list_offset(id).or_else(|| {
            let offset = self.scroll_handles.get(&id)?.offset();
            Some([
                f64::from(f32::from(offset.x)),
                f64::from(f32::from(offset.y)),
            ])
        })
    }

    /// Focus `id`, first scrolling a virtual list that has it unmounted.
    pub(crate) fn focus_element(
        &self,
        id: u64,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.reveal_virtual_list_ancestor(id);
        if let Some(handle) = self.focus_handles.get(&id) {
            handle.focus(window, cx);
        }
    }
}

impl SoloView {
    /// Sync focus handles with the current element tree.
    /// Creates handles for new focusable elements, subscribes on_focus/on_blur,
    /// and cleans up handles for destroyed elements.
    fn sync_focus_handles(
        &mut self,
        tree: &RetainedTree,
        callback: &Option<EventCallback>,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let tab_index = |element: &crate::retained_tree::RetainedElement| {
            element
                .custom_props
                .get("tabIndex")
                .and_then(|value| value.as_i64())
                .and_then(|index| isize::try_from(index).ok())
        };
        let has_commands = crate::application_commands::has_commands(cx);
        let root_id = self.root_id(tree);
        // With several windows on one tree, each view owns only its subtree;
        // otherwise an autoFocus in one window would focus into another.
        let members: Option<std::collections::HashSet<u64>> = (!tree.window_roots.is_empty())
            .then(|| root_id.map(|root| tree.subtree_ids(root)).unwrap_or_default().into_iter().collect());
        let is_member = |id: u64| members.as_ref().is_none_or(|members| members.contains(&id));
        let needs_focus = |element: &crate::retained_tree::RetainedElement| {
            is_member(element.id)
                && ((has_commands && root_id == Some(element.id))
                || matches!(element.element_type.as_str(), "input" | "textarea")
                || tab_index(element).is_some()
                || element.events.contains("keyDown")
                || element.events.contains("keyUp")
                || element.events.contains("focus")
                || element.events.contains("blur"))
        };
        // Create handles for elements that need focus but don't have one yet.
        for (&id, element) in &tree.elements {
            let tab_index = tab_index(element).or_else(|| {
                matches!(element.element_type.as_str(), "input" | "textarea").then_some(0)
            });

            if needs_focus(element) && !self.focus_handles.contains_key(&id) {
                let handle = match tab_index {
                    Some(index) => cx.focus_handle().tab_index(index).tab_stop(index >= 0),
                    None => cx.focus_handle(),
                };
                // Focus once, at creation. Re-focusing every frame would
                // steal focus back from whatever the user clicked next.
                if element.auto_focus {
                    handle.focus(window, cx);
                }
                self.focus_handles.insert(id, handle);
            } else if let (Some(handle), Some(index)) =
                (self.focus_handles.get(&id).cloned(), tab_index)
            {
                self.focus_handles
                    .insert(id, handle.tab_index(index).tab_stop(index >= 0));
            } else if let Some(handle) = self.focus_handles.get(&id).cloned() {
                self.focus_handles.insert(id, handle.tab_stop(false));
            }
        }

        self.focus_subscriptions.retain(|(id, event), _| {
            tree.elements
                .get(id)
                .is_some_and(|element| element.events.contains(event))
        });
        for (&id, element) in &tree.elements {
            let Some(handle) = self.focus_handles.get(&id).cloned() else {
                continue;
            };
            let focus_key = (id, "focus".to_string());
            if element.events.contains("focus")
                && !self.focus_subscriptions.contains_key(&focus_key)
            {
                let callback = callback.clone();
                let subscription = cx.on_focus(&handle, window, move |_this, _window, _cx| {
                    emit_event_full(&callback, id, "focus", |_| {});
                });
                self.focus_subscriptions.insert(focus_key, subscription);
            }
            let blur_key = (id, "blur".to_string());
            if element.events.contains("blur") && !self.focus_subscriptions.contains_key(&blur_key)
            {
                let callback = callback.clone();
                let subscription = cx.on_blur(&handle, window, move |_this, _window, _cx| {
                    emit_event_full(&callback, id, "blur", |_| {});
                });
                self.focus_subscriptions.insert(blur_key, subscription);
            }
        }

        // Clean up handles for elements that no longer exist.
        self.focus_handles
            .retain(|id, _| tree.elements.get(id).is_some_and(&needs_focus));
    }
}

impl gpui::Render for SoloView {
    fn render(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        use gpui::IntoElement;

        if self.quit_hook.is_none() {
            self.quit_hook = Some(cx.on_app_quit(|view, _cx| {
                #[cfg(all(not(target_os = "macos"), ui_thread))]
                crate::context_menu_gpui::cancel_all();
                view.system_appearance_subscription.take();
                view.custom_registry.destroy_all();
                // Native child views (WKWebView) outlive the retained tree, so
                // they need their own teardown or AppKit keeps the web process
                // alive past the window.
                crate::native_view::with_registry(|registry| registry.destroy_all());
                async {}
            }));
        }

        // The document marker and the app's title both feed this one, so a
        // `setTitle` while the document is edited keeps the marker.
        let title = self.native_title();
        if self.applied_title.as_ref() != Some(&title) {
            window.set_window_title(&title);
            self.applied_title = Some(title);
        }

        // Clone Arc so we don't borrow self.tree — frees self for focus_handles access.
        let tree_arc = self.tree.clone();
        let tree = tree_arc.lock().unwrap();
        let callback = self.event_callback.clone();

        let has_commands = crate::application_commands::has_commands(cx);
        let root_id = self.root_id(&tree);
        let synced = Some((tree.generation(), has_commands, root_id));
        if self.synced != synced {
            self.synced = synced;
            // Sync focus handles before building elements.
            self.sync_focus_handles(&tree, &callback, window, cx);

            // Ensure custom element instances are destroyed when their IDs disappear.
            self.custom_registry
                .prune_missing(|id| tree.elements.contains_key(&id));

            // Clean up scroll handles for destroyed elements (IDs removed from tree).
            // Scrollability-based cleanup (element still exists but style changed
            // from scroll to non-scroll) is handled inside build_div().
            self.scroll_handles
                .retain(|id, _| tree.elements.contains_key(id));
            self.virtual_lists
                .retain(|id, _| tree.elements.contains_key(id));
            self.motion_states
                .retain(|id, _| tree.elements.contains_key(id));
        }

        // Route global shortcuts through the root capture listener before the
        // first control is focused, without stealing focus from native views.
        if has_commands
            && window.focused(cx).is_none()
            && !crate::application_commands::native_child_has_focus(window)
        {
            if let Some(handle) = root_id.and_then(|id| self.focus_handles.get(&id)) {
                handle.focus(window, cx);
            }
        }

        // Build the element tree. custom_registry, focus_handles, and scroll_handles
        // are different fields of self, so Rust allows borrowing all simultaneously.
        let theme = Theme::dark();
        let now = self.clock.now();
        let mut motion_active = false;
        let result = match self.root_id(&tree) {
            Some(root_id) => {
                let mut ctx = BuildCtx {
                    tree: &tree,
                    event_callback: &callback,
                    focus_handles: &self.focus_handles,
                    scroll_handles: &mut self.scroll_handles,
                    custom_registry: &mut self.custom_registry,
                    virtual_lists: &mut self.virtual_lists,
                    motion_states: &mut self.motion_states,
                    now,
                    motion_active: &mut motion_active,
                    selection: self.selection.clone(),
                    inherited: Inherited::root(&theme),
                    root_id: Some(root_id),
                };
                build_element(root_id, &mut ctx, window, cx)
            }
            None => gpui::Empty.into_any_element(),
        };

        // Linux has no native window tabs, so a window that other windows
        // joined paints the strip itself (see `renderer::tabs`).
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let result = if self.tabs.len() > 1 {
            super::tabs::tabbed(self, result, &theme, cx)
        } else {
            result
        };

        // A client-decorated window has no titlebar of its own, so Solo draws
        // one; server-decorated windows keep their window manager's chrome.
        // Wrapping the tree before the frame-reset wrapper below keeps the
        // titlebar's text after the reset, so `getPaintedText` sees it.
        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        let result = super::decorations::decorate(self, result, &theme, cx.button_layout(), window);

        // The frame reset must paint BEFORE any text, so it is the first child of
        // the root wrapper. Without it the selection registry accumulates stale
        // entries across frames and a drag resolves against elements that are no
        // longer on screen.
        let result = {
            use gpui::prelude::*;
            gpui::div()
                .size_full()
                .capture_key_down(cx.listener(|view, event, window, cx| {
                    // An open context menu owns navigation keys before any
                    // shortcut or focused input sees them.
                    #[cfg(all(not(target_os = "macos"), ui_thread))]
                    if crate::context_menu_gpui::key_down(event) {
                        cx.notify();
                        return;
                    }
                    crate::application_commands::dispatch_shortcut(event, view, window, cx);
                }))
                .on_action(|_: &FocusNext, window, cx| window.focus_next(cx))
                .on_action(|_: &FocusPrevious, window, cx| window.focus_prev(cx))
                .child(selection_frame_reset(self.selection.clone()))
                .child(crate::automation::bounds_frame_reset())
                .child(result)
                .into_any_element()
        };

        // Context menus paint above the tree, in the window's content space.
        #[cfg(all(not(target_os = "macos"), ui_thread))]
        let result = {
            use gpui::prelude::*;
            let viewport = window.viewport_size();
            gpui::div()
                .size_full()
                .child(result)
                .children(crate::context_menu_gpui::overlay(
                    &theme,
                    viewport,
                    self.tab().root,
                    cx,
                ))
                .into_any_element()
        };

        if motion_active {
            window.request_animation_frame();
        }

        result
    }
}

#[cfg(all(test, not(target_os = "macos")))]
mod tests {
    use super::{marked_title, SoloView};
    use crate::retained_tree::RetainedTree;
    use crate::text::selection::SelectionState;
    use std::sync::Arc;

    fn view() -> SoloView {
        SoloView::new(
            Arc::new(std::sync::Mutex::new(RetainedTree::new())),
            None,
            "Main".to_string(),
            Arc::new(parking_lot::Mutex::new(SelectionState::default())),
        )
    }

    #[test]
    fn unsaved_changes_mark_the_native_title() {
        assert_eq!(marked_title("notes.txt", false), "notes.txt");
        assert_eq!(marked_title("notes.txt", true), "*notes.txt");
        // The marker is a prefix, so a title the app retitles keeps it at the
        // front where window managers truncate last.
        assert_eq!(marked_title("", true), "*");
    }

    #[test]
    fn tabs_select_and_close_in_order() {
        let mut view = view();
        assert_eq!(view.tabs.len(), 1);
        assert_eq!(view.tab().root, None);
        assert_eq!(view.native_title(), "Main");

        view.add_tab(7, "Alpha".to_string());
        view.add_tab(9, "Beta".to_string());
        assert_eq!(
            view.tabs.iter().filter_map(|tab| tab.root).collect::<Vec<_>>(),
            vec![7, 9]
        );
        // A join selects the new tab, and the native title follows the tab.
        assert_eq!(view.tab().root, Some(9));
        assert_eq!(view.native_title(), "Beta");

        view.activate_tab(7);
        assert_eq!(view.native_title(), "Alpha");
        // A closed tab leaves its neighbour selected.
        assert!(view.remove_tab(7));
        assert_eq!(view.tab().root, Some(9));
        assert!(!view.remove_tab(7));
        // The window still owns its own root: the last tab never removes it.
        assert!(view.remove_tab(9));
        assert_eq!(view.tab().root, None);
    }

    /// The window's close button closes every tab, so it must ask the guards
    /// of background tabs too, not only the selected one.
    #[test]
    fn a_window_close_asks_every_guarded_tab() {
        let mut view = SoloView::new(
            Arc::new(std::sync::Mutex::new(RetainedTree::new())),
            None,
            "Alpha".to_string(),
            Arc::new(parking_lot::Mutex::new(SelectionState::default())),
        );
        view.add_tab(9, "Beta".to_string());
        assert!(view.allow_close());
        view.tabs[0].close_guarded = true;
        assert!(!view.allow_close());
        view.tabs[0].close_guarded = false;
        view.tabs[1].close_guarded = true;
        // Beta is selected; Alpha guarding still keeps the window open.
        assert!(!view.allow_close());
    }

    /// Confirming the prompt closes the whole window once no guard is left,
    /// even when it also hosts tabs that never asked.
    #[test]
    fn confirming_the_last_guard_completes_the_window_close() {
        let mut view = SoloView::new(
            Arc::new(std::sync::Mutex::new(RetainedTree::new())),
            None,
            "Alpha".to_string(),
            Arc::new(parking_lot::Mutex::new(SelectionState::default())),
        );
        view.add_tab(9, "Alpha".to_string());
        view.add_tab(10, "Beta".to_string());
        view.tabs[1].close_guarded = true;
        assert!(!view.allow_close());
        assert!(view.pending_close());
        // Beta never asked, so its remaining open tab keeps the window open.
        assert!(view.remove_tab(10));
        assert!(!view.pending_close_complete());
        // Alpha confirmed and closed, the only guard is gone.
        assert!(view.remove_tab(9));
        assert!(view.pending_close_complete());
    }
}
