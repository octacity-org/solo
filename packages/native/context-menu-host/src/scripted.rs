//! Keyboard navigation for a menu that automation drives without opening it.
//!
//! An open AppKit menu is a system-wide modal: any click or key from the
//! person at the machine can dismiss or steer it. A scripted menu is never
//! opened, so its keys are resolved here. The rules follow AppKit's own
//! behavior, observed by sending the same keys to an open NSMenu.

use crate::protocol::MenuItem;

#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    Continue,
    /// Choose the item at this index of the request's items.
    Select(usize),
    Cancel,
}

#[derive(Default)]
pub struct Navigator {
    highlighted: Option<usize>,
    typed: String,
}

impl Navigator {
    pub fn key(&mut self, items: &[MenuItem], key: &str) -> Step {
        let enabled = |index: &usize| {
            matches!(
                items[*index],
                MenuItem::Action {
                    disabled: false,
                    ..
                }
            )
        };
        let mut chars = key.chars();
        if let (Some(character), None) = (chars.next(), chars.next()) {
            // Type-select: consecutive characters extend one prefix, as in
            // AppKit. Unlike AppKit, a prefix nothing matches keeps the
            // current highlight instead of jumping to a nearby item.
            self.typed.extend(character.to_lowercase());
            if let Some(index) = (0..items.len()).filter(enabled).find(|index| {
                matches!(&items[*index], MenuItem::Action { label, .. }
                    if label.to_lowercase().starts_with(&self.typed))
            }) {
                self.highlighted = Some(index);
            }
            return Step::Continue;
        }
        self.typed.clear();
        match key {
            // AppKit stops at the ends instead of wrapping around.
            "down" => {
                let start = self.highlighted.map_or(0, |index| index + 1);
                if let Some(index) = (start..items.len()).find(enabled) {
                    self.highlighted = Some(index);
                }
            }
            "up" => {
                let end = self.highlighted.unwrap_or(items.len());
                if let Some(index) = (0..end).rev().find(enabled) {
                    self.highlighted = Some(index);
                }
            }
            "enter" => return self.highlighted.map_or(Step::Cancel, Step::Select),
            "escape" => return Step::Cancel,
            _ => {}
        }
        Step::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_request;

    /// Items from labels: a leading `-` disables one, `---` is a separator.
    fn menu(labels: &[&str]) -> Vec<MenuItem> {
        let items: Vec<String> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| match *label {
                "---" => r#"{"type":"separator"}"#.into(),
                label => match label.strip_prefix('-') {
                    Some(label) => {
                        format!(r#"{{"id":"{index}","label":"{label}","disabled":true}}"#)
                    }
                    None => format!(r#"{{"id":"{index}","label":"{label}"}}"#),
                },
            })
            .collect();
        parse_request(&format!(r#"{{"x":0,"y":0,"items":[{}]}}"#, items.join(",")))
            .unwrap()
            .items
    }

    /// The label the key sequence chooses, or None for a cancelled menu.
    fn choose(labels: &[&str], keys: &str) -> Option<String> {
        let items = menu(labels);
        let mut navigator = Navigator::default();
        for key in keys.split(' ') {
            match navigator.key(&items, key) {
                Step::Continue => {}
                Step::Cancel => return None,
                Step::Select(index) => match &items[index] {
                    MenuItem::Action { label, .. } => return Some(label.clone()),
                    MenuItem::Separator { .. } => panic!("selected a separator"),
                },
            }
        }
        panic!("keys left the menu open: {keys}")
    }

    #[test]
    fn arrows_skip_disabled_items_and_separators() {
        assert_eq!(
            choose(&["-Unavailable", "---", "Read"], "down enter").as_deref(),
            Some("Read")
        );
        assert_eq!(
            choose(&["Apple", "-Banana", "Cherry"], "down down enter").as_deref(),
            Some("Cherry")
        );
        assert_eq!(
            choose(&["Apple", "-Banana"], "up enter").as_deref(),
            Some("Apple")
        );
    }

    #[test]
    fn up_from_nothing_starts_at_the_bottom() {
        assert_eq!(
            choose(&["Apple", "Banana"], "up enter").as_deref(),
            Some("Banana")
        );
        assert_eq!(
            choose(&["Apple", "Banana"], "down up enter").as_deref(),
            Some("Apple")
        );
    }

    #[test]
    fn arrows_stop_at_the_ends() {
        assert_eq!(
            choose(&["Apple", "Banana"], "down down down enter").as_deref(),
            Some("Banana")
        );
        assert_eq!(
            choose(&["Apple", "Banana"], "down up up enter").as_deref(),
            Some("Apple")
        );
    }

    #[test]
    fn enter_without_a_highlight_and_escape_cancel() {
        assert_eq!(choose(&["Apple"], "enter"), None);
        assert_eq!(choose(&["-Apple"], "down enter"), None);
        assert_eq!(choose(&["Apple"], "down escape"), None);
    }

    #[test]
    fn modifiers_and_unknown_keys_do_not_move_the_highlight() {
        // Keys arrive by name; shift-down reaches the helper as "down".
        assert_eq!(
            choose(&["Apple", "Banana"], "down tab space enter").as_deref(),
            Some("Apple")
        );
    }

    #[test]
    fn typing_selects_the_first_enabled_prefix_match() {
        assert_eq!(
            choose(&["Apple", "Banana"], "b enter").as_deref(),
            Some("Banana")
        );
        assert_eq!(
            choose(&["Apple", "Banana"], "B enter").as_deref(),
            Some("Banana")
        );
        assert_eq!(
            choose(&["Apple", "-Banana", "Blue"], "b enter").as_deref(),
            Some("Blue")
        );
        assert_eq!(
            choose(&["Bad", "Ban", "Bar"], "b a r enter").as_deref(),
            Some("Bar")
        );
        assert_eq!(
            choose(&["Apple", "Avocado"], "down down a enter").as_deref(),
            Some("Apple")
        );
    }

    #[test]
    fn an_unmatched_prefix_keeps_the_highlight() {
        assert_eq!(
            choose(&["Bad", "Ban", "Bar"], "b b enter").as_deref(),
            Some("Bad")
        );
        assert_eq!(choose(&["Apple", "Cherry"], "z enter"), None);
        // A navigation key starts a new prefix.
        assert_eq!(
            choose(&["Apple", "Banana"], "b down a enter").as_deref(),
            Some("Apple")
        );
    }
}
