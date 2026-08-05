use super::*;

/// Built-in methods on a slice — a window over a list that does not copy it.
///
/// These were `stdlib/crates/slice`, a module whose eight exports overlapped
/// `bytes` in six names and whose other two (`sub`, `to_string`) were `bytes`'
/// operations under different spellings. What it had that `bytes` did not is
/// the list window, and that is what moved here: taking a window is something a
/// list can do, not a module you have to import first.
pub(super) fn dispatch_slice_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let Some(HeapValue::Slice(slice)) = heap.get(*handle) else {
        return Ok(None);
    };
    let slice = slice.clone();
    // Not `slice.len`: the source can have shrunk since the window was taken,
    // and every method here has to agree about how long it is *now*.
    let len = slice.live_len(heap);

    match method {
        "len" => {
            if !positional.is_empty() {
                bail!("slice.len() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Int(len as i64)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("slice.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(len == 0)))
        }
        "get" => {
            if positional.len() != 1 {
                bail!("slice.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(index) = &positional[0] else {
                bail!("slice.get() index must be Int");
            };
            // Same rule as `list.get` and as `w[i]`: negative counts from the
            // window's end (see the note in `list_dispatch.rs`).
            let index = if *index < 0 { len as i64 + *index } else { *index };
            if index < 0 || index as usize >= len {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(slice_item(&slice, index as usize, heap)))
        }
        // A window on a window, resolved against the original rather than
        // nested — otherwise a loop that keeps re-slicing builds a chain.
        "slice" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "slice.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let start = super::slice_position(&positional[0], len, "slice.slice() start")?;
            let end = match positional.get(1) {
                Some(RuntimeVal::Nil) | None => len,
                Some(value) => super::slice_position(value, len, "slice.slice() end")?,
            };
            let end = end.max(start);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::Slice(Arc::new(
                SliceValue {
                    source: slice.source,
                    start: slice.start + start,
                    len: end - start,
                },
            ))))))
        }
        // A contiguous run of a window is still a window, so these cost
        // nothing. `filter` cannot be one — what it keeps is not contiguous —
        // and materializes a list instead.
        "take" | "skip" => {
            if positional.len() != 1 {
                bail!("slice.{method}() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(count) = &positional[0] else {
                bail!("slice.{method}() count must be Int");
            };
            if *count < 0 {
                bail!("slice.{method}() count must be non-negative, got {count}");
            }
            let count = (*count as usize).min(len);
            let (start, window_len) = if method == "take" {
                (slice.start, count)
            } else {
                (slice.start + count, len - count)
            };
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::Slice(Arc::new(
                SliceValue {
                    source: slice.source,
                    start,
                    len: window_len,
                },
            ))))))
        }
        "first" => {
            if !positional.is_empty() {
                bail!("slice.first() expects no arguments, got {}", positional.len());
            }
            if len == 0 {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(slice_item(&slice, 0, heap)))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("slice.last() expects no arguments, got {}", positional.len());
            }
            if len == 0 {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(slice_item(&slice, len - 1, heap)))
        }
        // A window over a list is a sequence too, and it answers the same three
        // reductions — through the list's own helpers, so a slice and the list
        // it borrows cannot give different answers for the same elements.
        "min" | "max" | "sum" => {
            if !positional.is_empty() {
                bail!("slice.{method}() expects no arguments, got {}", positional.len());
            }
            // The window's own elements, as a list: the source may have shrunk
            // since the window was taken, so `live_len` decides how far it goes
            // — the same rule every other method here follows.
            let RuntimeVal::Obj(source) = slice.source else {
                return Ok(Some(RuntimeVal::Nil));
            };
            let Some(HeapValue::List(list)) = heap.get(source) else {
                return Ok(Some(RuntimeVal::Nil));
            };
            let window = list.window(slice.start, len);
            if method == "sum" {
                return Ok(Some(typed_list_sum(&window, heap)?));
            }
            let index = typed_list_extreme_index(&window, heap, method == "max");
            Ok(Some(match index {
                Some(index) => slice_item(&slice, index, heap),
                None => RuntimeVal::Nil,
            }))
        }
        "contains" | "index_of" => {
            if positional.len() != 1 {
                bail!("slice.{method}() expects 1 argument (value), got {}", positional.len());
            }
            let needle = positional[0];
            let mut found = None;
            for index in 0..len {
                let item = slice_item(&slice, index, heap);
                if crate::val::runtime_values_equal(&item, &needle, heap)? {
                    found = Some(index);
                    break;
                }
            }
            Ok(Some(if method == "contains" {
                RuntimeVal::Bool(found.is_some())
            } else {
                found.map_or(RuntimeVal::Nil, |index| RuntimeVal::Int(index as i64))
            }))
        }
        // A window is a *range of its source*, and a reversed range is not one
        // — so unlike `take`/`skip`/`slice`, which answer sub-windows, this
        // materializes. That is the same rule `map` already follows here.
        "reverse" => {
            if !positional.is_empty() {
                bail!("slice.reverse() expects no arguments, got {}", positional.len());
            }
            let mut items: Vec<RuntimeVal> = (0..len).map(|index| slice_item(&slice, index, heap)).collect();
            items.reverse();
            let items = TypedList::from_runtime_values(&items, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(items)))))
        }
        // Neither answer is a range of the source, so both materialize — the
        // same rule `reverse` and `map` follow here.
        "sort" | "unique" => {
            if !positional.is_empty() {
                bail!("slice.{method}() expects no arguments, got {}", positional.len());
            }
            let items: Vec<RuntimeVal> = (0..len).map(|index| slice_item(&slice, index, heap)).collect();
            let items = TypedList::from_runtime_values(&items, heap);
            // Routed through the list implementations rather than repeated:
            // `sort`'s order and `unique`'s "later duplicates dropped, order
            // preserved" are rules, and a second copy of a rule is how two
            // spellings of one operation come to disagree.
            let answer = if method == "sort" {
                typed_list_sorted(&items, heap)
            } else {
                typed_list_unique(&items, heap)?
            };
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(answer)))))
        }
        // `index_of`'s sibling, and it was on `Str` alone.
        "count" => {
            if positional.len() != 1 {
                bail!("slice.count() expects 1 argument (value), got {}", positional.len());
            }
            let needle = positional[0];
            let mut found = 0i64;
            for index in 0..len {
                let item = slice_item(&slice, index, heap);
                if crate::val::runtime_values_equal(&item, &needle, heap)? {
                    found += 1;
                }
            }
            Ok(Some(RuntimeVal::Int(found)))
        }
        "to_list" => {
            if !positional.is_empty() {
                bail!("slice.to_list() expects no arguments, got {}", positional.len());
            }
            let items: Vec<RuntimeVal> = (0..len).map(|index| slice_item(&slice, index, heap)).collect();
            let items = TypedList::from_runtime_values(&items, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(items)))))
        }
        // As in `bytes_dispatch`: the operations whose answer is a list of the
        // elements are the list's, reached by materializing the window once.
        // `join` too — a window's elements display the same as a list's.
        "enumerate" | "zip" | "chain" | "chunk" | "concat" => {
            let items: Vec<RuntimeVal> = (0..len).map(|index| slice_item(&slice, index, heap)).collect();
            let items = TypedList::from_runtime_values(&items, heap);
            let list = RuntimeVal::Obj(heap.alloc(HeapValue::List(items)));
            super::dispatch_list_builtin_method(&list, method, positional, heap)
        }
        _ => Ok(None),
    }
}

/// The element at `index` *within the window*, or nil when the source is no
/// longer a list.
///
/// Takes `&mut HeapStore` because a `TypedList::String` element is an
/// `Arc<str>` that has to be handed back as a heap string — reading one
/// element can allocate, even though the window itself never copies.
fn slice_item(slice: &SliceValue, index: usize, heap: &mut HeapStore) -> RuntimeVal {
    let RuntimeVal::Obj(handle) = slice.source else {
        return RuntimeVal::Nil;
    };
    let position = slice.start + index;

    enum Element {
        Ready(RuntimeVal),
        Text(Arc<str>),
    }
    let element = match heap.get(handle) {
        Some(HeapValue::List(list)) => match list {
            TypedList::Mixed(values) => values.get(position).copied().map(Element::Ready),
            TypedList::Int(values) => values.get(position).copied().map(RuntimeVal::Int).map(Element::Ready),
            TypedList::Float(values) => values.get(position).copied().map(RuntimeVal::Float).map(Element::Ready),
            TypedList::Bool(values) => values.get(position).copied().map(RuntimeVal::Bool).map(Element::Ready),
            TypedList::String(values) => values.get(position).cloned().map(Element::Text),
        },
        _ => None,
    };
    match element {
        Some(Element::Ready(value)) => value,
        Some(Element::Text(text)) => make_string_val(text.as_ref(), heap),
        None => RuntimeVal::Nil,
    }
}
