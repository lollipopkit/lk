use super::*;

/// Built-in methods on `Bytes`.
///
/// `Bytes` was the one sequence in the language with no methods at all: `b[0]`
/// did not index, `for x in b` did not iterate, and anything sequence-shaped
/// went through `bytes.to_list(b)` — a copy that also inflates each byte into
/// an eight-byte `RuntimeVal::Int`. So the only way to read bytes was to stop
/// having bytes.
///
/// What is here is the **read** half of the list surface: the operations whose
/// meaning does not depend on the element type, and which therefore mean the
/// same thing on a `Bytes` as on a `List`. The transforming half is not, and
/// deliberately: `b.map(|x| x + 1000)` cannot answer a `Bytes`, because 1000 is
/// not a byte. Those live on `List`, reachable through `to_list`.
pub(super) fn dispatch_bytes_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let Some(HeapValue::Bytes(bytes)) = heap.get(*handle) else {
        return Ok(None);
    };
    let bytes = bytes.clone();

    match method {
        "len" => {
            if !positional.is_empty() {
                bail!("bytes.len() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Int(bytes.len() as i64)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("bytes.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(bytes.is_empty())))
        }
        // Same index rule as everywhere else: a negative counts from the end,
        // and outside is nil rather than a raise.
        "get" => {
            if positional.len() != 1 {
                bail!("bytes.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(index) = &positional[0] else {
                bail!("bytes.get() index must be Int");
            };
            Ok(Some(byte_at(&bytes, *index)))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!("bytes.contains() expects 1 argument (value), got {}", positional.len());
            }
            let RuntimeVal::Int(value) = &positional[0] else {
                bail!("bytes.contains() value must be Int");
            };
            let found = u8::try_from(*value).is_ok_and(|byte| bytes.contains(&byte));
            Ok(Some(RuntimeVal::Bool(found)))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!("bytes.index_of() expects 1 argument (value), got {}", positional.len());
            }
            let RuntimeVal::Int(value) = &positional[0] else {
                bail!("bytes.index_of() value must be Int");
            };
            let found = u8::try_from(*value)
                .ok()
                .and_then(|byte| bytes.iter().position(|candidate| *candidate == byte));
            Ok(Some(RuntimeVal::Int(found.map_or(-1, |index| index as i64))))
        }
        "first" => {
            if !positional.is_empty() {
                bail!("bytes.first() expects no arguments, got {}", positional.len());
            }
            Ok(Some(byte_at(&bytes, 0)))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("bytes.last() expects no arguments, got {}", positional.len());
            }
            Ok(Some(byte_at(&bytes, -1)))
        }
        // A window over bytes is `Bytes` again, not a `Slice`: the element type
        // is what makes this a distinct type, and a window over it has the same
        // elements. (`Slice` windows a `List` without copying; this copies,
        // because `Arc<[u8]>` has no cheap sub-range.)
        "slice" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "bytes.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let start = byte_index_arg(&positional[0], "bytes.slice() start")?;
            let end = match positional.get(1) {
                Some(RuntimeVal::Nil) | None => bytes.len(),
                Some(value) => byte_index_arg(value, "bytes.slice() end")?,
            };
            let start = start.min(bytes.len());
            let end = end.clamp(start, bytes.len());
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(&bytes[start..end]))),
            )))
        }
        "to_list" => {
            if !positional.is_empty() {
                bail!("bytes.to_list() expects no arguments, got {}", positional.len());
            }
            let values: Vec<i64> = bytes.iter().map(|byte| *byte as i64).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Int(values))),
            )))
        }
        _ => Ok(None),
    }
}

/// One byte as an `Int`, with the language's index rule: negative counts from
/// the end, outside is nil.
fn byte_at(bytes: &[u8], index: i64) -> RuntimeVal {
    let index = if index < 0 { bytes.len() as i64 + index } else { index };
    if index < 0 {
        return RuntimeVal::Nil;
    }
    bytes
        .get(index as usize)
        .map_or(RuntimeVal::Nil, |byte| RuntimeVal::Int(*byte as i64))
}

fn byte_index_arg(value: &RuntimeVal, context: &str) -> anyhow::Result<usize> {
    let RuntimeVal::Int(index) = value else {
        bail!("{context} must be Int");
    };
    if *index < 0 {
        bail!("{context} must be non-negative, got {index}");
    }
    Ok(*index as usize)
}
