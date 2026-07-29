use super::*;

/// Dispatch built-in list instance methods: join.
/// Returns Some(value) if handled, None to fall through.
pub(super) fn dispatch_list_builtin_method(
    receiver: &RuntimeVal,
    method: &str,
    positional: &[RuntimeVal],
    heap: &mut HeapStore,
) -> anyhow::Result<Option<RuntimeVal>> {
    let RuntimeVal::Obj(handle) = receiver else {
        return Ok(None);
    };
    let handle = *handle;
    if !matches!(heap.get(handle), Some(HeapValue::List(_))) {
        return Ok(None);
    }
    match method {
        "first" => {
            if !positional.is_empty() {
                bail!("list.first() expects no arguments, got {}", positional.len());
            }
            Ok(Some(typed_list_element(handle, 0, heap)))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("list.last() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let len = list.len();
            Ok(Some(match len.checked_sub(1) {
                Some(last) => typed_list_element(handle, last, heap),
                None => RuntimeVal::Nil,
            }))
        }
        "get" => {
            if positional.len() != 1 {
                bail!("list.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(idx) = &positional[0] else {
                bail!("list.get() index must be Int");
            };
            let list = clone_list(receiver, heap)?;
            // `.get(i)` is `xs[i]` that answers nil instead of failing, so it
            // indexes the same way: a negative counts from the end.
            //
            // This arm used to reject a negative outright — and never got the
            // chance to, because the compiler lowers every `x.get(k)` call to
            // `GetIndex` (`lower_map_get_method_call`). So the rule written
            // here was not the rule the language had; `xs.get(-1)` answered the
            // last element, as it still does. Leaving the two spellings
            // disagreeing meant whichever path a call happened to take decided
            // its meaning.
            let index = if *idx < 0 { list.len() as i64 + *idx } else { *idx };
            if index < 0 || index as usize >= list.len() {
                return Ok(Some(RuntimeVal::Nil));
            }
            Ok(Some(typed_list_element(handle, index as usize, heap)))
        }
        "skip" => {
            if positional.len() != 1 {
                bail!("list.skip() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.skip() count must be Int");
            };
            // A count is not an index: there is nothing for a negative one to
            // mean, so it is an error rather than a value. It used to be
            // ignored (`if *n > 0`), which turned an off-by-one that computed
            // `-1` into "the whole list" — the answer most likely to look
            // right. `iter.skip` has always raised here; the two spellings now
            // agree.
            if *n < 0 {
                bail!("list.skip() count must be non-negative, got {n}");
            }
            let mut list = clone_list(receiver, heap)?;
            list.drain_prefix(*n as usize);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(list)))))
        }
        "take" => {
            if positional.len() != 1 {
                bail!("list.take() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.take() count must be Int");
            };
            // As in `skip` — and here the old code was not even ignoring the
            // negative, it was casting it: `-1 as usize` is `usize::MAX`, so
            // `take(-1)` took everything by way of an unchecked wrap.
            if *n < 0 {
                bail!("list.take() count must be non-negative, got {n}");
            }
            let list = clone_list(receiver, heap)?;
            let taken = list.take_prefix(*n as usize);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(taken)))))
        }
        "unique" => {
            if !positional.is_empty() {
                bail!("list.unique() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let unique = typed_list_unique(list, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(unique)))))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!("list.contains() expects 1 argument (value), got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            Ok(Some(RuntimeVal::Bool(
                typed_list_position(list, &positional[0], heap).is_some(),
            )))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!("list.index_of() expects 1 argument (value), got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let index = typed_list_position(list, &positional[0], heap).map_or(-1, |index| index as i64);
            Ok(Some(RuntimeVal::Int(index)))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("list.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(clone_list(receiver, heap)?.is_empty())))
        }
        "reverse" => {
            if !positional.is_empty() {
                bail!("list.reverse() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let reversed = typed_list_reversed(list);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(reversed)))))
        }
        // `pop` takes the last element *off*. It used to be a byte-for-byte
        // duplicate of `last` above — same body, same declared type, same doc
        // sentence — so the language had two names for "peek" and no way at all
        // to remove the last element (`remove_at` does not mutate either). A
        // name that every language uses for "remove and return" must not
        // quietly mean "read".
        "pop" => {
            if !positional.is_empty() {
                bail!("list.pop() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let Some(last) = list.len().checked_sub(1) else {
                return Ok(Some(RuntimeVal::Nil));
            };
            let value = typed_list_element(handle, last, heap);
            if let Some(HeapValue::List(list)) = heap.get_mut(handle) {
                list.truncate(last);
            }
            Ok(Some(value))
        }
        "push" => {
            if positional.len() != 1 {
                bail!("list.push() expects 1 argument (value), got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.push(positional[0]);
            let items = TypedList::from_runtime_values(&items, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(items)))))
        }
        "slice" => {
            // A window, not a copy. This used to materialize `items[a..b]` into
            // a fresh list, which meant every window over a large list
            // duplicated the part it looked at. `to_list()` is how you ask for
            // the copy now, and asking is the point — the two are different
            // operations and used to share one name.
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "list.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let start = list_index_arg(&positional[0], "list.slice() start")?;
            let source_len = clone_list(receiver, heap)?.len();
            let end = match positional.get(1) {
                Some(value) => list_index_arg(value, "list.slice() end")?,
                None => source_len,
            };
            // Clamped, like every other position in this language.
            let start = start.min(source_len);
            let end = end.clamp(start, source_len);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::Slice(Arc::new(
                SliceValue {
                    source: *receiver,
                    start,
                    len: end - start,
                },
            ))))))
        }
        "insert" => {
            if positional.len() != 2 {
                bail!(
                    "list.insert() expects 2 arguments (index, value), got {}",
                    positional.len()
                );
            }
            let index = list_index_arg(&positional[0], "list.insert() index")?;
            let value = positional[1];
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            if index > list.len() {
                bail!("list.insert() index {} out of bounds (len={})", index, list.len());
            }
            // In place, like `push` and `set`. It used to copy the whole list,
            // insert, and hand back a *new* one — so `xs.insert(…)` left `xs`
            // alone while `xs.push(…)` changed it, two opposite answers to
            // "does adding an element change this list".
            //
            // The typed cases move memory and keep the representation; a value
            // that does not fit the representation (or a string, whose text
            // lives on the heap) goes the long way and is written back to the
            // same handle, so it mutates either way.
            let inserted_in_place = match (heap.get_mut(handle), value) {
                (Some(HeapValue::List(TypedList::Int(values))), RuntimeVal::Int(value)) => {
                    values.insert(index, value);
                    true
                }
                (Some(HeapValue::List(TypedList::Float(values))), RuntimeVal::Float(value)) => {
                    values.insert(index, value);
                    true
                }
                (Some(HeapValue::List(TypedList::Bool(values))), RuntimeVal::Bool(value)) => {
                    values.insert(index, value);
                    true
                }
                (Some(HeapValue::List(TypedList::Mixed(values))), value) => {
                    values.insert(index, value);
                    true
                }
                _ => false,
            };
            if !inserted_in_place {
                let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
                items.insert(index, value);
                let items = TypedList::from_runtime_values(&items, heap);
                if let Some(slot) = heap.get_mut(handle) {
                    *slot = HeapValue::List(items);
                }
            }
            Ok(Some(*receiver))
        }
        "remove_at" => {
            if positional.len() != 1 {
                bail!("list.remove_at() expects 1 argument (index), got {}", positional.len());
            }
            let index = list_index_arg(&positional[0], "list.remove_at() index")?;
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            if index >= list.len() {
                bail!("list.remove_at() index {} out of bounds (len={})", index, list.len());
            }
            // Returns the element it removed, the way `pop` does. It used to
            // return a two-element list `[updated, old]` — the only method in
            // the language shaped that way — *and* leave the receiver alone,
            // so the "updated" list was a copy nobody was holding.
            let removed = typed_list_element(handle, index, heap);
            if let Some(HeapValue::List(list)) = heap.get_mut(handle) {
                list.remove_at(index);
            }
            Ok(Some(removed))
        }
        "set" => {
            if positional.len() != 2 {
                bail!(
                    "list.set() expects 2 arguments (index, value), got {}",
                    positional.len()
                );
            }
            let index = list_index_arg(&positional[0], "list.set() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let Some(slot) = items.get_mut(index) else {
                bail!("list.set() index {} out of bounds (len={})", index, items.len());
            };
            let old = core::mem::replace(slot, positional[1]);
            let items = TypedList::from_runtime_values(&items, heap);
            let updated = RuntimeVal::Obj(heap.alloc(HeapValue::List(items)));
            let pair = TypedList::from_runtime_values(&[updated, old], heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(pair)))))
        }
        "sort" => {
            if !positional.is_empty() {
                bail!("list.sort() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let sorted = typed_list_sorted(list, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(sorted)))))
        }
        // One operation, two spellings — they had two identical bodies.
        "concat" | "chain" => {
            if positional.len() != 1 {
                bail!("list.{method}() expects 1 argument (list), got {}", positional.len());
            }
            let merged = {
                let left = clone_list(receiver, heap)?;
                let right = clone_list(&positional[0], heap)?;
                match typed_lists_concatenated(&left, &right) {
                    Some(merged) => merged,
                    // Different representations: materializing is the only
                    // thing that can join an `Int` list to a `String` one.
                    None => {
                        let mut items = list_runtime_items(left, heap);
                        items.extend(list_runtime_items(right, heap));
                        TypedList::from_runtime_values(&items, heap)
                    }
                }
            };
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(merged)))))
        }
        "zip" => {
            if positional.len() != 1 {
                bail!("list.zip() expects 1 argument (other list), got {}", positional.len());
            }
            let lhs = list_runtime_items(clone_list(receiver, heap)?, heap);
            let rhs = list_runtime_items(clone_list(&positional[0], heap)?, heap);
            let mut pairs = Vec::with_capacity(lhs.len().min(rhs.len()));
            for (a, b) in lhs.into_iter().zip(rhs) {
                let pair = TypedList::from_runtime_values(&[a, b], heap);
                pairs.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(pair))));
            }
            let pairs = TypedList::from_runtime_values(&pairs, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(pairs)))))
        }
        "flatten" => {
            if !positional.is_empty() {
                bail!("list.flatten() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let mut flat: Vec<RuntimeVal> = Vec::new();
            for item in items {
                if let RuntimeVal::Obj(h) = &item
                    && let Some(HeapValue::List(inner)) = heap.get(*h)
                {
                    let inner = inner.clone();
                    flat.extend(list_runtime_items(inner, heap));
                    continue;
                }
                flat.push(item);
            }
            let flat = TypedList::from_runtime_values(&flat, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(flat)))))
        }
        "chunk" => {
            if positional.len() != 1 {
                bail!("list.chunk() expects 1 argument (size), got {}", positional.len());
            }
            let RuntimeVal::Int(size) = &positional[0] else {
                bail!("list.chunk() size must be Int");
            };
            if *size <= 0 {
                bail!("list.chunk() size must be positive");
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let mut chunks: Vec<RuntimeVal> = Vec::new();
            let mut i = 0;
            while i < items.len() {
                let end = (i + *size as usize).min(items.len());
                let chunk: Vec<RuntimeVal> = items[i..end].to_vec();
                let chunk = TypedList::from_runtime_values(&chunk, heap);
                chunks.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(chunk))));
                i = end;
            }
            let chunks = TypedList::from_runtime_values(&chunks, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(chunks)))))
        }
        "enumerate" => {
            if !positional.is_empty() {
                bail!("list.enumerate() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let mut pairs = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                let pair = TypedList::from_runtime_values(&[RuntimeVal::Int(i as i64), item], heap);
                pairs.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(pair))));
            }
            let pairs = TypedList::from_runtime_values(&pairs, heap);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(pairs)))))
        }
        "join" => {
            if positional.len() != 1 {
                bail!("list.join() expects 1 argument (separator), got {}", positional.len());
            }
            let sep = extract_string_detached(&positional[0], heap, "list.join() separator")?;
            let parts = match heap.get(handle) {
                Some(HeapValue::List(list)) => list_join_parts(list, heap)?,
                _ => return Ok(None),
            };
            let joined = parts.join(sep.as_str());
            Ok(Some(make_string_val(&joined, heap)))
        }
        _ => Ok(None),
    }
}
