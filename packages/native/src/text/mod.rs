//! Selectable text: the shared foundation for every text Solo paints.
//!
//! Ported from Comet (https://github.com/zeronsh/comet), MIT.
//!
//! Every text node in Solo goes through [`paint::selectable_text`], so a drag
//! can start in a plain `<text>` and end inside a `<code>` block. That only
//! works because all of them register into the same per-frame registry in paint
//! order. Adding a new text-painting element means calling that helper, never
//! `div().child(string)`.

pub mod paint;
pub mod runs;
pub mod selection;

pub use paint::{
    chrome_text, forget_window, log_painted_text, range_rects, selectable_text, selection_frame_reset,
    selection_key, SelectableText, SharedSelection,
};
#[cfg(feature = "automation")]
pub use paint::painted_text;
