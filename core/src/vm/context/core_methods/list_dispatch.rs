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
            let list = clone_list(receiver, heap)?;
            if list.is_empty() {
                return Ok(Some(RuntimeVal::Nil));
            }
            let first = list_runtime_items(list, heap)
                .into_iter()
                .next()
                .unwrap_or(RuntimeVal::Nil);
            Ok(Some(first))
        }
        "last" => {
            if !positional.is_empty() {
                bail!("list.last() expects no arguments, got {}", positional.len());
            }
            let list = clone_list(receiver, heap)?;
            let items = list_runtime_items(list, heap);
            let last = items.into_iter().last().unwrap_or(RuntimeVal::Nil);
            Ok(Some(last))
        }
        "get" => {
            if positional.len() != 1 {
                bail!("list.get() expects 1 argument (index), got {}", positional.len());
            }
            let RuntimeVal::Int(idx) = &positional[0] else {
                bail!("list.get() index must be Int");
            };
            let list = clone_list(receiver, heap)?;
            if *idx < 0 || *idx as usize >= list.len() {
                return Ok(Some(RuntimeVal::Nil));
            }
            let items = list_runtime_items(list, heap);
            Ok(Some(items.into_iter().nth(*idx as usize).unwrap_or(RuntimeVal::Nil)))
        }
        "skip" => {
            if positional.len() != 1 {
                bail!("list.skip() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.skip() count must be Int");
            };
            let mut list = clone_list(receiver, heap)?;
            if *n > 0 {
                list.drain_prefix(*n as usize);
            }
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(list)))))
        }
        "take" => {
            if positional.len() != 1 {
                bail!("list.take() expects 1 argument (count), got {}", positional.len());
            }
            let RuntimeVal::Int(n) = &positional[0] else {
                bail!("list.take() count must be Int");
            };
            let list = clone_list(receiver, heap)?;
            let taken = list.take_prefix(*n as usize);
            Ok(Some(RuntimeVal::Obj(heap.alloc(HeapValue::List(taken)))))
        }
        "unique" => {
            if !positional.is_empty() {
                bail!("list.unique() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let mut unique: Vec<RuntimeVal> = Vec::new();
            for item in items {
                if !unique.iter().any(|seen| runtime_values_equal(seen, &item)) {
                    unique.push(item);
                }
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(unique))),
            )))
        }
        "contains" => {
            if positional.len() != 1 {
                bail!("list.contains() expects 1 argument (value), got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            Ok(Some(RuntimeVal::Bool(
                items.iter().any(|item| runtime_values_equal(item, &positional[0])),
            )))
        }
        "index_of" => {
            if positional.len() != 1 {
                bail!("list.index_of() expects 1 argument (value), got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let index = items
                .iter()
                .position(|item| runtime_values_equal(item, &positional[0]))
                .map(|index| index as i64)
                .unwrap_or(-1);
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
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.reverse();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "pop" => {
            if !positional.is_empty() {
                bail!("list.pop() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            Ok(Some(items.into_iter().last().unwrap_or(RuntimeVal::Nil)))
        }
        "push" => {
            if positional.len() != 1 {
                bail!("list.push() expects 1 argument (value), got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.push(positional[0]);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "slice" => {
            if positional.is_empty() || positional.len() > 2 {
                bail!(
                    "list.slice() expects 1 or 2 arguments (start[, end]), got {}",
                    positional.len()
                );
            }
            let start = list_index_arg(&positional[0], "list.slice() start")?;
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let end = match positional.get(1) {
                Some(value) => list_index_arg(value, "list.slice() end")?.min(items.len()),
                None => items.len(),
            };
            let sliced = if start >= end {
                Vec::new()
            } else {
                items[start..end].to_vec()
            };
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(sliced))),
            )))
        }
        "insert" => {
            if positional.len() != 2 {
                bail!(
                    "list.insert() expects 2 arguments (index, value), got {}",
                    positional.len()
                );
            }
            let index = list_index_arg(&positional[0], "list.insert() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            if index > items.len() {
                bail!("list.insert() index {} out of bounds (len={})", index, items.len());
            }
            items.insert(index, positional[1]);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "remove_at" => {
            if positional.len() != 1 {
                bail!("list.remove_at() expects 1 argument (index), got {}", positional.len());
            }
            let index = list_index_arg(&positional[0], "list.remove_at() index")?;
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            if index >= items.len() {
                bail!("list.remove_at() index {} out of bounds (len={})", index, items.len());
            }
            let old = items.remove(index);
            let updated = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(items))));
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vec![updated, old]))),
            )))
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
            let updated = RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(items))));
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(vec![updated, old]))),
            )))
        }
        "sort" => {
            if !positional.is_empty() {
                bail!("list.sort() expects no arguments, got {}", positional.len());
            }
            let mut items = list_runtime_items(clone_list(receiver, heap)?, heap);
            items.sort_by(compare_runtime_values);
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(items))),
            )))
        }
        "concat" => {
            if positional.len() != 1 {
                bail!("list.concat() expects 1 argument (list), got {}", positional.len());
            }
            let lhs = list_runtime_items(clone_list(receiver, heap)?, heap);
            let rhs = list_runtime_items(clone_list(&positional[0], heap)?, heap);
            let merged: Vec<RuntimeVal> = lhs.into_iter().chain(rhs).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(merged))),
            )))
        }
        "zip" => {
            if positional.len() != 1 {
                bail!("list.zip() expects 1 argument (other list), got {}", positional.len());
            }
            let lhs = list_runtime_items(clone_list(receiver, heap)?, heap);
            let rhs = list_runtime_items(clone_list(&positional[0], heap)?, heap);
            let mut pairs = Vec::with_capacity(lhs.len().min(rhs.len()));
            for (a, b) in lhs.into_iter().zip(rhs) {
                pairs.push(RuntimeVal::Obj(
                    heap.alloc(HeapValue::List(TypedList::Mixed(vec![a, b]))),
                ));
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
            )))
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
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(flat))),
            )))
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
                chunks.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(chunk)))));
                i = end;
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(chunks))),
            )))
        }
        "enumerate" => {
            if !positional.is_empty() {
                bail!("list.enumerate() expects no arguments, got {}", positional.len());
            }
            let items = list_runtime_items(clone_list(receiver, heap)?, heap);
            let mut pairs = Vec::with_capacity(items.len());
            for (i, item) in items.into_iter().enumerate() {
                pairs.push(RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::Mixed(vec![
                    RuntimeVal::Int(i as i64),
                    item,
                ])))));
            }
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(pairs))),
            )))
        }
        "chain" => {
            if positional.len() != 1 {
                bail!("list.chain() expects 1 argument (list), got {}", positional.len());
            }
            let lhs = list_runtime_items(clone_list(receiver, heap)?, heap);
            let rhs = list_runtime_items(clone_list(&positional[0], heap)?, heap);
            let merged: Vec<RuntimeVal> = lhs.into_iter().chain(rhs).collect();
            Ok(Some(RuntimeVal::Obj(
                heap.alloc(HeapValue::List(TypedList::Mixed(merged))),
            )))
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
