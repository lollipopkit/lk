use crate::val::Val;
use crate::vm::bytecode::{rk_index, rk_is_const};
use crate::vm::vm::Vm;
use crate::vm::vm::caches::{AccessIc, IndexIc, ListEntry, MapStrEntry, ObjectStrEntry, StrEntry};

use super::helpers::assign_reg_slice;

impl Vm {
    #[inline(always)]
    pub(super) fn promote_or_insert<T, F, Make>(
        slots: &mut [Option<T>; 4],
        mut matches: F,
        make_entry: Make,
    ) -> (bool, &mut T)
    where
        F: FnMut(&T) -> bool,
        Make: FnOnce() -> T,
    {
        if slots[0].as_ref().is_some_and(&mut matches) {
            return (true, slots[0].as_mut().unwrap());
        }
        for idx in 1..slots.len() {
            if slots[idx].as_ref().is_some_and(&mut matches) {
                slots[..=idx].rotate_right(1);
                return (true, slots[0].as_mut().unwrap());
            }
        }
        slots.rotate_right(1);
        slots[0] = Some(make_entry());
        (false, slots[0].as_mut().unwrap())
    }

    #[inline(always)]
    pub(super) fn lookup_promote<T, F>(slots: &mut [Option<T>; 4], mut matches: F) -> Option<&T>
    where
        F: FnMut(&T) -> bool,
    {
        if slots[0].as_ref().is_some_and(&mut matches) {
            return slots[0].as_ref();
        }
        for idx in 1..slots.len() {
            if slots[idx].as_ref().is_some_and(&mut matches) {
                slots[..=idx].rotate_right(1);
                return slots[0].as_ref();
            }
        }
        None
    }

    #[inline(always)]
    pub(super) fn update_list_ic(index_ic: &mut [Option<IndexIc>], pc: usize, base_ptr: usize, idx: i64, value: &Val) {
        match index_ic[pc].as_mut() {
            Some(IndexIc::List(slots)) => {
                let (hit, entry) = Vm::promote_or_insert(
                    slots,
                    |e| e.base_ptr == base_ptr && e.idx == idx,
                    || ListEntry {
                        base_ptr,
                        idx,
                        value: value.clone(),
                    },
                );
                if hit {
                    entry.value = value.clone();
                }
            }
            _ => {
                index_ic[pc] = Some(IndexIc::List([
                    Some(ListEntry {
                        base_ptr,
                        idx,
                        value: value.clone(),
                    }),
                    None,
                    None,
                    None,
                ]));
            }
        }
    }

    #[inline(always)]
    pub(super) fn update_str_ic(index_ic: &mut [Option<IndexIc>], pc: usize, base_ptr: usize, idx: i64, value: &Val) {
        match index_ic[pc].as_mut() {
            Some(IndexIc::Str(slots)) => {
                let (hit, entry) = Vm::promote_or_insert(
                    slots,
                    |e| e.base_ptr == base_ptr && e.idx == idx,
                    || StrEntry {
                        base_ptr,
                        idx,
                        value: value.clone(),
                    },
                );
                if hit {
                    entry.value = value.clone();
                }
            }
            _ => {
                index_ic[pc] = Some(IndexIc::Str([
                    Some(StrEntry {
                        base_ptr,
                        idx,
                        value: value.clone(),
                    }),
                    None,
                    None,
                    None,
                ]));
            }
        }
    }

    #[inline(always)]
    pub(super) fn update_map_ic(
        access_ic: &mut [Option<AccessIc>],
        pc: usize,
        map_ptr: usize,
        key_ptr: usize,
        value: &Val,
    ) {
        match access_ic[pc].as_mut() {
            Some(AccessIc::MapStr(slots)) => {
                let (hit, entry) = Vm::promote_or_insert(
                    slots,
                    |e| e.map_ptr == map_ptr && e.key_ptr == key_ptr,
                    || MapStrEntry {
                        map_ptr,
                        key_ptr,
                        value: value.clone(),
                    },
                );
                if hit {
                    entry.value = value.clone();
                }
            }
            _ => {
                access_ic[pc] = Some(AccessIc::MapStr([
                    Some(MapStrEntry {
                        map_ptr,
                        key_ptr,
                        value: value.clone(),
                    }),
                    None,
                    None,
                    None,
                ]));
            }
        }
    }

    #[inline(always)]
    pub(super) fn update_object_ic(
        access_ic: &mut [Option<AccessIc>],
        pc: usize,
        obj_ptr: usize,
        key: &str,
        value: &Val,
    ) {
        match access_ic[pc].as_mut() {
            Some(AccessIc::ObjectStr(slots)) => {
                let (hit, entry) = Vm::promote_or_insert(
                    slots,
                    |e| e.obj_ptr == obj_ptr && e.key == key,
                    || ObjectStrEntry {
                        obj_ptr,
                        key: key.to_string(),
                        value: value.clone(),
                    },
                );
                if hit {
                    entry.value = value.clone();
                }
            }
            _ => {
                access_ic[pc] = Some(AccessIc::ObjectStr([
                    Some(ObjectStrEntry {
                        obj_ptr,
                        key: key.to_string(),
                        value: value.clone(),
                    }),
                    None,
                    None,
                    None,
                ]));
            }
        }
    }

    #[inline(always)]
    pub(super) fn arith2_try_numeric(
        frame_raw: *mut super::FrameState<'_>,
        regs: &mut [Val],
        consts: &[Val],
        dst: u16,
        a: u16,
        b: u16,
        op_label: &'static str,
        iop: impl FnOnce(i64, i64) -> i64,
        fop: impl FnOnce(f64, f64) -> f64,
    ) -> bool {
        let ar = if rk_is_const(a) {
            &consts[rk_index(a) as usize]
        } else {
            &regs[rk_index(a) as usize]
        };
        let br = if rk_is_const(b) {
            &consts[rk_index(b) as usize]
        } else {
            &regs[rk_index(b) as usize]
        };
        let dst_idx = dst as usize;
        match (ar, br) {
            (Val::Int(x), Val::Int(y)) => {
                assign_reg_slice(frame_raw, regs, dst_idx, Val::Int(iop(*x, *y)));
                true
            }
            (Val::Float(x), Val::Float(y)) => {
                assign_reg_slice(frame_raw, regs, dst_idx, Val::Float(fop(*x, *y)));
                true
            }
            (Val::Int(x), Val::Float(y)) => {
                assign_reg_slice(frame_raw, regs, dst_idx, Val::Float(fop(*x as f64, *y)));
                true
            }
            (Val::Float(x), Val::Int(y)) => {
                assign_reg_slice(frame_raw, regs, dst_idx, Val::Float(fop(*x, *y as f64)));
                true
            }
            _ => {
                tracing::debug!(
                    target: "lkr::vm::slowpath",
                    op = op_label,
                    lhs = ar.type_name(),
                    rhs = br.type_name(),
                    "arith fast path miss"
                );
                false
            }
        }
    }

    #[inline(always)]
    pub(super) fn cmp2_try_numeric(
        frame_raw: *mut super::FrameState<'_>,
        regs: &mut [Val],
        consts: &[Val],
        dst: u16,
        a: u16,
        b: u16,
        iop: impl FnOnce(i64, i64) -> bool,
        fop: impl FnOnce(f64, f64) -> bool,
    ) -> bool {
        let ar = if rk_is_const(a) {
            &consts[rk_index(a) as usize]
        } else {
            &regs[rk_index(a) as usize]
        };
        let br = if rk_is_const(b) {
            &consts[rk_index(b) as usize]
        } else {
            &regs[rk_index(b) as usize]
        };
        let res_opt = match (ar, br) {
            (Val::Int(x), Val::Int(y)) => Some(iop(*x, *y)),
            (Val::Float(x), Val::Float(y)) => Some(fop(*x, *y)),
            (Val::Int(x), Val::Float(y)) => Some(fop(*x as f64, *y)),
            (Val::Float(x), Val::Int(y)) => Some(fop(*x, *y as f64)),
            _ => None,
        };
        if let Some(res) = res_opt {
            assign_reg_slice(frame_raw, regs, dst as usize, Val::Bool(res));
            true
        } else {
            false
        }
    }
}
