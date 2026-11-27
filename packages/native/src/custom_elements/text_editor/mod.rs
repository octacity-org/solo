//! Editing model shared by `<input>` and `<textarea>`: text storage,
//! revisions and undo. Nothing here depends on GPUI.

pub mod revision;
pub mod storage;
pub mod undo;

use std::ops::Range;

use revision::{rebase_offset, Bias, Change};

/// A selection by byte offsets. `head` is where the caret is; it equals
/// `anchor` when nothing is selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn caret(offset: usize) -> Self {
        Self {
            anchor: offset,
            head: offset,
        }
    }

    pub fn from_range(range: Range<usize>, reversed: bool) -> Self {
        if reversed {
            Self {
                anchor: range.end,
                head: range.start,
            }
        } else {
            Self {
                anchor: range.start,
                head: range.end,
            }
        }
    }

    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }

    pub fn reversed(&self) -> bool {
        self.head < self.anchor
    }

    /// Move the selection through a change made around it.
    pub fn rebase(self, change: Change) -> Self {
        Self {
            anchor: rebase_offset(self.anchor, change, Bias::After),
            head: rebase_offset(self.head, change, Bias::After),
        }
    }
}
