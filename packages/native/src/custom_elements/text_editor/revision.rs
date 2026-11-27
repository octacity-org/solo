//! Revisions and the log that moves stale offsets forward.
//!
//! Every edit to an editor's text bumps its revision and appends the lengths
//! it changed. Text is never stored. A caller holding an offset from an older
//! revision rebases it through the entries that came after; when the log has
//! been trimmed past that revision, the caller gets `Expired`.
//!
//! Nothing here depends on the offset unit. The editor logs UTF-16 code units,
//! the unit JS callers hold offsets in; undo rebases byte offsets.

use std::collections::VecDeque;
use std::ops::Range;

/// Entries kept before the log is trimmed, and how many survive a trim.
pub const LOG_LIMIT: usize = 512;
pub const LOG_KEEP: usize = 256;

/// One replacement of `old_len` units at `start` with `new_len` units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Change {
    pub start: usize,
    pub old_len: usize,
    pub new_len: usize,
}

impl Change {
    pub fn end(&self) -> usize {
        self.start + self.old_len
    }
}

/// Which side of an insertion an offset lands on when both share a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bias {
    Before,
    After,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebaseError {
    /// A newer change touched the inside of the range.
    Conflict,
    /// The log no longer reaches back to the requested revision.
    Expired,
}

/// Move an offset on the text before `change` to the text after it. An offset
/// inside the replaced bytes snaps to the start of the change.
pub fn rebase_offset(offset: usize, change: Change, bias: Bias) -> usize {
    let end = change.end();
    if offset < change.start {
        offset
    } else if offset == change.start && change.old_len == 0 {
        match bias {
            Bias::Before => offset,
            Bias::After => offset + change.new_len,
        }
    } else if offset == change.start {
        offset
    } else if offset < end {
        change.start
    } else {
        offset - change.old_len + change.new_len
    }
}

/// Move a range on the text before `change` to the text after it, or fail when
/// `change` replaced or inserted bytes inside it. When both are insertions at
/// the same point, `bias` says whether the range lands after `change`'s text.
pub fn rebase_range(
    range: Range<usize>,
    change: Change,
    bias: Bias,
) -> Result<Range<usize>, RebaseError> {
    let end = change.end();
    if change.start < range.end && end > range.start {
        return Err(RebaseError::Conflict);
    }
    let tie = range.is_empty() && change.old_len == 0 && change.start == range.start;
    let shifts = if tie { bias == Bias::After } else { end <= range.start };
    if shifts {
        let shift = |offset: usize| offset - change.old_len + change.new_len;
        Ok(shift(range.start)..shift(range.end))
    } else {
        Ok(range)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LogEntry {
    revision: u64,
    /// Applied in order; each is on the text left by the previous one.
    changes: Vec<Change>,
}

/// The current revision plus the changes that produced recent revisions.
#[derive(Debug, Clone)]
pub struct RevisionLog {
    revision: u64,
    /// Every revision above this one is in `entries`.
    floor: u64,
    entries: VecDeque<LogEntry>,
}

impl Default for RevisionLog {
    fn default() -> Self {
        Self::new()
    }
}

impl RevisionLog {
    pub fn new() -> Self {
        Self {
            revision: 0,
            floor: 0,
            entries: VecDeque::new(),
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Record one revision made of `changes`, applied in order. Returns it.
    pub fn push(&mut self, changes: Vec<Change>) -> u64 {
        self.revision += 1;
        self.entries.push_back(LogEntry {
            revision: self.revision,
            changes,
        });
        if self.entries.len() > LOG_LIMIT {
            while self.entries.len() > LOG_KEEP {
                let dropped = self.entries.pop_front().expect("log is not empty");
                self.floor = dropped.revision;
            }
        }
        self.revision
    }

    fn changes_since(&self, revision: u64) -> Result<impl Iterator<Item = Change> + '_, RebaseError> {
        if revision < self.floor || revision > self.revision {
            return Err(RebaseError::Expired);
        }
        Ok(self
            .entries
            .iter()
            .filter(move |entry| entry.revision > revision)
            .flat_map(|entry| entry.changes.iter().copied()))
    }

    /// Move a range from `revision` to the current revision.
    pub fn rebase_range(
        &self,
        mut range: Range<usize>,
        revision: u64,
    ) -> Result<Range<usize>, RebaseError> {
        for change in self.changes_since(revision)? {
            range = rebase_range(range, change, Bias::After)?;
        }
        Ok(range)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(start: usize, old_len: usize, new_len: usize) -> Change {
        Change {
            start,
            old_len,
            new_len,
        }
    }

    #[test]
    fn offsets_move_with_changes_before_them() {
        // "hello world": replace "hello" with "hi".
        let replace = change(0, 5, 2);
        assert_eq!(rebase_offset(6, replace, Bias::After), 3);
        assert_eq!(rebase_offset(5, replace, Bias::After), 2);
        assert_eq!(rebase_offset(0, replace, Bias::After), 0);
        assert_eq!(rebase_offset(3, replace, Bias::After), 0, "inside snaps to start");
    }

    #[test]
    fn insertion_point_follows_bias() {
        let insert = change(4, 0, 3);
        assert_eq!(rebase_offset(4, insert, Bias::Before), 4);
        assert_eq!(rebase_offset(4, insert, Bias::After), 7);
        assert_eq!(rebase_offset(3, insert, Bias::After), 3);
    }

    #[test]
    fn ranges_conflict_only_when_a_change_reaches_inside() {
        let range = 4..8;
        assert_eq!(rebase_range(range.clone(), change(0, 2, 5), Bias::After), Ok(7..11));
        assert_eq!(rebase_range(range.clone(), change(2, 2, 0), Bias::After), Ok(2..6));
        assert_eq!(rebase_range(range.clone(), change(8, 3, 1), Bias::After), Ok(4..8));
        assert_eq!(rebase_range(range.clone(), change(4, 0, 2), Bias::After), Ok(6..10));
        assert_eq!(
            rebase_range(range.clone(), change(5, 0, 1), Bias::After),
            Err(RebaseError::Conflict)
        );
        assert_eq!(
            rebase_range(range.clone(), change(7, 2, 0), Bias::After),
            Err(RebaseError::Conflict)
        );
        assert_eq!(
            rebase_range(5..5, change(4, 2, 0), Bias::After),
            Err(RebaseError::Conflict),
            "a deletion swallowing an insertion point conflicts"
        );
    }

    #[test]
    fn insertions_at_the_same_point_follow_bias() {
        assert_eq!(rebase_range(4..4, change(4, 0, 2), Bias::After), Ok(6..6));
        assert_eq!(rebase_range(4..4, change(4, 0, 2), Bias::Before), Ok(4..4));
    }

    #[test]
    fn log_rebases_stale_ranges_through_later_revisions() {
        let mut log = RevisionLog::new();
        assert_eq!(log.push(vec![change(0, 0, 3)]), 1);
        assert_eq!(log.push(vec![change(10, 0, 1), change(0, 1, 0)]), 2);
        assert_eq!(log.rebase_range(2..4, 0), Ok(4..6));
        assert_eq!(log.rebase_range(2..4, 1), Ok(1..3));
        assert_eq!(log.rebase_range(2..4, 2), Ok(2..4));
        assert_eq!(log.rebase_range(2..4, 3), Err(RebaseError::Expired));
    }

    #[test]
    fn trimming_expires_old_revisions_only() {
        let mut log = RevisionLog::new();
        for _ in 0..LOG_LIMIT {
            log.push(vec![change(0, 0, 1)]);
        }
        assert_eq!(log.rebase_range(0..0, 0), Ok(LOG_LIMIT..LOG_LIMIT));
        log.push(vec![change(0, 0, 1)]);
        let revision = log.revision();
        let floor = revision - LOG_KEEP as u64;
        assert_eq!(log.rebase_range(0..0, floor - 1), Err(RebaseError::Expired));
        assert_eq!(log.rebase_range(0..0, floor), Ok(LOG_KEEP..LOG_KEEP));
        assert_eq!(log.rebase_range(0..0, revision), Ok(0..0));
    }
}
