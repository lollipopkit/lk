use super::*;

#[derive(Clone, Copy)]
pub(super) enum RuntimePositionalArgs<'a> {
    Slice(&'a [RuntimeVal]),
    ListHandle(HeapRef),
    Prefixed {
        first: &'a RuntimeVal,
        rest: &'a [RuntimeVal],
    },
    PrefixedList {
        first: &'a RuntimeVal,
        rest: HeapRef,
    },
}

impl<'a> RuntimePositionalArgs<'a> {
    pub(super) fn len(&self, heap: &HeapStore) -> Result<usize> {
        match self {
            Self::Slice(values) => Ok(values.len()),
            Self::ListHandle(handle) => typed_list_arg_len(*handle, heap),
            Self::Prefixed { rest, .. } => Ok(rest.len() + 1),
            Self::PrefixedList { rest, .. } => Ok(typed_list_arg_len(*rest, heap)? + 1),
        }
    }

    pub(super) fn materialize_full_state_native_args(
        &self,
        native: &NativeEntry,
        heap: &mut HeapStore,
    ) -> Result<InlineNativeArgs> {
        let len = self.len(heap)?;
        Ok(match len {
            0 => InlineNativeArgs::Zero,
            1 => InlineNativeArgs::One([self.cloned_arg_at(heap, 0)?]),
            2 => InlineNativeArgs::Two([self.cloned_arg_at(heap, 0)?, self.cloned_arg_at(heap, 1)?]),
            3 => InlineNativeArgs::Three([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
            ]),
            4 => InlineNativeArgs::Four([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
                self.cloned_arg_at(heap, 3)?,
            ]),
            5 => InlineNativeArgs::Five([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
                self.cloned_arg_at(heap, 3)?,
                self.cloned_arg_at(heap, 4)?,
            ]),
            6 => InlineNativeArgs::Six([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
                self.cloned_arg_at(heap, 3)?,
                self.cloned_arg_at(heap, 4)?,
                self.cloned_arg_at(heap, 5)?,
            ]),
            7 => InlineNativeArgs::Seven([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
                self.cloned_arg_at(heap, 3)?,
                self.cloned_arg_at(heap, 4)?,
                self.cloned_arg_at(heap, 5)?,
                self.cloned_arg_at(heap, 6)?,
            ]),
            8 => InlineNativeArgs::Eight([
                self.cloned_arg_at(heap, 0)?,
                self.cloned_arg_at(heap, 1)?,
                self.cloned_arg_at(heap, 2)?,
                self.cloned_arg_at(heap, 3)?,
                self.cloned_arg_at(heap, 4)?,
                self.cloned_arg_at(heap, 5)?,
                self.cloned_arg_at(heap, 6)?,
                self.cloned_arg_at(heap, 7)?,
            ]),
            len => bail!(
                "{} FullState native argument count {} exceeds inline buffer",
                native.name,
                len
            ),
        })
    }

    pub(super) fn cloned_arg_at(&self, heap: &mut HeapStore, index: usize) -> Result<RuntimeVal> {
        match self {
            Self::Slice(values) => values
                .get(index)
                .cloned()
                .ok_or_else(|| anyhow!("runtime positional argument index {index} out of bounds")),
            Self::ListHandle(handle) => typed_list_arg_value(*handle, heap, index),
            Self::Prefixed { first, rest } => {
                if index == 0 {
                    return Ok(*(*first));
                }
                rest.get(index - 1)
                    .cloned()
                    .ok_or_else(|| anyhow!("runtime positional argument index {index} out of bounds"))
            }
            Self::PrefixedList { first, rest } => {
                if index == 0 {
                    return Ok(*(*first));
                }
                typed_list_arg_value(*rest, heap, index - 1)
            }
        }
    }

    pub(super) fn copy_into_frame(self, heap: &mut HeapStore, frame: &mut [RuntimeVal]) -> Result<()> {
        match self {
            Self::Slice(values) => {
                for (slot, value) in frame.iter_mut().zip(values) {
                    *slot = *value;
                }
                Ok(())
            }
            Self::ListHandle(handle) => copy_list_handle_into_slots(handle, heap, frame),
            Self::PrefixedList { first, rest } => {
                frame[0] = *first;
                copy_list_handle_into_slots(rest, heap, &mut frame[1..])
            }
            Self::Prefixed { first, rest } => {
                frame[0] = *first;
                for (slot, value) in frame[1..1 + rest.len()].iter_mut().zip(rest) {
                    *slot = *value;
                }
                Ok(())
            }
        }
    }

    pub(super) fn append_root_values(&self, roots: &mut Vec<RuntimeVal>) {
        match self {
            Self::Slice(values) => roots.extend(values.iter().cloned()),
            Self::ListHandle(handle) => roots.push(RuntimeVal::Obj(*handle)),
            Self::Prefixed { first, rest } => {
                roots.push(*(*first));
                roots.extend(rest.iter().cloned());
            }
            Self::PrefixedList { first, rest } => {
                roots.push(*(*first));
                roots.push(RuntimeVal::Obj(*rest));
            }
        }
    }
}

pub(super) fn call_runtime_native_positional(
    native: &NativeEntry,
    pos: RuntimePositionalArgs<'_>,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
    callee_root: RuntimeVal,
) -> Result<RuntimeVal> {
    let mut roots = vec![callee_root];
    pos.append_root_values(&mut roots);
    if native.function.requires_full_state() {
        let pos = pos.materialize_full_state_native_args(native, &mut state.heap)?;
        let result = call_native_entry(native, pos.as_slice(), state, module, None, ctx);
        return collect_direct_native_garbage_after_result(state, roots, result);
    }

    let RuntimeModuleState {
        heap, globals, stack, ..
    } = state;
    let result = with_runtime_positional_stack_slice(pos, heap, stack, |heap, args| {
        call_native_entry_parts_with_args(native, NativeArgs::new(args), heap, globals, module, None, ctx)
    });
    collect_direct_native_garbage_after_result(state, roots, result)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn call_runtime_native_named_map(
    native: &NativeEntry,
    pos: RuntimePositionalArgs<'_>,
    named: HeapRef,
    named_count: usize,
    state: &mut RuntimeModuleState,
    module: Option<&Module>,
    ctx: Option<&mut VmContext>,
    callee_root: RuntimeVal,
) -> Result<RuntimeVal> {
    let mut roots = vec![callee_root, RuntimeVal::Obj(named)];
    pos.append_root_values(&mut roots);
    if native.function.requires_full_state() {
        let pos = pos.materialize_full_state_native_args(native, &mut state.heap)?;
        let result = call_native_entry_with_args(
            native,
            NativeArgs::new_with_named_map_handle(pos.as_slice(), named, named_count),
            state,
            module,
            None,
            ctx,
        );
        return collect_direct_native_garbage_after_result(state, roots, result);
    }

    let RuntimeModuleState {
        heap, globals, stack, ..
    } = state;
    let result = with_runtime_positional_stack_slice(pos, heap, stack, |heap, args| {
        call_native_entry_parts_with_args(
            native,
            NativeArgs::new_with_named_map_handle(args, named, named_count),
            heap,
            globals,
            module,
            None,
            ctx,
        )
    });
    collect_direct_native_garbage_after_result(state, roots, result)
}

pub(super) fn collect_direct_native_garbage_after_result(
    state: &mut RuntimeModuleState,
    mut roots: Vec<RuntimeVal>,
    result: Result<RuntimeVal>,
) -> Result<RuntimeVal> {
    if let Ok(value) = &result {
        roots.push(*value);
    }
    if state.heap.should_collect() {
        state.collect_garbage(roots.iter());
    }
    result
}

pub(super) fn with_runtime_positional_stack_slice<R>(
    pos: RuntimePositionalArgs<'_>,
    heap: &mut HeapStore,
    stack: &mut Vec<RuntimeVal>,
    f: impl FnOnce(&mut HeapStore, &[RuntimeVal]) -> Result<R>,
) -> Result<R> {
    match pos {
        RuntimePositionalArgs::Slice(values) => f(heap, values),
        RuntimePositionalArgs::ListHandle(handle) => {
            let len = typed_list_arg_len(handle, heap)?;
            let start = stack.len();
            stack.resize(start + len, RuntimeVal::Nil);
            copy_list_handle_into_slots(handle, heap, &mut stack[start..start + len])?;
            let result = f(heap, &stack[start..start + len]);
            stack.truncate(start);
            result
        }
        RuntimePositionalArgs::Prefixed { first, rest } => {
            let len = rest.len() + 1;
            let start = stack.len();
            stack.resize(start + len, RuntimeVal::Nil);
            stack[start] = *first;
            for (slot, value) in stack[start + 1..start + len].iter_mut().zip(rest) {
                *slot = *value;
            }
            let result = f(heap, &stack[start..start + len]);
            stack.truncate(start);
            result
        }
        RuntimePositionalArgs::PrefixedList { first, rest } => {
            let rest_len = typed_list_arg_len(rest, heap)?;
            let len = rest_len + 1;
            let start = stack.len();
            stack.resize(start + len, RuntimeVal::Nil);
            stack[start] = *first;
            copy_list_handle_into_slots(rest, heap, &mut stack[start + 1..start + len])?;
            let result = f(heap, &stack[start..start + len]);
            stack.truncate(start);
            result
        }
    }
}
