//! `<virtual-list>`: GPUI's list over retained rows or data-driven items.

use super::*;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct VirtualListConfig {
    alignment: gpui::ListAlignment,
    follow_tail: bool,
    overdraw: f32,
    estimated_item_height: Option<f32>,
}

impl VirtualListConfig {
    fn from_element(element: &crate::retained_tree::RetainedElement) -> Self {
        let prop = |key: &str| element.custom_props.get(key);
        let alignment = match prop("alignment").and_then(serde_json::Value::as_str) {
            Some("bottom") => gpui::ListAlignment::Bottom,
            _ => gpui::ListAlignment::Top,
        };
        let follow_tail = prop("followTail")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let overdraw = prop("overdraw")
            .and_then(serde_json::Value::as_f64)
            .unwrap_or(512.0)
            .max(0.0) as f32;
        // A data-driven list paints placeholders at the estimate, so it always
        // has one.
        let estimated_item_height = prop("estimatedItemHeight")
            .and_then(serde_json::Value::as_f64)
            .filter(|height| *height > 0.0)
            .map(|height| height as f32)
            .or_else(|| prop("itemCount").map(|_| DEFAULT_DATA_ROW_HEIGHT));
        Self {
            alignment,
            follow_tail,
            overdraw,
            estimated_item_height,
        }
    }

    fn make_state(self, focus_handles: &[Option<gpui::FocusHandle>]) -> gpui::ListState {
        let item_count = focus_handles.len();
        let mut state = gpui::ListState::new(item_count, self.alignment, gpui::px(self.overdraw));
        state.splice_focusable(0..item_count, focus_handles.iter().cloned());
        if let Some(height) = self.estimated_item_height {
            state = state.with_uniform_item_height(gpui::px(height));
        }
        if self.follow_tail {
            state.set_follow_mode(gpui::FollowMode::Tail);
        }
        state
    }
}

/// Placeholder height of a data-driven list given no `estimatedItemHeight`.
pub(super) const DEFAULT_DATA_ROW_HEIGHT: f32 = 32.0;

pub(super) struct VirtualListEntry {
    pub(super) state: gpui::ListState,
    config: VirtualListConfig,
    /// The list element's `subtree_revision` at the last sync. Rows, their
    /// revisions and focusability only change with it, so an unchanged list
    /// skips rebuilding them each frame.
    synced_revision: u64,
    /// Shared with the frame's row renderer.
    child_ids: Rc<[u64]>,
    child_revisions: Vec<u64>,
    row_focus_handles: Vec<Option<gpui::FocusHandle>>,
    pub(super) seen_rows: HashSet<u64>,
    /// Set for a data-driven list (`itemCount`), whose rows are not its
    /// children in order; the three vectors above stay empty then.
    pub(super) data: Option<DataRows>,
}

/// A data-driven list has `count` items, but only the rows JavaScript mounted
/// exist, each carrying its `itemIndex`. The others paint as placeholders
/// until a `rangeChange` event gets them mounted.
pub(super) struct DataRows {
    count: usize,
    rows: std::collections::BTreeMap<usize, DataRow>,
    /// `rows` as index → element ID, shared with the frame's row renderer.
    row_ids: Rc<HashMap<usize, u64>>,
    /// The last range sent, so a frame that changes nothing sends nothing.
    reported: Rc<std::cell::Cell<Option<RowRange>>>,
}

pub(super) struct DataRow {
    id: u64,
    revision: u64,
    focus_handle: Option<gpui::FocusHandle>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct RowRange {
    start: usize,
    end: usize,
    focused: Option<usize>,
}

impl VirtualListEntry {
    fn new(
        config: VirtualListConfig,
        child_ids: Vec<u64>,
        child_revisions: Vec<u64>,
        row_focus_handles: Vec<Option<gpui::FocusHandle>>,
    ) -> Self {
        Self {
            state: config.make_state(&row_focus_handles),
            config,
            synced_revision: 0,
            child_ids: child_ids.into(),
            child_revisions,
            row_focus_handles,
            seen_rows: HashSet::new(),
            data: None,
        }
    }

    fn new_data(
        config: VirtualListConfig,
        count: usize,
        rows: std::collections::BTreeMap<usize, DataRow>,
    ) -> Self {
        let mut focus_handles = vec![None; count];
        for (&index, row) in &rows {
            focus_handles[index] = row.focus_handle.clone();
        }
        Self {
            state: config.make_state(&focus_handles),
            config,
            synced_revision: 0,
            child_ids: Rc::new([]),
            child_revisions: Vec::new(),
            row_focus_handles: Vec::new(),
            seen_rows: HashSet::new(),
            data: Some(DataRows {
                count,
                row_ids: row_ids(&rows),
                rows,
                reported: Rc::default(),
            }),
        }
    }

    /// Swap in a list rebuilt from scratch, keeping the scroll position unless
    /// the new list starts following its tail.
    fn replace(&mut self, mut replacement: Self) {
        let should_follow = replacement.config.follow_tail
            && (!self.config.follow_tail || self.state.is_following_tail());
        let seen_rows = std::mem::take(&mut self.seen_rows);
        replacement.seen_rows = seen_rows
            .into_iter()
            .filter(|id| replacement.row_index(*id).is_some())
            .collect();
        if !should_follow {
            replacement.state.scroll_to(self.state.logical_scroll_top());
        }
        *self = replacement;
    }

    /// Show the first rows of a top-aligned list that just went from empty to
    /// having rows. An empty list's scroll top, item 0, is also its end, and
    /// GPUI's splice keeps a scroll top at the end there, past the new rows.
    fn open_at_top(&self) {
        if self.config.alignment == gpui::ListAlignment::Top && !self.state.is_following_tail() {
            self.state.scroll_to(gpui::ListOffset::default());
        }
    }

    fn row_index(&self, row_id: u64) -> Option<usize> {
        match &self.data {
            Some(data) => data
                .rows
                .iter()
                .find_map(|(&index, row)| (row.id == row_id).then_some(index)),
            None => self.child_ids.iter().position(|id| *id == row_id),
        }
    }

    pub(super) fn row_focus_handle(&self, index: usize, row_id: u64) -> Option<gpui::FocusHandle> {
        match &self.data {
            Some(data) => data
                .rows
                .get(&index)
                .filter(|row| row.id == row_id)?
                .focus_handle
                .clone(),
            None => (self.child_ids.get(index) == Some(&row_id))
                .then(|| self.row_focus_handles.get(index).cloned())
                .flatten()
                .flatten(),
        }
    }

    /// The focus handle a data row already has, wherever it now sits.
    fn data_focus_handle(&self, row_id: u64) -> Option<gpui::FocusHandle> {
        self.data
            .as_ref()?
            .rows
            .values()
            .find(|row| row.id == row_id)?
            .focus_handle
            .clone()
    }

    fn sync_data(
        &mut self,
        config: VirtualListConfig,
        count: usize,
        rows: std::collections::BTreeMap<usize, DataRow>,
    ) {
        let Some(data) = self.data.as_mut().filter(|_| self.config == config) else {
            self.replace(Self::new_data(config, count, rows));
            return;
        };
        let was_empty = data.count == 0 && count > 0;
        if data.count != count {
            let kept = data.count.min(count);
            self.state.splice(kept..data.count, count - kept);
            data.count = count;
            // Spliced rows start without a size hint.
            if let Some(height) = config.estimated_item_height {
                let _ = self.state.clone().with_uniform_item_height(gpui::px(height));
            }
        }
        for (&index, old) in &data.rows {
            if index < count && old.focus_handle.is_some() && !rows.contains_key(&index) {
                self.state.splice_focusable(index..index + 1, [None]);
            }
        }
        for (&index, row) in &rows {
            let old = data.rows.get(&index);
            if old.and_then(|old| old.focus_handle.as_ref()) != row.focus_handle.as_ref() {
                self.state
                    .splice_focusable(index..index + 1, [row.focus_handle.clone()]);
            } else if old.is_none_or(|old| old.id != row.id || old.revision != row.revision) {
                // A mounted row replaces a placeholder measured at the estimate.
                self.state.remeasure_items(index..index + 1);
            }
        }
        self.seen_rows
            .retain(|id| rows.values().any(|row| row.id == *id));
        data.row_ids = row_ids(&rows);
        data.rows = rows;
        if was_empty {
            self.open_at_top();
        }
    }

    fn sync(
        &mut self,
        config: VirtualListConfig,
        child_ids: Vec<u64>,
        child_revisions: Vec<u64>,
        focusable_rows: &HashSet<u64>,
        cx: &mut gpui::Context<SoloView>,
    ) {
        let old_rows: HashMap<u64, (u64, Option<gpui::FocusHandle>)> = self
            .child_ids
            .iter()
            .copied()
            .zip(self.child_revisions.iter().copied())
            .zip(self.row_focus_handles.iter().cloned())
            .map(|((id, revision), focus_handle)| (id, (revision, focus_handle)))
            .collect();
        let row_focus_handles: Vec<Option<gpui::FocusHandle>> = child_ids
            .iter()
            .map(|id| {
                focusable_rows.contains(id).then(|| {
                    old_rows
                        .get(id)
                        .and_then(|(_, focus_handle)| focus_handle.clone())
                        .unwrap_or_else(|| cx.focus_handle())
                })
            })
            .collect();
        if self.config != config || self.data.is_some() {
            self.replace(Self::new(config, child_ids, child_revisions, row_focus_handles));
            return;
        }

        let was_empty = self.child_ids.is_empty();
        let mut spliced = *self.child_ids != *child_ids;
        if spliced {
            let prefix = self
                .child_ids
                .iter()
                .zip(&child_ids)
                .take_while(|(old, new)| old == new)
                .count();
            let suffix = self.child_ids[prefix..]
                .iter()
                .rev()
                .zip(child_ids[prefix..].iter().rev())
                .take_while(|(old, new)| old == new)
                .count();
            self.state.splice_focusable(
                prefix..self.child_ids.len().saturating_sub(suffix),
                row_focus_handles[prefix..row_focus_handles.len().saturating_sub(suffix)]
                    .iter()
                    .cloned(),
            );
        }

        for (index, (&id, focus_handle)) in child_ids.iter().zip(&row_focus_handles).enumerate() {
            let focusability_changed = old_rows
                .get(&id)
                .is_some_and(|(_, old_handle)| old_handle.is_some() != focus_handle.is_some());
            if focusability_changed {
                self.state
                    .splice_focusable(index..index + 1, std::iter::once(focus_handle.clone()));
                spliced = true;
            }
        }
        // Spliced rows start without a size hint.
        if spliced {
            if let Some(height) = config.estimated_item_height {
                self.state = self
                    .state
                    .clone()
                    .with_uniform_item_height(gpui::px(height));
            }
            if was_empty {
                self.open_at_top();
            }
        }

        let mut changed_start = None;
        for (index, (&id, &revision)) in child_ids.iter().zip(&child_revisions).enumerate() {
            let changed = old_rows
                .get(&id)
                .is_some_and(|(old_revision, _)| *old_revision != revision);
            match (changed_start, changed) {
                (None, true) => changed_start = Some(index),
                (Some(start), false) => {
                    self.state.remeasure_items(start..index);
                    changed_start = None;
                }
                _ => {}
            }
        }
        if let Some(start) = changed_start {
            self.state.remeasure_items(start..child_ids.len());
        }

        self.child_ids = child_ids.into();
        self.child_revisions = child_revisions;
        self.row_focus_handles = row_focus_handles;
    }
}

pub(super) fn row_ids(
    rows: &std::collections::BTreeMap<usize, DataRow>,
) -> Rc<HashMap<usize, u64>> {
    Rc::new(rows.iter().map(|(&index, row)| (index, row.id)).collect())
}

/// Rebuild a virtual list's rows from the retained tree.
pub(super) fn sync_virtual_list<'e>(
    element: &crate::retained_tree::RetainedElement,
    ctx: &'e mut BuildCtx,
    cx: &mut gpui::Context<SoloView>,
) -> &'e mut VirtualListEntry {
    let child_ids: Vec<u64> = element
        .children
        .iter()
        .copied()
        .filter(|child_id| ctx.tree.elements.contains_key(child_id))
        .collect();
    let child_revisions: Vec<u64> = child_ids
        .iter()
        .filter_map(|child_id| {
            ctx.tree
                .elements
                .get(child_id)
                .map(|child| child.subtree_revision)
        })
        .collect();
    let focusable_rows: HashSet<u64> = ctx
        .focus_handles
        .keys()
        .filter_map(|element_id| virtual_row_ancestor(ctx.tree, element.id, *element_id))
        .collect();
    let config = VirtualListConfig::from_element(element);
    let item_count = element
        .custom_props
        .get("itemCount")
        .and_then(serde_json::Value::as_u64)
        .and_then(|count| usize::try_from(count).ok());
    match item_count {
        Some(count) => {
            let previous = ctx.virtual_lists.get(&element.id);
            let rows: std::collections::BTreeMap<usize, DataRow> = child_ids
                .iter()
                .filter_map(|&id| {
                    let child = ctx.tree.elements.get(&id)?;
                    let index = child.custom_props.get("itemIndex")?.as_u64()?;
                    let index = usize::try_from(index).ok().filter(|index| *index < count)?;
                    let focus_handle = focusable_rows.contains(&id).then(|| {
                        previous
                            .and_then(|entry| entry.data_focus_handle(id))
                            .unwrap_or_else(|| cx.focus_handle())
                    });
                    Some((
                        index,
                        DataRow {
                            id,
                            revision: child.subtree_revision,
                            focus_handle,
                        },
                    ))
                })
                .collect();
            match ctx.virtual_lists.entry(element.id) {
                std::collections::hash_map::Entry::Occupied(entry) => {
                    let entry = entry.into_mut();
                    entry.sync_data(config, count, rows);
                    entry
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(VirtualListEntry::new_data(config, count, rows))
                }
            }
        }
        None => match ctx.virtual_lists.entry(element.id) {
            std::collections::hash_map::Entry::Occupied(entry) => {
                let entry = entry.into_mut();
                entry.sync(
                    config,
                    child_ids.clone(),
                    child_revisions,
                    &focusable_rows,
                    cx,
                );
                entry
            }
            std::collections::hash_map::Entry::Vacant(entry) => {
                let row_focus_handles = child_ids
                    .iter()
                    .map(|id| focusable_rows.contains(id).then(|| cx.focus_handle()))
                    .collect();
                entry.insert(VirtualListEntry::new(
                    config,
                    child_ids.clone(),
                    child_revisions,
                    row_focus_handles,
                ))
            }
        },
    }
}

pub(super) fn build_virtual_list(
    element: &crate::retained_tree::RetainedElement,
    ctx: &mut BuildCtx,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<SoloView>,
) -> gpui::AnyElement {
    use gpui::prelude::*;

    let focused_row = ctx
        .focus_handles
        .iter()
        .find_map(|(element_id, handle)| {
            handle
                .is_focused(window)
                .then(|| virtual_row_ancestor(ctx.tree, element.id, *element_id))
                .flatten()
        })
        .or_else(|| {
            ctx.focus_handles.keys().find_map(|element_id| {
                ctx.tree
                    .elements
                    .get(element_id)
                    .is_some_and(|element| element.auto_focus)
                    .then(|| virtual_row_ancestor(ctx.tree, element.id, *element_id))
                    .flatten()
            })
        });
    let inherited = ctx.inherited;
    let event_callback = ctx.event_callback.clone();
    let entry = match ctx.virtual_lists.entry(element.id) {
        std::collections::hash_map::Entry::Occupied(entry)
            if entry.get().synced_revision == element.subtree_revision =>
        {
            entry.into_mut()
        }
        _ => sync_virtual_list(element, ctx, cx),
    };
    entry.synced_revision = element.subtree_revision;
    let config = entry.config;
    let focused_index = focused_row.and_then(|row_id| entry.row_index(row_id));
    if let Some(index) = focused_row
        .filter(|row_id| !entry.seen_rows.contains(row_id))
        .and(focused_index)
    {
        entry.state.scroll_to(gpui::ListOffset {
            item_ix: index,
            offset_in_item: gpui::px(0.0),
        });
    }
    let list_state = entry.state.clone();
    let estimated_height = config.estimated_item_height.map(gpui::px);

    let list_id = element.id;
    let (render_item, range): (Box<dyn Fn(usize, &mut gpui::Window, &mut gpui::App) -> gpui::AnyElement>, _) =
        match &entry.data {
            Some(data) => {
                let rows = data.row_ids.clone();
                let range = element.events.contains("rangeChange").then(|| RangeReport {
                    list_id,
                    callback: event_callback,
                    count: data.count,
                    margin: (config.overdraw
                        / config.estimated_item_height.unwrap_or(DEFAULT_DATA_ROW_HEIGHT))
                    .ceil() as usize,
                    focused: focused_index,
                    painted: Rc::default(),
                    reported: data.reported.clone(),
                });
                let painted = range.as_ref().map(|range| range.painted.clone());
                let placeholder_height =
                    estimated_height.unwrap_or(gpui::px(DEFAULT_DATA_ROW_HEIGHT));
                let render = cx.processor(move |view, index: usize, window, cx| {
                    let row = match rows.get(&index) {
                        Some(&child_id) => {
                            view.build_virtual_child(list_id, index, child_id, inherited, window, cx)
                        }
                        None => gpui::div().w_full().h(placeholder_height).into_any_element(),
                    };
                    let Some(painted) = painted.clone() else {
                        return row;
                    };
                    // Only rows the list places get prepainted, so this sees
                    // the rows in view and a focused row kept offscreen.
                    gpui::div()
                        .w_full()
                        .child(row)
                        .on_children_prepainted(move |bounds, _, _| {
                            if let Some(bounds) = bounds.first() {
                                painted.borrow_mut().push((index, *bounds));
                            }
                        })
                        .into_any_element()
                });
                (Box::new(render), range)
            }
            None => {
                let child_ids = entry.child_ids.clone();
                let render = cx.processor(move |view, index: usize, window, cx| {
                    let Some(&child_id) = child_ids.get(index) else {
                        return gpui::Empty.into_any_element();
                    };
                    view.build_virtual_child(list_id, index, child_id, inherited, window, cx)
                });
                (Box::new(render), None)
            }
        };
    let mut list = gpui::list(list_state.clone(), render_item)
        .with_sizing_behavior(gpui::ListSizingBehavior::Auto);
    if let Some(style) = element.style.as_ref() {
        list = apply_styles(list, style);
    }
    if estimated_height.is_none() && range.is_none() {
        return list.into_any_element();
    }
    VirtualListElement {
        list,
        state: list_state,
        estimated_height,
        range,
    }
    .into_any_element()
}

/// Wraps GPUI's list for two jobs it cannot do itself.
///
/// GPUI's list drops every row's size hint when its width changes, including
/// its first layout, so unmeasured rows count as zero height and pixel scrolls
/// and the scrollbar stop short. The wrapper re-applies the estimate after
/// that reset.
///
/// A data-driven list also reports which items it needs after each layout.
pub(super) struct VirtualListElement {
    list: gpui::List,
    state: gpui::ListState,
    estimated_height: Option<gpui::Pixels>,
    range: Option<RangeReport>,
}

/// Sends a data-driven list's `rangeChange`: the rows in view plus `margin`
/// rows on each side for overdraw, and the focused row.
pub(super) struct RangeReport {
    list_id: u64,
    callback: Option<EventCallback>,
    count: usize,
    margin: usize,
    focused: Option<usize>,
    /// Rows prepainted this frame, with their bounds.
    painted: Rc<RefCell<Vec<(usize, gpui::Bounds<gpui::Pixels>)>>>,
    reported: Rc<std::cell::Cell<Option<RowRange>>>,
}

impl RangeReport {
    fn report(&self, viewport: gpui::Bounds<gpui::Pixels>) {
        let mut visible = self
            .painted
            .borrow()
            .iter()
            .filter(|(_, bounds)| bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom())
            .map(|(index, _)| *index)
            .collect::<Vec<_>>();
        visible.sort_unstable();
        let (start, end) = match (visible.first(), visible.last()) {
            (Some(&first), Some(&last)) => (
                first.saturating_sub(self.margin),
                (last + 1 + self.margin).min(self.count),
            ),
            _ => (0, 0),
        };
        let range = RowRange {
            start,
            end,
            focused: self.focused,
        };
        if self.reported.replace(Some(range)) == Some(range) {
            return;
        }
        emit_event_full(&self.callback, self.list_id, "rangeChange", |payload| {
            payload.range_start = Some(start as f64);
            payload.range_end = Some(end as f64);
            payload.focused_index = range.focused.map(|index| index as f64);
        });
    }
}

impl gpui::Element for VirtualListElement {
    type RequestLayoutState = ();
    type PrepaintState = <gpui::List as gpui::Element>::PrepaintState;

    fn id(&self) -> Option<gpui::ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> (gpui::LayoutId, ()) {
        self.list.request_layout(id, inspector_id, window, cx)
    }

    fn prepaint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut (),
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Self::PrepaintState {
        let previous_width = self.state.viewport_bounds().size.width;
        if let Some(range) = &self.range {
            range.painted.borrow_mut().clear();
        }
        let prepaint = self
            .list
            .prepaint(id, inspector_id, bounds, request_layout, window, cx);
        if let Some(height) = self.estimated_height.filter(|_| bounds.size.width != previous_width) {
            // Fills only rows without a size, so measured heights are kept.
            let _ = self.state.clone().with_uniform_item_height(height);
        }
        if let Some(range) = &self.range {
            range.report(bounds);
        }
        prepaint
    }

    fn paint(
        &mut self,
        id: Option<&gpui::GlobalElementId>,
        inspector_id: Option<&gpui::InspectorElementId>,
        bounds: gpui::Bounds<gpui::Pixels>,
        request_layout: &mut (),
        prepaint: &mut Self::PrepaintState,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) {
        self.list
            .paint(id, inspector_id, bounds, request_layout, prepaint, window, cx);
    }
}

impl gpui::IntoElement for VirtualListElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

pub(super) fn virtual_row_ancestor(tree: &RetainedTree, list_id: u64, element_id: u64) -> Option<u64> {
    let mut current = element_id;
    loop {
        let parent = tree.elements.get(&current)?.parent?;
        if parent == list_id {
            return Some(current);
        }
        current = parent;
    }
}
