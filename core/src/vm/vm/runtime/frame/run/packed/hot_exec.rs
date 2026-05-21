use super::super::helpers::assign_reg_with_metrics;
use super::*;
use crate::vm::{self, VmCallMetric, VmContainerMetric};

#[inline]
fn packed_string_int_key_arcstr(func: &Function, regs: &[Val], pc: usize) -> Option<ArcStr> {
    let source_pc = packed_instr_pc(func, pc);
    let fact = func
        .analysis
        .as_ref()
        .and_then(|analysis| analysis.perf.known_key(source_pc))
        .and_then(|fact| fact.string_int)?;
    let prefix = func.consts.get(fact.prefix_key as usize)?.as_str()?;
    let Val::Int(suffix) = regs.get(fact.suffix_reg as usize)? else {
        return None;
    };
    Some(Val::cached_str_int_key(prefix, *suffix))
}

#[inline(always)]
#[allow(clippy::too_many_arguments)]
pub(super) fn exec_hot_slot(
    entry: &PackedHotSlot,
    regs: &mut [Val],
    func: &Function,
    ctx: &mut VmContext,
    frame_captures: &Option<Arc<ClosureCapture>>,
    frame_capture_specs: &Option<Arc<Vec<CaptureSpec>>>,
    access_ic: &mut [Option<AccessIc>],
    index_ic: &mut [Option<IndexIc>],
    global_ic: &mut [Option<GlobalEntry>],
    call_ic: &mut [Option<CallIc>],
    for_range_ic: &mut [Option<ForRangeState>],
    pc: usize,
    frame_base: usize,
    region_plan: Option<&RegionPlan>,
    region_allocator_ptr: *const RegionAllocator,
    collect_metrics: bool,
) -> Result<Option<usize>> {
    let record_branch = |typed| collect_metrics.then(|| vm::record_branch_op_known_enabled(typed));
    let record_call = |kind| collect_metrics.then(|| vm::record_call_op_known_enabled(kind));
    let record_container = |kind| collect_metrics.then(|| vm::record_container_op_known_enabled(kind));
    let result = match &entry.kind {
        PackedHotKind::Nop => None,
        PackedHotKind::Move { dst, src } => {
            assign_packed_move_with_metrics(regs, func, pc, *dst, *src, collect_metrics);
            None
        }
        PackedHotKind::LoadK { dst, kidx } => {
            assign_packed_const_with_metrics(regs, func, *dst, *kidx, collect_metrics);
            None
        }
        PackedHotKind::LoadLocal { dst, idx } => {
            assign_packed_local_load_with_metrics(regs, func, pc, *dst, *idx, collect_metrics);
            None
        }
        PackedHotKind::StoreLocal { idx, src } => {
            assign_packed_local_store_with_metrics(regs, func, pc, *idx, *src, collect_metrics);
            None
        }
        PackedHotKind::LoadGlobal { dst, name_k } => {
            let name_val = &func.consts[*name_k as usize];
            let out = load_global_for_register(ctx, global_ic, pc, name_val, collect_metrics);
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::DefineGlobal { name_k, src } => {
            if let Some(s) = func.consts[*name_k as usize].as_str() {
                ctx.set(
                    s.to_string(),
                    copy_value_for_register_with_metrics(&regs[*src as usize], collect_metrics),
                );
            }
            None
        }
        PackedHotKind::LoadCapture { dst, idx } => {
            closure::run_load_capture(
                regs,
                ctx,
                frame_captures,
                frame_capture_specs,
                *dst,
                *idx,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::Access { dst, base, field } => {
            exec_access_hot(regs, access_ic, pc, *dst, *base, *field, collect_metrics);
            None
        }
        PackedHotKind::ListIndex { dst, base, index } => {
            record_container(VmContainerMetric::List);
            exec_list_index_hot(regs, *dst, *base, *index, collect_metrics);
            None
        }
        PackedHotKind::StrIndex { dst, base, index } => {
            record_container(VmContainerMetric::String);
            exec_str_index_hot(regs, *dst, *base, *index, collect_metrics);
            None
        }
        PackedHotKind::AccessIntArith {
            access_dst,
            base,
            field,
            write_access_dst,
            arith_op,
            arith_dst,
            arith_a,
            arith_b,
        } => {
            exec_access_int_arith_hot(
                regs,
                func,
                access_ic,
                pc,
                *access_dst,
                *base,
                *field,
                *write_access_dst,
                *arith_op,
                *arith_dst,
                *arith_a,
                *arith_b,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::AccessK { dst, base, key } => {
            let key_val = &func.consts[*key as usize];
            let result = if let Some(key_str) = key_val.as_str() {
                let (hit_value, object_ptr) = match &regs[*base as usize] {
                    Val::Map(map) => (
                        Some(
                            Val::map_get_str(map, key_str)
                                .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                                .unwrap_or(Val::Nil),
                        ),
                        None,
                    ),
                    Val::Object(object) => {
                        let fields = &object.fields;
                        let object_ptr = Arc::as_ptr(fields) as usize;
                        let hit = match access_ic[pc].as_mut() {
                            Some(AccessIc::ObjectStr(slots)) => Vm::lookup_promote(slots, |entry| {
                                entry.obj_ptr == object_ptr && entry.key.as_str() == key_str
                            })
                            .map(|entry| copy_value_for_register_with_metrics(&entry.value, collect_metrics)),
                            _ => None,
                        };
                        (hit, Some(object_ptr))
                    }
                    _ => (None, None),
                };
                if let Some(value) = hit_value {
                    value
                } else {
                    let value = regs[*base as usize]
                        .access_with_metrics(key_val, collect_metrics)
                        .unwrap_or(Val::Nil);
                    if let Some(object_ptr) = object_ptr {
                        Vm::update_object_ic(access_ic, pc, object_ptr, key_str, &value, collect_metrics);
                    }
                    value
                }
            } else {
                Val::Nil
            };
            assign_reg_with_metrics(regs, *dst as usize, result, collect_metrics);
            None
        }
        PackedHotKind::ListLen { dst, src } => {
            record_container(VmContainerMetric::List);
            let out = match &regs[*src as usize] {
                Val::List(list) => Val::Int(list.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::MapLen { dst, src } => {
            record_container(VmContainerMetric::Map);
            let out = match &regs[*src as usize] {
                Val::Map(map) => Val::Int(map.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::StrLen { dst, src } => {
            record_container(VmContainerMetric::String);
            let out = match &regs[*src as usize] {
                Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
                Val::Str(value) => Val::Int(value.len() as i64),
                _ => Val::Int(0),
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::Len { dst, src } => {
            record_container(VmContainerMetric::Generic);
            exec_len(regs, *dst, *src, collect_metrics);
            None
        }
        PackedHotKind::Index { dst, base, idx } => {
            record_container(VmContainerMetric::Generic);
            exec_index(regs, index_ic, pc, *dst, *base, *idx, collect_metrics);
            None
        }
        PackedHotKind::MapGetInterned { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::MapGetInternedCmpJmp {
            dst,
            map,
            key,
            op,
            rhs,
            jump_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            let lhs = &regs[*dst as usize];
            let rhs = rk_read(regs, &func.consts, *rhs);
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                _ => unreachable!("map-get compare fusion only supports equality"),
            };
            if cmp { None } else { Some(*jump_pc) }
        }
        PackedHotKind::MapGetInternedUpsertAdd {
            get_dst,
            cmp_dst,
            map,
            key,
            default,
            default_load,
            add_dst,
            add_rhs,
            write_temps,
        } => {
            record_container(VmContainerMetric::Map);
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key = func.consts[*key as usize]
                .string_key_arcstr()
                .ok_or_else(|| anyhow!("MapSetInterned key must be a String"))?;
            let lookup_key = Some(key.clone());
            exec_map_upsert_add(
                regs,
                func,
                *get_dst,
                *cmp_dst,
                *map,
                lookup_key,
                key,
                *default,
                *default_load,
                *add_dst,
                *add_rhs,
                *write_temps,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::MapGetDynamic { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key_arc = regs[*key as usize]
                .as_str()
                .is_none()
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let lookup_key = key_arc
                .as_ref()
                .map(|key| key.as_str())
                .or_else(|| regs[*key as usize].as_str());
            let out = match (&regs[*map as usize], lookup_key) {
                (Val::Map(map), Some(key)) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::MapGetDynamicCmpJmp {
            dst,
            map,
            key,
            op,
            rhs,
            jump_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key_arc = regs[*key as usize]
                .as_str()
                .is_none()
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let lookup_key = key_arc
                .as_ref()
                .map(|key| key.as_str())
                .or_else(|| regs[*key as usize].as_str());
            let out = match (&regs[*map as usize], lookup_key) {
                (Val::Map(map), Some(key)) => Val::map_get_str(map, key)
                    .map(|value| copy_value_for_register_with_metrics(value, collect_metrics))
                    .unwrap_or(Val::Nil),
                _ => Val::Nil,
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            let lhs = &regs[*dst as usize];
            let rhs = rk_read(regs, &func.consts, *rhs);
            let cmp = match op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                _ => unreachable!("map-get compare fusion only supports equality"),
            };
            if cmp { None } else { Some(*jump_pc) }
        }
        PackedHotKind::MapGetDynamicUpsertAdd {
            get_dst,
            cmp_dst,
            map,
            key,
            default,
            default_load,
            add_dst,
            add_rhs,
            write_temps,
        } => {
            record_container(VmContainerMetric::Map);
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key_reg = *key;
            let key_arc = regs[key_reg as usize]
                .primitive_key_arcstr()
                .is_none()
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let lookup_key = regs[key_reg as usize]
                .as_str()
                .map(ArcStr::from)
                .or_else(|| key_arc.clone());
            let key = regs[key_reg as usize]
                .primitive_key_arcstr()
                .or(key_arc)
                .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", regs[key_reg as usize]))?;
            exec_map_upsert_add(
                regs,
                func,
                *get_dst,
                *cmp_dst,
                *map,
                lookup_key,
                key,
                *default,
                *default_load,
                *add_dst,
                *add_rhs,
                *write_temps,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::MapHas { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key_arc = regs[*key as usize]
                .as_str()
                .is_none()
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let lookup_key = key_arc
                .as_ref()
                .map(|key| key.as_str())
                .or_else(|| regs[*key as usize].as_str());
            let out = match (&regs[*map as usize], lookup_key) {
                (Val::Map(map), Some(key)) => Val::Bool(Val::map_contains_str(map, key)),
                (Val::Map(_), None) => Val::Bool(false),
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::MapHasIncJmp {
            dst,
            map,
            key,
            inc_r,
            inc_imm,
            true_pc,
            false_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key_arc = regs[*key as usize]
                .as_str()
                .is_none()
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let lookup_key = key_arc
                .as_ref()
                .map(|key| key.as_str())
                .or_else(|| regs[*key as usize].as_str());
            let contains = match (&regs[*map as usize], lookup_key) {
                (Val::Map(map), Some(key)) => Val::map_contains_str(map, key),
                (Val::Map(_), None) => false,
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg_with_metrics(regs, *dst as usize, Val::Bool(contains), collect_metrics);
            if contains {
                record_branch(true);
                if let Val::Int(x) = regs[*inc_r as usize] {
                    assign_reg_with_metrics(
                        regs,
                        *inc_r as usize,
                        Val::Int(x.wrapping_add(*inc_imm as i64)),
                        collect_metrics,
                    );
                }
                Some(*true_pc)
            } else {
                Some(*false_pc)
            }
        }
        PackedHotKind::MapHasK { dst, map, key } => {
            record_container(VmContainerMetric::Map);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let out = match &regs[*map as usize] {
                Val::Map(map) => Val::Bool(Val::map_contains_str(map, key)),
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::MapHasKIncJmp {
            dst,
            map,
            key,
            inc_r,
            inc_imm,
            true_pc,
            false_pc,
        } => {
            record_container(VmContainerMetric::Map);
            record_branch(false);
            let key = func.consts[*key as usize].as_str().unwrap_or("");
            let contains = match &regs[*map as usize] {
                Val::Map(map) => Val::map_contains_str(map, key),
                _ => return Err(anyhow!("has() first argument must be a map")),
            };
            assign_reg_with_metrics(regs, *dst as usize, Val::Bool(contains), collect_metrics);
            if contains {
                record_branch(true);
                if let Val::Int(x) = regs[*inc_r as usize] {
                    assign_reg_with_metrics(
                        regs,
                        *inc_r as usize,
                        Val::Int(x.wrapping_add(*inc_imm as i64)),
                        collect_metrics,
                    );
                }
                Some(*true_pc)
            } else {
                Some(*false_pc)
            }
        }
        PackedHotKind::MapSetInterned { map, key, val } => {
            record_container(VmContainerMetric::Map);
            exec_map_set_interned(func, regs, *map, *key, *val, collect_metrics)?;
            None
        }
        PackedHotKind::MapSetInternedMove { map, key, val } => {
            record_container(VmContainerMetric::Map);
            exec_map_set_interned_move(func, regs, *map, *key, *val, collect_metrics)?;
            None
        }
        PackedHotKind::StrConcatKnownCap { dst, a, b } => {
            record_container(VmContainerMetric::String);
            let a_val = &regs[*a as usize];
            let b_val = &regs[*b as usize];
            let out = match (a_val.as_str(), b_val.as_str()) {
                (Some(a_str), Some(b_str)) => Val::concat_strings(a_str, b_str),
                _ => BinOp::Add.eval_vals_with_metrics(a_val, b_val, collect_metrics)?,
            };
            assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            None
        }
        PackedHotKind::StrConcatToStr { dst, lhs, src } => {
            record_container(VmContainerMetric::String);
            let source_pc = packed_instr_pc(func, pc);
            if !func
                .analysis
                .as_ref()
                .is_some_and(|analysis| analysis.perf.is_dead_write(source_pc))
            {
                let lhs_val = &regs[*lhs as usize];
                let out = if let Some(lhs_str) = lhs_val.as_str()
                    && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[*src as usize])
                {
                    value
                } else {
                    let rhs = Val::to_str_value(&regs[*src as usize]);
                    BinOp::Add.eval_vals_with_metrics(lhs_val, &rhs, collect_metrics)?
                };
                assign_reg_with_metrics(regs, *dst as usize, out, collect_metrics);
            } else {
                assign_reg_with_metrics(regs, *dst as usize, Val::Nil, collect_metrics);
            }
            None
        }
        PackedHotKind::IntArith { op, dst, a, b } => {
            exec_int_arith(regs, func, *op, *dst, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::IntArithAddIntImm {
            arith_op,
            arith_dst,
            arith_a,
            arith_b,
            add_dst,
            add_imm,
        } => {
            exec_int_arith(regs, func, *arith_op, *arith_dst, *arith_a, *arith_b, collect_metrics)?;
            let src_idx = *arith_dst as usize;
            if let Val::Int(x) = regs[src_idx] {
                assign_reg_with_metrics(regs, *add_dst as usize, Val::Int(x + *add_imm as i64), collect_metrics);
            } else {
                int_binop_imm(
                    regs,
                    &func.consts,
                    *add_dst,
                    *arith_dst,
                    *add_imm,
                    |x, y| x + y,
                    BinOp::Add,
                    collect_metrics,
                )?;
            }
            None
        }
        PackedHotKind::IntArithCmpIntJmp {
            arith_op,
            arith_dst,
            arith_a,
            arith_b,
            cmp_op,
            cmp_a,
            cmp_b,
            jump_pc,
        } => {
            let arith_value = match (
                rk_read(regs, &func.consts, *arith_a),
                rk_read(regs, &func.consts, *arith_b),
            ) {
                (Val::Int(lhs), Val::Int(rhs)) => match arith_op {
                    PackedArithOp::Add => Val::Int(lhs + rhs),
                    PackedArithOp::Sub => Val::Int(lhs - rhs),
                    PackedArithOp::Mul => Val::Int(lhs * rhs),
                    PackedArithOp::Mod => Val::Int(lhs % rhs),
                    PackedArithOp::Div => {
                        BinOp::Div.eval_vals_with_metrics(&Val::Int(*lhs), &Val::Int(*rhs), collect_metrics)?
                    }
                },
                (lhs, rhs) => match arith_op {
                    PackedArithOp::Add => BinOp::Add.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
                    PackedArithOp::Sub => BinOp::Sub.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
                    PackedArithOp::Mul => BinOp::Mul.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
                    PackedArithOp::Mod => BinOp::Mod.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
                    PackedArithOp::Div => BinOp::Div.eval_vals_with_metrics(lhs, rhs, collect_metrics)?,
                },
            };
            // Extract integer from arith result before moving it into the register.
            // When arith_dst matches a compare operand, reuse this extracted value
            // to avoid re-reading and type-checking the register.
            let arith_int = if let Val::Int(n) = arith_value { Some(n) } else { None };
            assign_reg_with_metrics(regs, *arith_dst as usize, arith_value, collect_metrics);
            record_branch(true);
            let cmp_a_idx = *cmp_a as usize;
            let cmp_b_idx = *cmp_b as usize;
            let arith_dst_idx = *arith_dst as usize;
            let (lhs, rhs) = if let Some(n) = arith_int {
                if cmp_a_idx == arith_dst_idx {
                    let Val::Int(rhs) = regs[cmp_b_idx] else {
                        return Err(anyhow!("CmpI expects integer register"));
                    };
                    (n, rhs)
                } else if cmp_b_idx == arith_dst_idx {
                    let Val::Int(lhs) = regs[cmp_a_idx] else {
                        return Err(anyhow!("CmpI expects integer register"));
                    };
                    (lhs, n)
                } else {
                    let (Val::Int(lhs), Val::Int(rhs)) = (&regs[cmp_a_idx], &regs[cmp_b_idx]) else {
                        return Err(anyhow!("CmpI expects integer registers"));
                    };
                    (*lhs, *rhs)
                }
            } else {
                let (Val::Int(lhs), Val::Int(rhs)) = (&regs[cmp_a_idx], &regs[cmp_b_idx]) else {
                    return Err(anyhow!("CmpI expects integer registers"));
                };
                (*lhs, *rhs)
            };
            let cmp = match cmp_op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            if !cmp { Some(*jump_pc) } else { None }
        }
        PackedHotKind::IntArithCmpIntMove {
            arith_op,
            arith_dst,
            arith_a,
            arith_b,
            cmp_op,
            cmp_a,
            cmp_b,
            move_dst,
            move_src,
        } => {
            exec_int_arith(regs, func, *arith_op, *arith_dst, *arith_a, *arith_b, collect_metrics)?;
            record_branch(true);
            let (Val::Int(lhs), Val::Int(rhs)) = (&regs[*cmp_a as usize], &regs[*cmp_b as usize]) else {
                return Err(anyhow!("CmpI expects integer registers"));
            };
            let cmp = match cmp_op {
                PackedCmpOp::Eq => lhs == rhs,
                PackedCmpOp::Ne => lhs != rhs,
                PackedCmpOp::Lt => lhs < rhs,
                PackedCmpOp::Le => lhs <= rhs,
                PackedCmpOp::Gt => lhs > rhs,
                PackedCmpOp::Ge => lhs >= rhs,
            };
            if cmp {
                assign_reg_from_reg_with_metrics(regs, *move_dst as usize, *move_src as usize, collect_metrics);
            }
            None
        }
        PackedHotKind::AddIntFloorDivImm {
            add_dst,
            a,
            b,
            div_dst,
            imm,
        } => {
            exec_int_arith(regs, func, PackedArithOp::Add, *add_dst, *a, *b, collect_metrics)?;
            exec_floor_div_imm(regs, *div_dst, *add_dst, *imm, collect_metrics);
            None
        }
        PackedHotKind::MulIntFloorDivImm {
            mul_dst,
            a,
            b,
            div_dst,
            imm,
        } => {
            exec_int_arith(regs, func, PackedArithOp::Mul, *mul_dst, *a, *b, collect_metrics)?;
            exec_floor_div_imm(regs, *div_dst, *mul_dst, *imm, collect_metrics);
            None
        }
        PackedHotKind::MulIntAddInt {
            mul_dst,
            mul_a,
            mul_b,
            add_dst,
            add_a,
            add_b,
        } => {
            exec_int_arith(
                regs,
                func,
                PackedArithOp::Mul,
                *mul_dst,
                *mul_a,
                *mul_b,
                collect_metrics,
            )?;
            exec_int_arith(
                regs,
                func,
                PackedArithOp::Add,
                *add_dst,
                *add_a,
                *add_b,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::MulIntAddIntModInt {
            mul_dst,
            mul_a,
            mul_b,
            add_dst,
            add_a,
            add_b,
            mod_dst,
            mod_rhs,
        } => {
            exec_mul_add_mod_int_hot(
                regs,
                func,
                *mul_dst,
                *mul_a,
                *mul_b,
                *add_dst,
                *add_a,
                *add_b,
                *mod_dst,
                *mod_rhs,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::MulIntMulIntAddInt {
            first_dst,
            first_a,
            first_b,
            second_dst,
            second_a,
            second_b,
            add_dst,
            add_a,
            add_b,
        } => {
            exec_int_arith(
                regs,
                func,
                PackedArithOp::Mul,
                *first_dst,
                *first_a,
                *first_b,
                collect_metrics,
            )?;
            exec_int_arith(
                regs,
                func,
                PackedArithOp::Mul,
                *second_dst,
                *second_a,
                *second_b,
                collect_metrics,
            )?;
            exec_int_arith(
                regs,
                func,
                PackedArithOp::Add,
                *add_dst,
                *add_a,
                *add_b,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::FloatArith { op, dst, a, b } => {
            exec_float_arith(regs, func, *op, *dst, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::Floor { dst, src } => {
            exec_floor(regs, *dst, *src, collect_metrics);
            None
        }
        PackedHotKind::FloorDivImm { dst, src, imm } => {
            exec_floor_div_imm(regs, *dst, *src, *imm, collect_metrics);
            None
        }
        PackedHotKind::ToBool { dst, src } => {
            let truthy = !matches!(regs[*src as usize], Val::Nil | Val::Bool(false));
            assign_reg_with_metrics(regs, *dst as usize, Val::Bool(truthy), collect_metrics);
            None
        }
        PackedHotKind::StartsWithK { dst, src, key } => {
            record_container(VmContainerMetric::String);
            exec_starts_with_k(regs, func, *dst, *src, *key, collect_metrics);
            None
        }
        PackedHotKind::StartsWithKJmp { src, key, ofs } => {
            record_container(VmContainerMetric::String);
            record_branch(true);
            (!starts_with_k_bool(regs, func, *src, *key)).then_some(((pc as isize) + (*ofs as isize)) as usize)
        }
        PackedHotKind::ContainsK { dst, src, key } => {
            record_container(VmContainerMetric::String);
            exec_contains_k(regs, func, *dst, *src, *key, collect_metrics);
            None
        }
        PackedHotKind::ContainsKJmp { src, key, ofs } => {
            record_container(VmContainerMetric::String);
            record_branch(true);
            (!contains_k_bool(regs, func, *src, *key)).then_some(((pc as isize) + (*ofs as isize)) as usize)
        }
        PackedHotKind::ToIter { dst, src } => {
            record_container(VmContainerMetric::Generic);
            exec_to_iter(regs, *dst, *src, region_plan, region_allocator_ptr, collect_metrics);
            None
        }
        PackedHotKind::BuildList { dst, base, len } => {
            record_container(VmContainerMetric::List);
            let start = *base as usize;
            let len = *len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let list_val = allocator.with_val_buffer(len, |scratch| {
                    scratch.extend(
                        (0..len).map(|i| copy_value_for_register_with_metrics(&regs[start + i], collect_metrics)),
                    );
                    let data = scratch.split_off(0);
                    Val::List(data.into())
                });
                assign_reg_with_metrics(regs, *dst as usize, list_val, collect_metrics);
            } else {
                let mut values = Vec::with_capacity(len);
                for i in 0..len {
                    values.push(copy_value_for_register_with_metrics(&regs[start + i], collect_metrics));
                }
                assign_reg_with_metrics(regs, *dst as usize, Val::List(values.into()), collect_metrics);
            }
            None
        }
        PackedHotKind::BuildMap { dst, base, len } => {
            record_container(VmContainerMetric::Map);
            let start = *base as usize;
            let len = *len as usize;
            let use_thread_local = region_plan
                .as_ref()
                .map(|plan| plan.region_for(*dst as usize) == AllocationRegion::ThreadLocal)
                .unwrap_or(false);
            if use_thread_local {
                let allocator = region_allocator(region_allocator_ptr);
                let map_val = allocator.with_map_entries(len, |entries| {
                    for i in 0..len {
                        let key_val = &regs[start + 2 * i];
                        let value = copy_value_for_register_with_metrics(&regs[start + 2 * i + 1], collect_metrics);
                        let key_arc = key_val
                            .primitive_key_arcstr()
                            .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                        entries.push((key_arc, value));
                    }
                    let mut map = fast_hash_map_with_capacity(entries.len());
                    for (key, value) in entries.drain(..) {
                        Val::map_insert_arcstr(&mut map, key, value);
                    }
                    Ok::<Val, anyhow::Error>(Val::Map(Arc::new(map)))
                })?;
                assign_reg_with_metrics(regs, *dst as usize, map_val, collect_metrics);
            } else {
                let mut map: FastHashMap<ArcStr, Val> = fast_hash_map_with_capacity(len);
                for i in 0..len {
                    let key_val = &regs[start + 2 * i];
                    let value = copy_value_for_register_with_metrics(&regs[start + 2 * i + 1], collect_metrics);
                    let key_arc = key_val
                        .primitive_key_arcstr()
                        .ok_or_else(|| anyhow!("Map key must be a primitive type, got: {:?}", key_val))?;
                    Val::map_insert_arcstr(&mut map, key_arc, value);
                }
                assign_reg_with_metrics(regs, *dst as usize, Val::Map(Arc::new(map)), collect_metrics);
            }
            None
        }
        PackedHotKind::ForRangePrep {
            idx,
            limit,
            step,
            inclusive,
            explicit,
        } => {
            let idx_reg = *idx as usize;
            let limit_reg = *limit as usize;
            let step_reg = *step as usize;
            let (i0, ilim) = match (&regs[idx_reg], &regs[limit_reg]) {
                (Val::Int(a0), Val::Int(b0)) => (*a0, *b0),
                _ => {
                    return Err(anyhow!(
                        "For-range requires integer bounds, got idx={:?}, limit={:?}",
                        regs[idx_reg],
                        regs[limit_reg]
                    ));
                }
            };
            let step_val = if !*explicit {
                let step_val = if i0 <= ilim { 1 } else { -1 };
                assign_reg_with_metrics(regs, step_reg, Val::Int(step_val), collect_metrics);
                step_val
            } else {
                match &regs[step_reg] {
                    Val::Int(0) => return Err(anyhow!("For-range step cannot be zero")),
                    Val::Int(v) => *v,
                    other => return Err(anyhow!("For-range step must be Int when explicit, got {:?}", other)),
                }
            };
            if step_val == 0 {
                return Err(anyhow!("For-range step cannot be zero"));
            }
            if let Some(slot) = for_range_ic.get_mut(entry.next_pc) {
                *slot = Some(ForRangeState::new(i0, ilim, step_val, *inclusive));
            }
            None
        }
        PackedHotKind::ForRangeLoop {
            idx,
            write_idx,
            ofs,
            fusion_back_pc,
        } => {
            // Direct indexing: hot slots are only built for valid PCs after ForRangePrep
            // has populated for_range_ic. Bounds-check is redundant (pc < code32.len()
            // and for_range_ic is pre-allocated to match). The Option unwrap is safe
            // because ForRangeLoop slots are only built when state exists.
            let state_entry = match for_range_ic.get_mut(pc) {
                Some(Some(state)) => state,
                _ => return Err(anyhow!("For-range state missing at pc {}", pc)),
            };
            let keep_going = if state_entry.positive {
                if state_entry.inclusive {
                    state_entry.current <= state_entry.limit
                } else {
                    state_entry.current < state_entry.limit
                }
            } else if state_entry.inclusive {
                state_entry.current >= state_entry.limit
            } else {
                state_entry.current > state_entry.limit
            };
            if keep_going {
                let current = state_entry.current;
                if *write_idx {
                    assign_reg_with_metrics(regs, *idx as usize, Val::Int(current), collect_metrics);
                }
                state_entry.current += state_entry.step;
                // Use pre-computed fusion target if available
                if let Some(back_pc) = fusion_back_pc {
                    return Ok(Some(*back_pc));
                }
                None
            } else {
                // Write final counter value on exit for correct post-loop counter value.
                if *write_idx {
                    assign_reg_with_metrics(regs, *idx as usize, Val::Int(state_entry.current), collect_metrics);
                }
                for_range_ic[pc] = None;
                Some(((pc as isize) + (*ofs as isize)) as usize)
            }
        }
        PackedHotKind::ForRangeStep { back_ofs, tail } => {
            let guard_pc = ((pc as isize) + (*back_ofs as isize)) as usize;
            match tail {
                Some(tail) => Some(advance_for_range_tail(
                    regs,
                    for_range_ic,
                    tail.guard_pc,
                    tail.body_pc,
                    tail.exit_pc,
                    tail.idx,
                    tail.write_idx,
                    collect_metrics,
                )?),
                None => Some(guard_pc),
            }
        }
        PackedHotKind::ToStr { dst, src } => {
            let s = Val::to_str_value(&regs[*src as usize]);
            assign_reg_with_metrics(regs, *dst as usize, s, collect_metrics);
            None
        }
        PackedHotKind::ToStrAddRhs {
            tmp,
            src,
            out,
            lhs,
            add_pc,
        } => {
            let lhs_val = rk_read(regs, &func.consts, *lhs);
            if let Some(lhs_str) = lhs_val.as_str()
                && let Some(value) = Val::concat_str_tostr_rhs(lhs_str, &regs[*src as usize])
            {
                assign_reg_with_metrics(regs, *out as usize, value, collect_metrics);
                None
            } else {
                let s = Val::to_str_value(&regs[*src as usize]);
                assign_reg_with_metrics(regs, *tmp as usize, s, collect_metrics);
                Some(*add_pc)
            }
        }
        PackedHotKind::MakeClosure { dst, proto } => {
            let clo = make_closure_value(func, *proto, ctx, regs, frame_base, collect_metrics)?;
            assign_reg_with_metrics(regs, *dst as usize, clo, collect_metrics);
            None
        }
        PackedHotKind::ArithAddIntImm {
            op,
            arith_dst,
            a,
            b,
            add_dst,
            add_imm,
        } => {
            exec_arith_add_int_imm(regs, func, *op, *arith_dst, *a, *b, *add_dst, *add_imm, collect_metrics)?;
            None
        }
        PackedHotKind::Arith { op, dst, a, b } => {
            exec_arith_hot(regs, func, *op, *dst, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::Cmp { op, dst, a, b } => {
            hot_compare::exec_cmp_hot(regs, func, *op, *dst, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::CmpInt { op, dst, a, b } => {
            hot_compare::exec_cmp_int(regs, *op, *dst, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::CmpIntJmp { op, a, b, ofs } => {
            record_branch(true);
            hot_compare::exec_cmp_int_jmp(regs, *op, *a, *b, pc, *ofs)?
        }
        PackedHotKind::CMoveInt { op, dst, src, a, b } => {
            hot_compare::exec_cmove_int(regs, *op, *dst, *src, *a, *b, collect_metrics)?;
            None
        }
        PackedHotKind::CmpIntMove {
            op,
            a,
            b,
            dst,
            src,
            ofs,
        } => {
            record_branch(true);
            hot_compare::exec_cmp_int_move(regs, *op, *a, *b, *dst, *src, pc, *ofs, collect_metrics)?
        }
        PackedHotKind::CmpIntAddIntImm {
            op,
            a,
            b,
            dst,
            src,
            imm,
            ofs,
        } => {
            record_branch(true);
            hot_compare::exec_cmp_int_add_int_imm(regs, func, *op, *a, *b, *dst, *src, *imm, pc, *ofs, collect_metrics)?
        }
        PackedHotKind::CmpIntSubAccessSub {
            op,
            a,
            b,
            first_dst,
            first_a,
            first_b,
            access_pc,
            access_dst,
            access_base,
            access_field,
            final_dst,
            final_a,
            final_b,
            ofs,
        } => {
            record_branch(true);
            hot_compare::exec_cmp_int_sub_access_sub(
                regs,
                func,
                access_ic,
                *op,
                *a,
                *b,
                *first_dst,
                *first_a,
                *first_b,
                *access_pc,
                *access_dst,
                *access_base,
                *access_field,
                *final_dst,
                *final_a,
                *final_b,
                pc,
                *ofs,
                collect_metrics,
            )?
        }
        PackedHotKind::CmpJmp { op, a, b, ofs } => {
            record_branch(false);
            hot_compare::exec_cmp_jmp(regs, func, *op, *a, *b, pc, *ofs)?
        }
        PackedHotKind::Jmp { ofs } => {
            record_branch(false);
            Some(((pc as isize) + (*ofs as isize)) as usize)
        }
        PackedHotKind::JmpFalse { r, ofs } => {
            record_branch(false);
            if matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::JmpFalseSet { r, dst, ofs } => {
            record_branch(false);
            if matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                assign_reg_with_metrics(regs, *dst as usize, Val::Bool(false), collect_metrics);
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::JmpTrueSet { r, dst, ofs } => {
            record_branch(false);
            if !matches!(regs[*r as usize], Val::Nil | Val::Bool(false)) {
                assign_reg_with_metrics(regs, *dst as usize, Val::Bool(true), collect_metrics);
                Some(((pc as isize) + (*ofs as isize)) as usize)
            } else {
                None
            }
        }
        PackedHotKind::Ret { .. } => unreachable!("Ret is handled directly by run_packed_code"),
        PackedHotKind::ListPush { list, val } => {
            record_container(VmContainerMetric::List);
            let pushed_val = copy_value_for_register_with_metrics(&regs[*val as usize], collect_metrics);
            match &mut regs[*list as usize] {
                Val::List(arc) => {
                    push_list_entry(arc, pushed_val);
                }
                _ => return Err(anyhow!("ListPush target is not a List")),
            }
            None
        }
        PackedHotKind::ListPushMove { list, val } => {
            record_container(VmContainerMetric::List);
            let list_idx = *list as usize;
            let val_idx = *val as usize;
            if list_idx == val_idx {
                let pushed_val = copy_value_for_register_with_metrics(&regs[val_idx], collect_metrics);
                match &mut regs[list_idx] {
                    Val::List(arc) => {
                        push_list_entry(arc, pushed_val);
                    }
                    _ => return Err(anyhow!("ListPush target is not a List")),
                }
                return Ok(None);
            }
            if !matches!(regs[list_idx], Val::List(_)) {
                return Err(anyhow!("ListPush target is not a List"));
            }
            let pushed_val = take_register_value(regs, val_idx);
            match &mut regs[list_idx] {
                Val::List(arc) => {
                    push_list_entry(arc, pushed_val);
                }
                _ => unreachable!("ListPush target was checked before moving value"),
            }
            None
        }
        PackedHotKind::MapSet { map, key, val } => {
            record_container(VmContainerMetric::Map);
            let key_arc = regs[*key as usize]
                .dynamic_string_key_arcstr()
                .or_else(|| packed_string_int_key_arcstr(func, regs, pc))
                .ok_or_else(|| anyhow!("MapSet key must be a String"))?;
            let pushed_val = copy_value_for_register_with_metrics(&regs[*val as usize], collect_metrics);
            match &mut regs[*map as usize] {
                Val::Map(arc) => {
                    insert_map_entry(arc, key_arc, pushed_val);
                }
                _ => return Err(anyhow!("MapSet target is not a Map")),
            }
            None
        }
        PackedHotKind::MapSetMove { map, key, val } => {
            record_container(VmContainerMetric::Map);
            let map_idx = *map as usize;
            let key_idx = *key as usize;
            let val_idx = *val as usize;
            if map_idx == key_idx || map_idx == val_idx || key_idx == val_idx {
                let key_arc = regs[key_idx]
                    .dynamic_string_key_arcstr()
                    .or_else(|| packed_string_int_key_arcstr(func, regs, pc))
                    .ok_or_else(|| anyhow!("MapSet key must be a String"))?;
                let pushed_val = copy_value_for_register_with_metrics(&regs[val_idx], collect_metrics);
                match &mut regs[map_idx] {
                    Val::Map(arc) => {
                        insert_map_entry(arc, key_arc, pushed_val);
                    }
                    _ => return Err(anyhow!("MapSet target is not a Map")),
                }
                return Ok(None);
            }
            if !matches!(regs[map_idx], Val::Map(_)) {
                return Err(anyhow!("MapSet target is not a Map"));
            }
            let fact_key_arc = (!matches!(regs[key_idx], Val::Str(_) | Val::ShortStr(_)))
                .then(|| packed_string_int_key_arcstr(func, regs, pc))
                .flatten();
            let key_val = take_register_value(regs, key_idx);
            let key_arc = match key_val.dynamic_string_key_arcstr() {
                Some(key_arc) => key_arc,
                None => match fact_key_arc {
                    Some(key_arc) => key_arc,
                    None => {
                        restore_register_value(regs, key_idx, key_val);
                        return Err(anyhow!("MapSet key must be a String"));
                    }
                },
            };
            let pushed_val = take_register_value(regs, val_idx);
            match &mut regs[map_idx] {
                Val::Map(arc) => {
                    insert_map_entry(arc, key_arc, pushed_val);
                }
                _ => unreachable!("MapSet target was checked before moving key/value"),
            }
            None
        }
        PackedHotKind::CallNativeFast { f, base, argc, retc } => {
            record_call(VmCallMetric::Native);
            let pc_slot = call_ic
                .get_mut(pc)
                .ok_or_else(|| anyhow!("call IC slot out of range for pc {}", pc))?;
            let callable = NativeCallable::from_val(&regs[*f as usize])
                .ok_or_else(|| anyhow!("CallNativeFast target is not a native function"))?;
            let ret_layout = CallReturnLayout::new(*base, *retc);
            invoke_native_callable_with_ic(ctx, regs, pc_slot, callable, *argc, ret_layout, collect_metrics)?;
            None
        }
        PackedHotKind::CallMethod0 { dst, receiver, method } => {
            record_call(VmCallMetric::Method);
            method_ops::run_call_method0(regs, ctx, func, *dst, *receiver, *method, collect_metrics)?;
            None
        }
        PackedHotKind::CallGlobalMethod0 { dst, receiver, method } => {
            record_call(VmCallMetric::Method);
            method_ops::run_call_global_method0(
                regs,
                ctx,
                func,
                global_ic,
                pc,
                *dst,
                *receiver,
                *method,
                collect_metrics,
            )?;
            None
        }
        PackedHotKind::Call { .. } | PackedHotKind::CallClosureExact { .. } | PackedHotKind::CallExact { .. } => {
            unreachable!("call hot slots are handled by run_packed_code")
        }
        PackedHotKind::MoveCall { .. } => unreachable!("move+call hot slots are handled by run_packed_code"),
        PackedHotKind::AddIntImm { dst, src, imm } => {
            let dst_idx = *dst as usize;
            let src_idx = *src as usize;
            if let Val::Int(x) = regs[src_idx] {
                assign_reg_with_metrics(regs, dst_idx, Val::Int(x + *imm as i64), collect_metrics);
            } else {
                int_binop_imm(
                    regs,
                    &func.consts,
                    *dst,
                    *src,
                    *imm,
                    |x, y| x + y,
                    BinOp::Add,
                    collect_metrics,
                )?;
            }
            None
        }
        PackedHotKind::CmpImm { op, dst, src, imm } => {
            hot_compare::exec_cmp_imm(regs, func, *op, *dst, *src, *imm, collect_metrics)?;
            None
        }
        PackedHotKind::CmpImmJmp { op, src, imm, ofs } => {
            record_branch(true);
            hot_compare::exec_cmp_imm_jmp(regs, func, *op, *src, *imm, pc, *ofs)?
        }
        PackedHotKind::CmpImmMulIntAddInt {
            op,
            src,
            imm,
            mul_dst,
            mul_a,
            mul_b,
            add_dst,
            add_a,
            add_b,
            ofs,
        } => {
            record_branch(true);
            hot_compare::exec_cmp_imm_mul_int_add_int(
                regs,
                func,
                *op,
                *src,
                *imm,
                *mul_dst,
                *mul_a,
                *mul_b,
                *add_dst,
                *add_a,
                *add_b,
                pc,
                *ofs,
                collect_metrics,
            )?
        }
        PackedHotKind::CmpLtImmJmp { r, imm, ofs } => {
            record_branch(true);
            hot_compare::exec_cmp_lt_imm_jmp(regs, *r, *imm, pc, *ofs)
        }
        PackedHotKind::CmpLeImmJmp { r, imm, ofs } => {
            record_branch(true);
            hot_compare::exec_cmp_le_imm_jmp(regs, *r, *imm, pc, *ofs)
        }
        PackedHotKind::AddIntImmJmp { r, imm, ofs } => {
            record_branch(true);
            // Fused: r += imm, then jump by ofs.
            if let Val::Int(x) = regs[*r as usize] {
                let result = x.wrapping_add(*imm as i64);
                assign_reg_with_metrics(regs, *r as usize, Val::Int(result), collect_metrics);
            }
            Some(((pc as isize) + (*ofs as isize)) as usize)
        }
    };
    Ok(result)
}
