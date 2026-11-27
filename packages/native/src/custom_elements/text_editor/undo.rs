//! Undo history built from inverse edits.
//!
//! A transaction stores the edits that undo it, not a copy of the text.
//! Consecutive typing joins one transaction until a pause, a newline, a
//! selection move or a different kind of edit. An edit that bypasses the
//! history (an external `value`, or an API edit marked not undoable) is moved
//! through every stored transaction; one it collides with is dropped together
//! with everything older, because those can no longer be replayed exactly.

use std::time::{Duration, Instant};

use super::revision::{rebase_range, Bias, Change};
use super::storage::Edit;
use super::Selection;

/// Typing pauses longer than this start a new transaction.
pub const GROUP_PAUSE: Duration = Duration::from_millis(500);
/// Transactions kept on the undo stack.
pub const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditKind {
    /// Typed text without a newline. Joins earlier `Insert`s.
    Insert,
    /// Backspace or delete. Joins earlier `Delete`s.
    Delete,
    /// IME composition. Joins the open transaction whatever its age.
    Compose,
    /// Everything else: newline, paste, cut, API edits. Never joins.
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    /// Applied in order to undo the transaction; each edit is on the text
    /// left by the previous one.
    pub inverse: Vec<Edit>,
    /// Selection before the transaction, restored by undo.
    pub before: Selection,
    /// Selection after the transaction, restored by redo.
    pub after: Selection,
    kind: EditKind,
}

impl Transaction {
    pub fn new(inverse: Vec<Edit>, before: Selection, after: Selection) -> Self {
        Self {
            inverse,
            before,
            after,
            kind: EditKind::Other,
        }
    }

    /// The same transaction undone by other edits, e.g. after undo replays
    /// it and the result must become its redo.
    pub fn with_inverse(self, inverse: Vec<Edit>) -> Self {
        Self { inverse, ..self }
    }
}

#[derive(Debug, Default)]
pub struct UndoHistory {
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    /// When the top transaction last grew, while it still accepts more edits.
    open_since: Option<Instant>,
}

impl UndoHistory {
    /// Record one applied edit by the edit that undoes it.
    pub fn record(
        &mut self,
        inverse: Edit,
        before: Selection,
        after: Selection,
        kind: EditKind,
        now: Instant,
    ) {
        self.redo.clear();
        let joins = match (self.open_since, self.undo.last()) {
            (Some(at), Some(top)) => match kind {
                EditKind::Compose => true,
                EditKind::Other => false,
                _ => top.kind == kind && now.duration_since(at) < GROUP_PAUSE,
            },
            _ => false,
        };
        if joins {
            let top = self.undo.last_mut().expect("checked above");
            top.inverse.insert(0, inverse);
            top.after = after;
            if top.kind == EditKind::Compose {
                top.kind = kind;
            }
        } else {
            self.push_undo(Transaction {
                inverse: vec![inverse],
                before,
                after,
                kind,
            });
        }
        self.open_since = (kind != EditKind::Other).then_some(now);
    }

    /// Record a finished transaction that never joins another.
    pub fn record_transaction(&mut self, transaction: Transaction) {
        self.redo.clear();
        self.push_undo(transaction);
        self.open_since = None;
    }

    fn push_undo(&mut self, transaction: Transaction) {
        self.undo.push(transaction);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
    }

    /// Stop the top transaction from growing, e.g. after the caret moved.
    pub fn break_group(&mut self) {
        self.open_since = None;
    }

    pub fn pop_undo(&mut self) -> Option<Transaction> {
        self.open_since = None;
        self.undo.pop()
    }

    pub fn pop_redo(&mut self) -> Option<Transaction> {
        self.open_since = None;
        self.redo.pop()
    }

    /// Store an undone transaction, as the edits that redo it.
    pub fn push_redo(&mut self, transaction: Transaction) {
        self.redo.push(transaction);
    }

    /// Store a redone transaction without clearing the redo stack.
    pub fn push_redone(&mut self, transaction: Transaction) {
        self.push_undo(transaction);
    }

    #[cfg(test)]
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Move the history through a change that is not itself undoable.
    pub fn rebase(&mut self, change: Change) {
        self.redo.clear();
        self.open_since = None;
        let mut change = change;
        for index in (0..self.undo.len()).rev() {
            match rebase_transaction(&self.undo[index], change) {
                Some((transaction, older)) => {
                    self.undo[index] = transaction;
                    change = older;
                }
                None => {
                    self.undo.drain(..=index);
                    return;
                }
            }
        }
    }
}

/// `change` is on the text after `transaction`. Returns the transaction moved
/// past `change`, and `change` moved to the text before the transaction.
fn rebase_transaction(transaction: &Transaction, change: Change) -> Option<(Transaction, Change)> {
    // `after` lives on the text after the transaction, where `change` is.
    let after = transaction.after.rebase(change);
    let mut change = change;
    let mut inverse = Vec::with_capacity(transaction.inverse.len());
    for edit in &transaction.inverse {
        let range = rebase_range(edit.range.clone(), change, Bias::After).ok()?;
        let moved = rebase_range(change.start..change.end(), edit.change(), Bias::Before).ok()?;
        inverse.push(Edit::new(range, edit.text.clone()));
        change = Change {
            start: moved.start,
            old_len: moved.len(),
            new_len: change.new_len,
        };
    }
    Some((
        Transaction {
            inverse,
            before: transaction.before.rebase(change),
            after,
            kind: transaction.kind,
        },
        change,
    ))
}

#[cfg(test)]
mod tests {
    use super::super::storage::TextStorage;
    use super::*;

    fn caret(offset: usize) -> Selection {
        Selection::caret(offset)
    }

    /// Apply `edit`, recording it like the editor does.
    fn type_edit(
        storage: &mut TextStorage,
        history: &mut UndoHistory,
        edit: Edit,
        kind: EditKind,
        now: Instant,
    ) {
        let before = caret(edit.range.start);
        let after = caret(edit.range.start + edit.text.len());
        let inverse = storage.apply(&edit);
        history.record(inverse, before, after, kind, now);
    }

    fn undo(storage: &mut TextStorage, history: &mut UndoHistory) -> Selection {
        let transaction = history.pop_undo().expect("something to undo");
        let mut redo: Vec<Edit> = transaction
            .inverse
            .iter()
            .map(|edit| storage.apply(edit))
            .collect();
        redo.reverse();
        let selection = transaction.before;
        history.push_redo(transaction.with_inverse(redo));
        selection
    }

    fn redo(storage: &mut TextStorage, history: &mut UndoHistory) -> Selection {
        let transaction = history.pop_redo().expect("something to redo");
        let mut undo: Vec<Edit> = transaction
            .inverse
            .iter()
            .map(|edit| storage.apply(edit))
            .collect();
        undo.reverse();
        let selection = transaction.after;
        history.push_redone(transaction.with_inverse(undo));
        selection
    }

    #[test]
    fn typing_joins_until_a_pause() {
        let start = Instant::now();
        let mut storage = TextStorage::default();
        let mut history = UndoHistory::default();
        type_edit(&mut storage, &mut history, Edit::new(0..0, "a"), EditKind::Insert, start);
        type_edit(&mut storage, &mut history, Edit::new(1..1, "b"), EditKind::Insert, start);
        let later = start + GROUP_PAUSE;
        type_edit(&mut storage, &mut history, Edit::new(2..2, "c"), EditKind::Insert, later);
        assert_eq!(history.undo_len(), 2);
        assert_eq!(undo(&mut storage, &mut history), caret(2));
        assert_eq!(storage.as_str(), "ab");
        assert_eq!(undo(&mut storage, &mut history), caret(0));
        assert_eq!(storage.as_str(), "");
        assert_eq!(redo(&mut storage, &mut history), caret(2));
        assert_eq!(redo(&mut storage, &mut history), caret(3));
        assert_eq!(storage.as_str(), "abc");
    }

    #[test]
    fn newlines_kind_changes_and_caret_moves_start_new_transactions() {
        let now = Instant::now();
        let mut storage = TextStorage::default();
        let mut history = UndoHistory::default();
        type_edit(&mut storage, &mut history, Edit::new(0..0, "ab"), EditKind::Insert, now);
        type_edit(&mut storage, &mut history, Edit::new(2..2, "\n"), EditKind::Other, now);
        type_edit(&mut storage, &mut history, Edit::new(3..3, "c"), EditKind::Insert, now);
        type_edit(&mut storage, &mut history, Edit::new(3..4, ""), EditKind::Delete, now);
        history.break_group();
        type_edit(&mut storage, &mut history, Edit::new(1..2, ""), EditKind::Delete, now);
        assert_eq!(history.undo_len(), 5);
    }

    #[test]
    fn composition_joins_the_open_transaction() {
        let start = Instant::now();
        let mut storage = TextStorage::default();
        let mut history = UndoHistory::default();
        type_edit(&mut storage, &mut history, Edit::new(0..0, "n"), EditKind::Compose, start);
        let late = start + GROUP_PAUSE * 4;
        type_edit(&mut storage, &mut history, Edit::new(0..1, "ñ"), EditKind::Compose, late);
        assert_eq!(history.undo_len(), 1);
        undo(&mut storage, &mut history);
        assert_eq!(storage.as_str(), "");
    }

    #[test]
    fn history_survives_a_change_that_is_not_undoable() {
        let start = Instant::now();
        let mut storage = TextStorage::default();
        let mut history = UndoHistory::default();
        type_edit(&mut storage, &mut history, Edit::new(0..0, "ab"), EditKind::Insert, start);
        let later = start + GROUP_PAUSE;
        type_edit(&mut storage, &mut history, Edit::new(1..2, "c"), EditKind::Other, later);
        assert_eq!(storage.as_str(), "ac");

        // An external edit in front of everything.
        let external = Edit::new(0..0, "> ");
        storage.apply(&external);
        history.rebase(external.change());

        undo(&mut storage, &mut history);
        assert_eq!(storage.as_str(), "> ab");
        assert_eq!(undo(&mut storage, &mut history), caret(2));
        assert_eq!(storage.as_str(), "> ");
    }

    #[test]
    fn a_colliding_change_drops_that_transaction_and_older_ones() {
        let start = Instant::now();
        let mut storage = TextStorage::default();
        let mut history = UndoHistory::default();
        type_edit(&mut storage, &mut history, Edit::new(0..0, "one"), EditKind::Insert, start);
        let later = start + GROUP_PAUSE;
        type_edit(&mut storage, &mut history, Edit::new(3..3, " two"), EditKind::Other, later);

        // Rewrites "one" from outside the history.
        let external = Edit::new(0..3, "1");
        storage.apply(&external);
        history.rebase(external.change());
        assert_eq!(storage.as_str(), "1 two");
        assert_eq!(history.undo_len(), 1);

        undo(&mut storage, &mut history);
        assert_eq!(storage.as_str(), "1");
        assert!(history.pop_undo().is_none());
    }

    #[test]
    fn rebase_keeps_replaying_exact_after_random_external_edits() {
        // Deterministic LCG so failures reproduce.
        let mut seed: u64 = 0x5eed;
        let mut next = |bound: usize| {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as usize) % bound.max(1)
        };
        let mut levels_checked = 0;
        for _ in 0..200 {
            let start = Instant::now();
            let mut storage = TextStorage::new("abcdefghij".into());
            let mut history = UndoHistory::default();
            let mut snapshots = vec![storage.as_str().to_string()];
            for step in 0..4 {
                let a = next(storage.len() + 1);
                let b = (a + next(3)).min(storage.len());
                let edit = Edit::new(a..b, ["x", "yy", ""][next(3)]);
                let now = start + GROUP_PAUSE * (step + 1);
                type_edit(&mut storage, &mut history, edit, EditKind::Other, now);
                snapshots.push(storage.as_str().to_string());
            }
            // An external insertion; replaying undo must reach each snapshot
            // with the same insertion, for as long as the history survives.
            let at = next(storage.len() + 1);
            let external = Edit::new(at..at, "Z");
            storage.apply(&external);
            history.rebase(external.change());
            let depth = history.undo_len();
            for level in 0..depth {
                undo(&mut storage, &mut history);
                let expected = &snapshots[snapshots.len() - 2 - level];
                assert_eq!(storage.as_str().replacen('Z', "", 1), *expected);
                assert_eq!(storage.as_str().matches('Z').count(), 1);
                levels_checked += 1;
            }
        }
        assert!(levels_checked > 200, "only {levels_checked} undo levels survived");
    }
}
