//! Parses and applies `applyBatch` mutation batches.

use super::*;

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

/// One batch tuple, its fields still raw JSON.
pub(super) type RawOp<'a> = Vec<&'a serde_json::value::RawValue>;

/// Parse a batch into typed ops. Fields deserialize straight from their
/// slice of the JSON text, so nothing is parsed twice or cloned, and this runs
/// before the tree is locked. Returns Err on the first invalid op.
pub(crate) fn parse_batch(json: &str) -> Result<Vec<BatchOp>> {
    let ops: Vec<&serde_json::value::RawValue> = serde_json::from_str(json)
        .map_err(|e| Error::from_reason(format!("Failed to parse batch: {}", e)))?;
    let mut parsed = Vec::with_capacity(ops.len());

    for (i, op) in ops.iter().enumerate() {
        let arr: RawOp = serde_json::from_str(op.get())
            .map_err(|_| Error::from_reason(format!("Batch op {} is not an array", i)))?;
        let op_name = arr
            .first()
            .and_then(|v| serde_json::from_str::<&str>(v.get()).ok())
            .ok_or_else(|| Error::from_reason(format!("Batch op {} missing op name string", i)))?;

        let batch_op = match op_name {
            "createElement" => BatchOp::CreateElement {
                id: batch_id(&arr, 1, i)?,
                element_type: batch_str(&arr, 2, i)?,
            },
            "destroyElement" => BatchOp::DestroyElement {
                id: batch_id(&arr, 1, i)?,
            },
            "appendChild" => BatchOp::AppendChild {
                parent_id: batch_id(&arr, 1, i)?,
                child_id: batch_id(&arr, 2, i)?,
            },
            "removeChild" => BatchOp::RemoveChild {
                parent_id: batch_id(&arr, 1, i)?,
                child_id: batch_id(&arr, 2, i)?,
            },
            "insertBefore" => BatchOp::InsertBefore {
                parent_id: batch_id(&arr, 1, i)?,
                child_id: batch_id(&arr, 2, i)?,
                before_id: batch_id(&arr, 3, i)?,
            },
            "setStyle" => {
                let style: Box<StyleDesc> = batch_decode(&arr, 2, i).map_err(|e| {
                    Error::from_reason(format!("Batch op {} setStyle parse error: {}", i, e))
                })?;
                BatchOp::SetStyle {
                    id: batch_id(&arr, 1, i)?,
                    style,
                }
            }
            "setText" => BatchOp::SetText {
                id: batch_id(&arr, 1, i)?,
                content: batch_str(&arr, 2, i)?,
            },
            "setEventListener" => {
                let has_handler = arr
                    .get(3)
                    .and_then(|v| {
                        serde_json::from_str::<bool>(v.get())
                            .ok()
                            .or_else(|| serde_json::from_str::<u64>(v.get()).ok().map(|n| n != 0))
                    })
                    .ok_or_else(|| {
                        Error::from_reason(format!(
                            "Batch op {} setEventListener missing/invalid hasHandler at index 3",
                            i
                        ))
                    })?;
                BatchOp::SetEventListener {
                    id: batch_id(&arr, 1, i)?,
                    event_type: batch_str(&arr, 2, i)?,
                    has_handler,
                }
            }
            "setRoot" => BatchOp::SetRoot {
                id: batch_id(&arr, 1, i)?,
            },
            "setCustomProp" => BatchOp::SetCustomProp {
                id: batch_id(&arr, 1, i)?,
                key: batch_str(&arr, 2, i)?,
                value: batch_payload(&arr, 3, i)?,
            },
            "setCustomPropValue" => BatchOp::SetCustomProp {
                id: batch_id(&arr, 1, i)?,
                key: batch_str(&arr, 2, i)?,
                value: arr
                    .get(3)
                    .and_then(|v| serde_json::from_str(v.get()).ok())
                    .ok_or_else(|| {
                        Error::from_reason(format!("Batch op {} missing custom prop value", i))
                    })?,
            },
            _ => {
                return Err(Error::from_reason(format!(
                    "Batch op {} unknown operation: {:?}",
                    i, op_name
                )));
            }
        };
        parsed.push(batch_op);
    }

    Ok(parsed)
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

/// Extract a u64 element ID from a batch tuple at the given index.
pub(super) fn batch_id(arr: &RawOp, idx: usize, op_idx: usize) -> Result<u64> {
    let v = arr
        .get(idx)
        .and_then(|v| serde_json::from_str::<f64>(v.get()).ok())
        .ok_or_else(|| {
            Error::from_reason(format!("Batch op {} missing id at index {}", op_idx, idx))
        })?;
    to_element_id(v)
}

pub(super) fn batch_field<'a>(
    arr: &RawOp<'a>,
    idx: usize,
    op_idx: usize,
) -> Result<&'a serde_json::value::RawValue> {
    arr.get(idx).copied().ok_or_else(|| {
        Error::from_reason(format!(
            "Batch op {} missing value at index {}",
            op_idx, idx
        ))
    })
}

/// A legacy batch sends a style or custom-prop payload as a JSON string.
pub(super) fn legacy_payload(raw: &serde_json::value::RawValue) -> Option<String> {
    raw.get()
        .starts_with('"')
        .then(|| serde_json::from_str(raw.get()).ok())
        .flatten()
}

/// A custom-prop payload. A legacy string that is not JSON stays a string.
pub(super) fn batch_payload(arr: &RawOp, idx: usize, op_idx: usize) -> Result<serde_json::Value> {
    let raw = batch_field(arr, idx, op_idx)?;
    if let Some(encoded) = legacy_payload(raw) {
        return Ok(serde_json::from_str(&encoded).unwrap_or(serde_json::Value::String(encoded)));
    }
    serde_json::from_str(raw.get()).map_err(|e| Error::from_reason(e.to_string()))
}

pub(super) fn batch_decode<T: serde::de::DeserializeOwned>(
    arr: &RawOp,
    idx: usize,
    op_idx: usize,
) -> Result<T> {
    let raw = batch_field(arr, idx, op_idx)?;
    let decoded = match legacy_payload(raw) {
        Some(encoded) => serde_json::from_str(&encoded),
        None => serde_json::from_str(raw.get()),
    };
    decoded.map_err(|e| Error::from_reason(e.to_string()))
}

/// Extract a String from a batch tuple at the given index.
pub(super) fn batch_str(arr: &RawOp, idx: usize, op_idx: usize) -> Result<String> {
    arr.get(idx)
        .and_then(|v| serde_json::from_str::<String>(v.get()).ok())
        .ok_or_else(|| {
            Error::from_reason(format!(
                "Batch op {} missing string at index {}",
                op_idx, idx
            ))
        })
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
}
