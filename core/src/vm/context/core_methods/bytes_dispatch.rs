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
        // A `Bytes` is a sequence of numbers, so the three reductions mean the
        // same here as on a list — and a receiver kind that answers `first`,
        // `len` and `contains` but not `sum` would be the half-surface this
        // dispatch was unified to remove.
        "sum" => {
            if !positional.is_empty() {
                bail!("bytes.sum() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Int(
                bytes
                    .iter()
                    .fold(0i64, |total, byte| total.wrapping_add(i64::from(*byte))),
            )))
        }
        "min" | "max" => {
            if !positional.is_empty() {
                bail!("bytes.{method}() expects no arguments, got {}", positional.len());
            }
            let extreme = if method == "max" {
                bytes.iter().max()
            } else {
                bytes.iter().min()
            };
            // Empty answers nil, as `first` does here and as `min` does on a list.
            Ok(Some(match extreme {
                Some(byte) => RuntimeVal::Int(i64::from(*byte)),
                None => RuntimeVal::Nil,
            }))
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
            Ok(Some(
                found.map_or(RuntimeVal::Nil, |index| RuntimeVal::Int(index as i64)),
            ))
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
            let start = super::slice_position(&positional[0], bytes.len(), "bytes.slice() start")?;
            let end = match positional.get(1) {
                Some(RuntimeVal::Nil) | None => bytes.len(),
                Some(value) => super::slice_position(value, bytes.len(), "bytes.slice() end")?,
            };
            let end = end.max(start);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(&bytes[start..end]))),
            )))
        }
        // A contiguous run of bytes is still bytes.
        "take" | "skip" => {
            if positional.len() != 1 {
                bail!("bytes.{method}() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(count) = &positional[0] else {
                bail!("bytes.{method}() count must be Int");
            };
            if *count < 0 {
                bail!("bytes.{method}() count must be non-negative, got {count}");
            }
            let count = (*count as usize).min(bytes.len());
            let kept = if method == "take" {
                &bytes[..count]
            } else {
                &bytes[count..]
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(kept))),
            )))
        }
        // The three that used to exist only as `bytes.f(b, …)` module
        // functions. Each is a receiver-first question about a `Bytes`, so it
        // belongs here with the rest and the module forwards to it — the split
        // is what let `bytes.slice(b, 3, 1)` raise while `b.slice(3, 1)`
        // answered an empty window.
        "to_string_utf8" => {
            if !positional.is_empty() {
                bail!("bytes.to_string_utf8() expects no arguments, got {}", positional.len());
            }
            // Raises on invalid UTF-8, unlike `to_string_lossy` next door: the
            // two exist precisely so the caller says which one they mean.
            let text = core::str::from_utf8(&bytes).map_err(|err| anyhow!("bytes are not valid UTF-8: {err}"))?;
            Ok(Some(make_string_val(text, heap)))
        }
        "to_string_lossy" => {
            if !positional.is_empty() {
                bail!("bytes.to_string_lossy() expects no arguments, got {}", positional.len());
            }
            Ok(Some(make_string_val(&String::from_utf8_lossy(&bytes), heap)))
        }
        "concat" => {
            if positional.len() != 1 {
                bail!("bytes.concat() expects 1 argument (other), got {}", positional.len());
            }
            let RuntimeVal::Obj(other) = &positional[0] else {
                bail!("bytes.concat() argument must be Bytes");
            };
            let Some(HeapValue::Bytes(other)) = heap.get(*other) else {
                bail!("bytes.concat() argument must be Bytes");
            };
            let mut out = Vec::with_capacity(bytes.len() + other.len());
            out.extend_from_slice(&bytes);
            out.extend_from_slice(other);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(out))),
            )))
        }
        // Shape-preserving, element-type-independent, and therefore a `Bytes`
        // again — the same reading `take`, `skip`, `slice` and `concat` already
        // take. `reverse` was on `List` and on `Str` and on neither of the two
        // carriers that have every other read of the list surface.
        "reverse" => {
            if !positional.is_empty() {
                bail!("bytes.reverse() expects no arguments, got {}", positional.len());
            }
            let mut out = bytes.to_vec();
            out.reverse();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(out))),
            )))
        }
        // Byte values are ordered scalars, so both mean here exactly what they
        // mean on a `List<Int>` — and both keep the carrier, because every
        // element of the answer is still a byte.
        "sort" => {
            if !positional.is_empty() {
                bail!("bytes.sort() expects no arguments, got {}", positional.len());
            }
            let mut out = bytes.to_vec();
            out.sort_unstable();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(out))),
            )))
        }
        "unique" => {
            if !positional.is_empty() {
                bail!("bytes.unique() expects no arguments, got {}", positional.len());
            }
            // Later duplicates dropped, order preserved — `List::unique`'s
            // rule. 256 possible values, so the "seen" set is a bitmap.
            let mut seen = [false; 256];
            let mut out = Vec::with_capacity(bytes.len());
            for byte in bytes.iter() {
                if !seen[*byte as usize] {
                    seen[*byte as usize] = true;
                    out.push(*byte);
                }
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(out))),
            )))
        }
        // `count` is `index_of`'s sibling — how many rather than where — and
        // `index_of` is on all four sequence carriers while `count` was on
        // `Str` alone. A value no byte can equal counts zero, which is the
        // same answer `contains` gives it.
        "count" => {
            if positional.len() != 1 {
                bail!("bytes.count() expects 1 argument (value), got {}", positional.len());
            }
            let RuntimeVal::Int(needle) = &positional[0] else {
                bail!("bytes.count() value must be Int");
            };
            let found = u8::try_from(*needle)
                .map(|needle| bytes.iter().filter(|byte| **byte == needle).count())
                .unwrap_or(0);
            Ok(Some(RuntimeVal::Int(found as i64)))
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
        // The operations whose answer is a *list of the elements*, whatever the
        // elements were: they mean the same thing here as on a `List` and
        // cannot keep the carrier, so they are the list's, reached by
        // materializing. One arm rather than six bodies — `enumerate`'s pairs,
        // `chunk`'s grouping and `flatten`'s one level are rules, and a second
        // copy of a rule is how two spellings of one operation come to
        // disagree.
        "enumerate" | "zip" | "chain" | "chunk" => {
            let values: Vec<i64> = bytes.iter().map(|byte| *byte as i64).collect();
            let list = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Int(values))));
            super::dispatch_list_builtin_method(&list, method, positional, heap)
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
