use crate::val::Val;
use crate::vm::vm::frame::FrameState;

use super::super::super::helpers::assign_reg;

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_len(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
) {
    let out = match &regs[src as usize] {
        Val::List(list) => Val::Int(list.len() as i64),
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        Val::Map(map) => Val::Int(map.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_list_len(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
) {
    let out = match &regs[src as usize] {
        Val::List(list) => Val::Int(list.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_map_len(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
) {
    let out = match &regs[src as usize] {
        Val::Map(map) => Val::Int(map.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_str_len(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
) {
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_floor(
    frame_raw: *mut FrameState<'_, '_>,
    regs: &mut [Val],
    dst: u16,
    src: u16,
) {
    let out = match &regs[src as usize] {
        Val::Float(value) => Val::Int(value.floor() as i64),
        Val::Int(value) => Val::Int(*value),
        _ => Val::Int(0),
    };
    assign_reg(frame_raw, regs, dst as usize, out);
}
