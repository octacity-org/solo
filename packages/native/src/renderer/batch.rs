//! Parses and applies `applyBatch` mutation batches.

use super::*;
use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use std::cell::Cell;

// ── Batch processing ─────────────────────────────────────────────

/// Parsed batch operation — typed enum for atomic validation.
/// All ops are parsed and validated BEFORE any tree mutation occurs.
/// This prevents partial application on malformed batches.
pub(crate) enum BatchOp {
    CreateElement {
        id: u64,
        element_type: String,
    },
    DestroyElement {
        id: u64,
    },
    AppendChild {
        parent_id: u64,
        child_id: u64,
    },
    RemoveChild {
        parent_id: u64,
        child_id: u64,
    },
    InsertBefore {
        parent_id: u64,
        child_id: u64,
        before_id: u64,
    },
    SetStyle {
        id: u64,
        /// Boxed: `StyleDesc` is about 1.4 KB, and every variant would pay it.
        style: Box<StyleDesc>,
    },
    SetText {
        id: u64,
        content: String,
    },
    SetEventListener {
        id: u64,
        event_type: String,
        has_handler: bool,
    },
    SetRoot {
        id: u64,
    },
    SetCustomProp {
        id: u64,
        key: String,
        value: serde_json::Value,
    },
}

/// Parse a batch into typed ops in one pass over the JSON text: each op tuple
/// is decoded field by field as it is read, with no intermediate values. This
/// runs before the tree is locked, so a malformed batch changes nothing.
/// Returns Err on the first invalid op, naming its index.
pub(crate) fn parse_batch(json: &str) -> Result<Vec<BatchOp>> {
    let current_op = Cell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(json);
    deserializer
        .deserialize_seq(BatchVisitor(&current_op))
        .and_then(|ops| deserializer.end().map(|()| ops))
        .map_err(|e| {
            Error::from_reason(match current_op.get() {
                Some(i) => format!("Batch op {}: {}", i, e),
                None => format!("Failed to parse batch: {}", e),
            })
        })
}

/// Reads the batch array, recording which op it is in for error messages.
struct BatchVisitor<'a>(&'a Cell<Option<usize>>);

impl<'de> Visitor<'de> for BatchVisitor<'_> {
    type Value = Vec<BatchOp>;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an array of batch ops")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error> {
        let mut ops = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        loop {
            self.0.set(Some(ops.len()));
            match seq.next_element_seed(OpSeed)? {
                Some(op) => ops.push(op),
                None => break,
            }
        }
        self.0.set(None);
        Ok(ops)
    }
}

/// Decodes one `[opName, ...args]` tuple.
struct OpSeed;

impl<'de> DeserializeSeed<'de> for OpSeed {
    type Value = BatchOp;

    fn deserialize<D: Deserializer<'de>>(self, deserializer: D) -> std::result::Result<BatchOp, D::Error> {
        deserializer.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for OpSeed {
    type Value = BatchOp;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an [opName, ...args] array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<BatchOp, A::Error> {
        let name: OpKind = seq
            .next_element()?
            .ok_or_else(|| de::Error::custom("missing op name string"))?;
        let mut field = 0;
        // Reads the next field, naming its position when it is missing.
        macro_rules! next {
            ($what:literal) => {{
                field += 1;
                seq.next_element()?.ok_or_else(|| {
                    de::Error::custom(format!("missing {} at index {}", $what, field))
                })?
            }};
        }
        macro_rules! id {
            () => {
                to_element_id(next!("id")).map_err(|e| de::Error::custom(&e.reason))?
            };
        }
        let op = match name {
            OpKind::CreateElement => BatchOp::CreateElement {
                id: id!(),
                element_type: next!("string"),
            },
            OpKind::DestroyElement => BatchOp::DestroyElement { id: id!() },
            OpKind::AppendChild => BatchOp::AppendChild {
                parent_id: id!(),
                child_id: id!(),
            },
            OpKind::RemoveChild => BatchOp::RemoveChild {
                parent_id: id!(),
                child_id: id!(),
            },
            OpKind::InsertBefore => BatchOp::InsertBefore {
                parent_id: id!(),
                child_id: id!(),
                before_id: id!(),
            },
            OpKind::SetStyle => {
                let id = id!();
                let Legacy(style) = next!("style");
                BatchOp::SetStyle {
                    id,
                    style: Box::new(style),
                }
            }
            OpKind::SetText => BatchOp::SetText {
                id: id!(),
                content: next!("string"),
            },
            OpKind::SetEventListener => {
                let id = id!();
                let event_type = next!("string");
                let HasHandler(has_handler) = next!("hasHandler");
                BatchOp::SetEventListener {
                    id,
                    event_type,
                    has_handler,
                }
            }
            OpKind::SetRoot => BatchOp::SetRoot { id: id!() },
            OpKind::SetCustomProp => {
                let id = id!();
                let key = next!("string");
                // A legacy string payload that is not JSON stays a string.
                let value = match next!("value") {
                    serde_json::Value::String(encoded) => serde_json::from_str(&encoded)
                        .unwrap_or(serde_json::Value::String(encoded)),
                    value => value,
                };
                BatchOp::SetCustomProp { id, key, value }
            }
            OpKind::SetCustomPropValue => BatchOp::SetCustomProp {
                id: id!(),
                key: next!("string"),
                value: next!("custom prop value"),
            },
            OpKind::Unknown(other) => {
                return Err(de::Error::custom(format!("unknown operation: {:?}", other)));
            }
        };
        // Extra trailing fields are ignored, as they always were.
        while seq.next_element::<de::IgnoredAny>()?.is_some() {}
        Ok(op)
    }
}

/// Op names, matched while reading so the name is never copied.
enum OpKind {
    CreateElement,
    DestroyElement,
    AppendChild,
    RemoveChild,
    InsertBefore,
    SetStyle,
    SetText,
    SetEventListener,
    SetRoot,
    SetCustomProp,
    SetCustomPropValue,
    Unknown(String),
}

impl<'de> Deserialize<'de> for OpKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct OpNameVisitor;

        impl<'de> Visitor<'de> for OpNameVisitor {
            type Value = OpKind;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an op name string")
            }

            fn visit_str<E: de::Error>(self, name: &str) -> std::result::Result<OpKind, E> {
                Ok(match name {
                    "createElement" => OpKind::CreateElement,
                    "destroyElement" => OpKind::DestroyElement,
                    "appendChild" => OpKind::AppendChild,
                    "removeChild" => OpKind::RemoveChild,
                    "insertBefore" => OpKind::InsertBefore,
                    "setStyle" => OpKind::SetStyle,
                    "setText" => OpKind::SetText,
                    "setEventListener" => OpKind::SetEventListener,
                    "setRoot" => OpKind::SetRoot,
                    "setCustomProp" => OpKind::SetCustomProp,
                    "setCustomPropValue" => OpKind::SetCustomPropValue,
                    other => OpKind::Unknown(other.to_owned()),
                })
            }
        }

        deserializer.deserialize_str(OpNameVisitor)
    }
}

/// A payload sent either as JSON or, by older batches, as a JSON-encoded string.
struct Legacy<T>(T);

impl<'de, T: serde::de::DeserializeOwned> Deserialize<'de> for Legacy<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct LegacyVisitor<T>(std::marker::PhantomData<T>);

        impl<'de, T: serde::de::DeserializeOwned> Visitor<'de> for LegacyVisitor<T> {
            type Value = Legacy<T>;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object or a JSON-encoded string")
            }

            fn visit_str<E: de::Error>(self, encoded: &str) -> std::result::Result<Self::Value, E> {
                serde_json::from_str(encoded).map(Legacy).map_err(E::custom)
            }

            fn visit_map<M: MapAccess<'de>>(self, map: M) -> std::result::Result<Self::Value, M::Error> {
                T::deserialize(de::value::MapAccessDeserializer::new(map)).map(Legacy)
            }
        }

        deserializer.deserialize_any(LegacyVisitor(std::marker::PhantomData))
    }
}

/// `setEventListener`'s flag: a bool, or a non-negative integer (non-zero = true).
struct HasHandler(bool);

impl<'de> Deserialize<'de> for HasHandler {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        struct HasHandlerVisitor;

        impl<'de> Visitor<'de> for HasHandlerVisitor {
            type Value = HasHandler;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a boolean or a non-negative integer")
            }

            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<HasHandler, E> {
                Ok(HasHandler(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<HasHandler, E> {
                Ok(HasHandler(v != 0))
            }
        }

        deserializer.deserialize_any(HasHandlerVisitor)
    }
}

/// Apply parsed batch ops to a RetainedTree. Shared between
/// SoloRenderer::apply_batch and TestSoloRenderer::apply_batch. Returns the
/// accumulated destroyed IDs (as f64) from all destroyElement ops.
///
/// ATOMIC: `parse_batch` validated every op, so a malformed batch never
/// reaches the tree and JS and Rust state cannot desync.
///
/// Batch format: JSON array of tuples [opcode, ...args].
/// See SoloRenderer::apply_batch for opcode documentation.
pub(crate) fn apply_batch_to_tree(tree: &mut RetainedTree, ops: Vec<BatchOp>) -> Vec<f64> {
    let mut destroyed_ids: Vec<f64> = Vec::new();
    for batch_op in ops {
        match batch_op {
            BatchOp::CreateElement { id, element_type } => {
                tree.create_element(id, element_type);
            }
            BatchOp::DestroyElement { id } => {
                let destroyed = tree.destroy_element(id);
                destroyed_ids.extend(destroyed.iter().map(|&id| id as f64));
            }
            BatchOp::AppendChild {
                parent_id,
                child_id,
            } => {
                tree.append_child(parent_id, child_id);
            }
            BatchOp::RemoveChild {
                parent_id,
                child_id,
            } => {
                tree.remove_child(parent_id, child_id);
            }
            BatchOp::InsertBefore {
                parent_id,
                child_id,
                before_id,
            } => {
                tree.insert_before(parent_id, child_id, before_id);
            }
            BatchOp::SetStyle { id, style } => {
                tree.set_style(id, *style);
            }
            BatchOp::SetText { id, content } => {
                tree.set_text(id, content);
            }
            BatchOp::SetEventListener {
                id,
                event_type,
                has_handler,
            } => {
                tree.set_event_listener(id, event_type, has_handler);
            }
            BatchOp::SetRoot { id } => {
                tree.set_root(id);
            }
            BatchOp::SetCustomProp { id, key, value } => {
                tree.set_custom_prop(id, key, value);
            }
        }
    }
    destroyed_ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::DimensionValue;

    /// An enum is as large as its largest variant, and `parse_batch` holds one
    /// per op, so a large payload must stay boxed. Today the largest is
    /// `SetCustomProp` (104 bytes): `preserve_order`, enabled by a dependency,
    /// makes `serde_json::Value` 72 bytes.
    #[test]
    fn batch_ops_stay_small() {
        assert!(
            std::mem::size_of::<BatchOp>() <= 128,
            "BatchOp is {} bytes",
            std::mem::size_of::<BatchOp>()
        );
    }

    #[test]
    fn set_style_reaches_the_tree() {
        let ops = parse_batch(
            r#"[["createElement",1,"div"],["setStyle",1,{"display":"flex","width":24}],["setStyle",1,"{\"height\":5}"]]"#,
        )
        .unwrap();
        let mut tree = RetainedTree::new();
        apply_batch_to_tree(&mut tree, ops);
        let style = tree.elements[&1].style.as_ref().unwrap();
        assert_eq!(style.height, Some(DimensionValue::Pixels(5.0)));
        assert_eq!(style.display, None);
    }

    fn apply(json: &str) -> (RetainedTree, Vec<f64>) {
        let mut tree = RetainedTree::new();
        let destroyed = apply_batch_to_tree(&mut tree, parse_batch(json).unwrap());
        (tree, destroyed)
    }

    fn parse_error(json: &str) -> String {
        match parse_batch(json) {
            Ok(_) => panic!("{json} parsed"),
            Err(e) => e.reason.clone(),
        }
    }

    #[test]
    fn parses_every_op() {
        let (tree, destroyed) = apply(
            r#"[
                ["createElement",1,"div"],["createElement",2,"div"],
                ["createElement",3,"text"],["createElement",4,"div"],
                ["createElement",5,"div"],
                ["setRoot",1],
                ["appendChild",1,2],["insertBefore",1,3,2],
                ["appendChild",2,5],["removeChild",2,5],
                ["setText",3,"hello \"world\""],
                ["setStyle",2,{"display":"flex","width":24}],
                ["setEventListener",2,"click",true],
                ["setEventListener",2,"keyDown",1],
                ["setEventListener",2,"mouseUp",0],
                ["setCustomProp",2,"object",{"a":[1,2]}],
                ["setCustomProp",2,"legacyJson","{\"b\":true}"],
                ["setCustomProp",2,"legacyText","plain"],
                ["setCustomPropValue",2,"value","{\"c\":1}"],
                ["destroyElement",4]
            ]"#,
        );
        assert_eq!(tree.root_id, Some(1));
        assert_eq!(tree.elements[&1].children, vec![3, 2]);
        assert!(tree.elements[&2].children.is_empty());
        assert_eq!(tree.elements[&3].content.as_deref(), Some("hello \"world\""));
        let element = &tree.elements[&2];
        let style = element.style.as_ref().unwrap();
        assert_eq!(style.display.as_deref(), Some("flex"));
        assert_eq!(style.width, Some(DimensionValue::Pixels(24.0)));
        let mut events: Vec<_> = element.events.iter().cloned().collect();
        events.sort();
        assert_eq!(events, ["click", "keyDown"]);
        let props = &element.custom_props;
        assert_eq!(props["object"], serde_json::json!({"a": [1, 2]}));
        assert_eq!(props["legacyJson"], serde_json::json!({"b": true}));
        assert_eq!(props["legacyText"], serde_json::json!("plain"));
        // setCustomPropValue never decodes a string.
        assert_eq!(props["value"], serde_json::json!("{\"c\":1}"));
        assert_eq!(destroyed, vec![4.0]);
        assert!(!tree.elements.contains_key(&4));
    }

    #[test]
    fn ignores_extra_fields() {
        let (tree, _) = apply(r#"[["createElement",1,"div","extra"],["setRoot",1,{}]]"#);
        assert_eq!(tree.root_id, Some(1));
    }

    #[test]
    fn rejects_malformed_batches() {
        // Each error names what failed and, once the op is known, its index.
        for (json, expected) in [
            ("not json", &["Failed to parse batch"][..]),
            ("{}", &["Failed to parse batch"]),
            (r#"[["setRoot",1]] x"#, &["Failed to parse batch", "trailing characters"]),
            (r#"[["setRoot",1]"#, &["EOF while parsing"]),
            ("[42]", &["Batch op 0"]),
            ("[[]]", &["Batch op 0", "missing op name string"]),
            (r#"[["setRoot",1],["nope",1]]"#, &["Batch op 1", r#"unknown operation: "nope""#]),
            (r#"[["appendChild",1]]"#, &["Batch op 0", "missing id at index 2"]),
            (r#"[["createElement",1]]"#, &["Batch op 0", "missing string at index 2"]),
            (r#"[["createElement",-1,"div"]]"#, &["Invalid element id: -1"]),
            (r#"[["createElement",1.5,"div"]]"#, &["Invalid element id: 1.5"]),
            (r#"[["setStyle",1,{"width":"wide"}]]"#, &["Batch op 0"]),
            (r#"[["setStyle",1,"not json"]]"#, &["Batch op 0"]),
            (r#"[["setEventListener",1,"click","yes"]]"#, &["Batch op 0"]),
            (r#"[["setEventListener",1,"click"]]"#, &["Batch op 0", "hasHandler at index 3"]),
            (r#"[["setCustomPropValue",1,"k"]]"#, &["Batch op 0", "missing custom prop value"]),
        ] {
            let error = parse_error(json);
            for part in expected {
                assert!(error.contains(part), "{json}: {error:?} lacks {part:?}");
            }
        }
    }

    /// One bad op rejects the whole batch, so nothing reaches the tree.
    #[test]
    fn rejects_a_batch_atomically() {
        parse_error(r#"[["createElement",1,"div"],["setRoot",1],["appendChild",1]]"#);
    }
}
