use crate::val::Val;

use super::super::super::helpers::assign_reg_with_metrics;

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_len(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let out = match &regs[src as usize] {
        Val::List(list) => Val::Int(list.len() as i64),
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        Val::Map(map) => Val::Int(map.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_list_len(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let out = match &regs[src as usize] {
        Val::List(list) => Val::Int(list.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_map_len(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let out = match &regs[src as usize] {
        Val::Map(map) => Val::Int(map.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_str_len(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let out = match &regs[src as usize] {
        Val::ShortStr(value) => Val::Int(value.as_str().len() as i64),
        Val::Str(value) => Val::Int(value.len() as i64),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}

#[inline]
pub(in crate::vm::vm::runtime::frame::run::opcode) fn run_floor(
    regs: &mut [Val],
    dst: u16,
    src: u16,
    collect_metrics: bool,
) {
    let out = match &regs[src as usize] {
        Val::Float(value) => Val::Int(value.floor() as i64),
        Val::Int(value) => Val::Int(*value),
        _ => Val::Int(0),
    };
    assign_reg_with_metrics(regs, dst as usize, out, collect_metrics);
}
