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
        // `min`/`max`/`sum`: the reductions a list API is expected to have.
        //
        // `map`, `filter`, `reduce`, `unique`, `zip` and `chunk` were all here
        // and these were not, so the three most ordinary questions about a list
        // of numbers had to be written as folds — with a comparison lambda that
        // then had to agree with `sort`'s order, which nothing checked.
        "min" | "max" => {
            if !positional.is_empty() {
                bail!("list.{method}() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            // The order is `sort`'s, from the same comparison: `xs.sort().first()`
            // and `xs.min()` cannot disagree, because there is only one rule.
            let index = typed_list_extreme_index(list, heap, method == "max");
            Ok(Some(match index {
                Some(index) => typed_list_element(handle, index, heap),
                None => RuntimeVal::Nil,
            }))
        }
        "sum" => {
            if !positional.is_empty() {
                bail!("list.sum() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            Ok(Some(typed_list_sum(list, heap)?))
        }
        "unique" => {
            if !positional.is_empty() {
                bail!("list.unique() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let unique = typed_list_unique(list, heap)?;
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
                typed_list_position(list, &positional[0], heap)?.is_some(),
            )))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!("list.index_of() expects 1 argument (value), got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let index = typed_list_position(list, &positional[0], heap)?
                .map_or(RuntimeVal::Nil, |index| RuntimeVal::Int(index as i64));
            Ok(Some(index))
        }
        "is_empty" => {
            if !positional.is_empty() {
                bail!("list.is_empty() expects no arguments, got {}", positional.len());
            }
            Ok(Some(RuntimeVal::Bool(clone_list(receiver, heap)?.is_empty())))
        }
        // The inverse of `b.to_list()`, which existed on its own for as long as
        // the way *back* was spelled `bytes.from_list(xs)` — a constructor in
        // another module for what is a question about this list. The module
        // spelling stays and forwards here.
        "to_bytes" => {
            if !positional.is_empty() {
                bail!("list.to_bytes() expects no arguments, got {}", positional.len());
            }
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let checked = |value: i64| {
                u8::try_from(value)
                    .map_err(|_| anyhow::anyhow!("list.to_bytes() expects byte values in 0..=255, got {value}"))
            };
            let bytes: Vec<u8> = match list {
                TypedList::Int(values) => values
                    .iter()
                    .map(|value| checked(*value))
                    .collect::<anyhow::Result<_>>()?,
                TypedList::Mixed(values) => values
                    .iter()
                    .map(|value| match value {
                        RuntimeVal::Int(value) => checked(*value),
                        other => bail!("list.to_bytes() expects Int items, got {}", other.type_name_in(heap)),
                    })
                    .collect::<anyhow::Result<_>>()?,
                // An empty list has no element type to disagree with.
                TypedList::Bool(values) if values.is_empty() => Vec::new(),
                _ => bail!("list.to_bytes() expects Int items"),
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::Bytes(Arc::<[u8]>::from(bytes))),
            )))
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
        // In place, answering the list itself — the same as `xs.push(v)` from
        // LK. This used to copy the whole list into a new one and hand that
        // back, so whether pushing changed the list depended on which side
        // called the method.
        "push" => {
            if positional.len() != 1 {
                bail!("list.push() expects 1 argument (value), got {}", positional.len());
            }
            // Read the text out before the mutable borrow: a `TypedList::String`
            // holds `Arc<str>`, which no `RuntimeVal` carries past seven bytes.
            let string_value = runtime_value_text(&positional[0], heap).map(Arc::<str>::from);
            let Some(HeapValue::List(list)) = heap.get_mut(handle) else {
                return Ok(None);
            };
            list.push(positional[0], string_value)?;
            Ok(Some(*receiver))
        }
        "clear" => {
            if !positional.is_empty() {
                bail!("list.clear() expects no arguments, got {}", positional.len());
            }
            if let Some(HeapValue::List(list)) = heap.get_mut(handle) {
                list.clear();
            }
            Ok(Some(*receiver))
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
            let source_len = clone_list(receiver, heap)?.len();
            let start = super::slice_position(&positional[0], source_len, "list.slice() start")?;
            let end = match positional.get(1) {
                Some(RuntimeVal::Nil) | None => source_len,
                Some(value) => super::slice_position(value, source_len, "list.slice() end")?,
            };
            // Clamped, like every other position in this language.
            let end = end.max(start);
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
            let value = positional[1];
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            // `-1` inserts before the last element, the same "from the end" the
            // read side means; `len` (the past-the-end position) stays legal
            // because that is where an append goes.
            let index = write_index_arg(&positional[0], list.len(), "list.insert() index")?;
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
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            let index = write_index_arg(&positional[0], list.len(), "list.remove_at() index")?;
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
            let value = positional[1];
            let Some(HeapValue::List(list)) = heap.get(handle) else {
                return Ok(None);
            };
            // Worded like the index-assignment path, not like `insert`/`remove_at`.
            // The compiler rewrites every `xs.set(k, v)` into a `SetIndex`, so
            // this arm is not reached from compiled code — a literal receiver, an
            // unannotated parameter, a map-indexed receiver and a module-global
            // list all report the assignment wording, and a sentinel put here
            // survived the whole test suite and every example unseen. It stays
            // because `set` is a real method and a program should not depend on
            // which route the compiler picked; what it must not do is *disagree*
            // with that route, which is what "list.set() index N out of bounds
            // (len=N)" did. `insert`/`remove_at` keep their own wording because
            // they have no rewrite and that wording is what a program sees.
            let RuntimeVal::Int(requested) = positional[0] else {
                bail!("list index must be Int");
            };
            let resolved = if requested < 0 {
                list.len() as i64 + requested
            } else {
                requested
            };
            if resolved < 0 {
                // The same wording as the other end. `xs[-1]` is the last
                // element, so "must be non-negative" states a rule the language
                // does not have — and the assertion two lines below in this
                // file's own test, that `set(-1, 7)` succeeds, is the proof.
                bail!("list index {requested} out of bounds");
            }
            let index = resolved as usize;
            if index >= list.len() {
                bail!("list index {requested} out of bounds");
            }
            // In place, answering the receiver — the same thing the compiler's
            // own lowering does. This arm used to copy the list and return a
            // `[updated, old]` pair, so the fallback and the fast path
            // disagreed about both the effect and the answer; only the fact
            // that the fallback is unreachable for `set` kept it from showing.
            let written_in_place = match (heap.get_mut(handle), value) {
                (Some(HeapValue::List(TypedList::Int(values))), RuntimeVal::Int(value)) => {
                    values[index] = value;
                    true
                }
                (Some(HeapValue::List(TypedList::Float(values))), RuntimeVal::Float(value)) => {
                    values[index] = value;
                    true
                }
                (Some(HeapValue::List(TypedList::Bool(values))), RuntimeVal::Bool(value)) => {
                    values[index] = value;
                    true
                }
                (Some(HeapValue::List(TypedList::Mixed(values))), value) => {
                    values[index] = value;
                    true
                }
                _ => false,
            };
            if !written_in_place {
                let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
                items[index] = value;
                let items = TypedList::from_runtime_values(&items, heap);
                if let Some(slot) = heap.get_mut(handle) {
                    *slot = HeapValue::List(items);
                }
            }
            Ok(Some(*receiver))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::val::TypedList;

    /// The `set` arm words its range failures like the index-assignment path.
    ///
    /// Reached from here rather than from LK because it cannot be reached from
    /// LK: the compiler rewrites every `xs.set(k, v)` into a `SetIndex`, and a
    /// sentinel put in this arm survived the whole test suite and every example
    /// unseen. That is exactly why it needs a test — an arm no program reaches is
    /// an arm whose wording nothing checks, and this one used to say
    /// `list.set() index N out of bounds (len=N)` where the route programs
    /// actually take says `list index N out of bounds`.
    #[test]
    fn the_unreachable_set_arm_agrees_with_the_assignment_path() {
        let mut heap = HeapStore::new();
        let handle = heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2])));
        let receiver = RuntimeVal::Obj(handle);

        let mut message = |index: i64| {
            dispatch_list_builtin_method(
                &receiver,
                "set",
                &[RuntimeVal::Int(index), RuntimeVal::Int(5)],
                &mut heap,
            )
            .expect_err("out of range")
            .to_string()
        };
        assert_eq!(message(9), "list index 9 out of bounds");
        assert_eq!(message(-9), "list index -9 out of bounds");

        // And it still writes, in place, answering the receiver — the effect the
        // rewritten route has.
        let answer =
            dispatch_list_builtin_method(&receiver, "set", &[RuntimeVal::Int(-1), RuntimeVal::Int(7)], &mut heap)
                .expect("in range")
                .expect("handled");
        assert_eq!(answer, receiver, "`set` answers the list it wrote to");
        assert!(
            matches!(heap.get(handle), Some(HeapValue::List(TypedList::Int(values))) if values == &[1, 7]),
            "the write lands in the receiver's own list"
        );
    }
}
