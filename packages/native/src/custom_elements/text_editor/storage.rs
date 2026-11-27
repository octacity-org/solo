//! An editor's text and the edits applied to it.
//!
//! Offsets are UTF-8 byte offsets on char boundaries. JS sees UTF-16 offsets;
//! convert at the boundary with `offset_to_utf16` / `offset_from_utf16`.

use std::ops::Range;

use super::revision::Change;

/// A replacement of `range` with `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub range: Range<usize>,
    pub text: String,
}

impl Edit {
    pub fn new(range: Range<usize>, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
        }
    }

    pub fn change(&self) -> Change {
        Change {
            start: self.range.start,
            old_len: self.range.len(),
            new_len: self.text.len(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct TextStorage {
    text: String,
}

impl TextStorage {
    pub fn new(text: String) -> Self {
        Self { text }
    }

    pub fn as_str(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Apply `edit` and return the edit that undoes it.
    pub fn apply(&mut self, edit: &Edit) -> Edit {
        let removed = self.text[edit.range.clone()].to_string();
        self.text.replace_range(edit.range.clone(), &edit.text);
        Edit::new(edit.range.start..edit.range.start + edit.text.len(), removed)
    }

    /// The largest char boundary at or below `offset`, within the text.
    pub fn clamp(&self, offset: usize) -> usize {
        let mut offset = offset.min(self.text.len());
        while !self.text.is_char_boundary(offset) {
            offset -= 1;
        }
        offset
    }

    pub fn utf16_len(&self) -> usize {
        self.text.encode_utf16().count()
    }

    pub fn offset_to_utf16(&self, offset: usize) -> usize {
        let offset = self.clamp(offset);
        self.text[..offset].encode_utf16().count()
    }

    /// `edit`, not yet applied, in UTF-16 code units.
    pub fn utf16_change(&self, edit: &Edit) -> Change {
        Change {
            start: self.offset_to_utf16(edit.range.start),
            old_len: self.text[edit.range.clone()].encode_utf16().count(),
            new_len: edit.text.encode_utf16().count(),
        }
    }

    /// The byte offset of a UTF-16 offset. One inside a surrogate pair, or
    /// past the end, lands on the next char boundary.
    pub fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for character in self.text.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += character.len_utf16();
            utf8_offset += character.len_utf8();
        }
        utf8_offset
    }

    /// The one edit that turns this text into `text`: everything between the
    /// longest common prefix and suffix. `None` when they are equal.
    pub fn diff(&self, text: &str) -> Option<Edit> {
        let old = self.text.as_bytes();
        let new = text.as_bytes();
        if old == new {
            return None;
        }
        let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        while !self.text.is_char_boundary(prefix) {
            prefix -= 1;
        }
        let limit = old.len().min(new.len()) - prefix;
        let mut suffix = old
            .iter()
            .rev()
            .zip(new.iter().rev())
            .take(limit)
            .take_while(|(a, b)| a == b)
            .count();
        while !self.text.is_char_boundary(old.len() - suffix) {
            suffix -= 1;
        }
        Some(Edit::new(
            prefix..old.len() - suffix,
            &text[prefix..new.len() - suffix],
        ))
    }

    /// The byte range of the line holding `offset`, without its newline.
    pub fn line_range_at(&self, offset: usize) -> Range<usize> {
        let start = self.text[..offset]
            .rfind('\n')
            .map(|index| index + 1)
            .unwrap_or(0);
        let end = self.text[offset..]
            .find('\n')
            .map(|index| offset + index)
            .unwrap_or(self.text.len());
        start..end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_returns_the_inverse_edit() {
        let mut storage = TextStorage::new("select 1".into());
        let inverse = storage.apply(&Edit::new(7..8, "42"));
        assert_eq!(storage.as_str(), "select 42");
        assert_eq!(inverse, Edit::new(7..9, "1"));
        storage.apply(&inverse);
        assert_eq!(storage.as_str(), "select 1");
    }

    #[test]
    fn utf16_offsets_count_surrogate_pairs() {
        let storage = TextStorage::new("é🙂x".into());
        assert_eq!(storage.utf16_len(), 4);
        assert_eq!(storage.offset_to_utf16("é🙂".len()), 3);
        assert_eq!(storage.offset_from_utf16(3), "é🙂".len());
        assert_eq!(storage.offset_from_utf16(2), "é🙂".len(), "inside a pair");
        assert_eq!(storage.offset_from_utf16(99), storage.len());
    }

    #[test]
    fn utf16_change_counts_code_units() {
        let storage = TextStorage::new("a🙂b".into());
        let edit = Edit::new(1..5, "é");
        assert_eq!(
            storage.utf16_change(&edit),
            Change {
                start: 1,
                old_len: 2,
                new_len: 1
            }
        );
    }

    #[test]
    fn diff_replaces_only_what_changed() {
        let diff = |old: &str, new: &str| TextStorage::new(old.into()).diff(new);
        assert_eq!(diff("select 1", "select 1"), None);
        assert_eq!(diff("select 1", "select 12"), Some(Edit::new(8..8, "2")));
        assert_eq!(diff("select 1", "elect 1"), Some(Edit::new(0..1, "")));
        assert_eq!(diff("aaa", "aa"), Some(Edit::new(2..3, "")));
        assert_eq!(diff("abc", "xyz"), Some(Edit::new(0..3, "xyz")));
        // Shared bytes inside one char do not split it.
        assert_eq!(diff("é", "è"), Some(Edit::new(0..2, "è")));
        assert_eq!(diff("a🙂", "a😀"), Some(Edit::new(1..5, "😀")));
        assert_eq!(diff("🙂b", "😀b"), Some(Edit::new(0..4, "😀")));
        for (old, new) in [("x🙂y", "x😀y"), ("", "abc"), ("abc", ""), ("abab", "ab")] {
            let mut storage = TextStorage::new(old.into());
            let edit = storage.diff(new).unwrap();
            storage.apply(&edit);
            assert_eq!(storage.as_str(), new);
        }
    }

    #[test]
    fn clamp_lands_on_a_char_boundary() {
        let storage = TextStorage::new("é".into());
        assert_eq!(storage.clamp(1), 0);
        assert_eq!(storage.clamp(9), 2);
    }

    #[test]
    fn line_range_excludes_the_newline() {
        let storage = TextStorage::new("ab\ncd\n".into());
        assert_eq!(storage.line_range_at(1), 0..2);
        assert_eq!(storage.line_range_at(3), 3..5);
        assert_eq!(storage.line_range_at(6), 6..6);
    }
}
