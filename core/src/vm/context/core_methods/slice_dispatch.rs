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

    match method {
        "len" => {
            if !positional.is_empty() {
                bail!("slice.len() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Int(slice.len as i64)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("slice.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(slice.len == 0)))
        }
        "get" => {
            if positional.len() != 1 {
                bail!("slice.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(index) = &positional[0] else {
                bail!("slice.get() index must be Int");
            };
            if *index < 0 || *index as usize >= slice.len {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(slice_item(&slice, *index as usize, heap)))
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
            let RuntimeVal::Int(start) = &positional[0] else {
                bail!("slice.slice() start must be Int");
            };
            let start = (*start).max(0) as usize;
            let end = match positional.get(1) {
                Some(RuntimeVal::Int(end)) => (*end).max(0) as usize,
                Some(RuntimeVal::Nil) | None => slice.len,
                Some(_) => bail!("slice.slice() end must be Int"),
            };
            let start = start.min(slice.len);
            let end = end.clamp(start, slice.len);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::Slice(Arc::new(
                SliceValue {
                    source: slice.source,
                    start: slice.start + start,
                    len: end - start,
                },
            ))))))
        }
        "to_list" => {
            if !positional.is_empty() {
                bail!("slice.to_list() expects no arguments, got {}", positional.len());
            }
            let items: Vec<RuntimeVal> = (0..slice.len)
                .map(|index| slice_item(&slice, index, heap))
                .collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
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
