use crate::{
    llvm::scalar::{
        block_helpers::{local_heap_kind_before, local_register_kind_before},
        facts::NativeScalarKind,
    },
    vm::{Instr, ModuleArtifact, Opcode},
};

pub(super) fn subfunction_param_kind_candidates(param_count: usize) -> Vec<Vec<NativeScalarKind>> {
    const MAX_CANDIDATES: usize = 256;
    const MAX_ENUMERATED_PARAMS: usize = 5;

    if param_count == 0 {
        return vec![Vec::new()];
    }
    if param_count > MAX_ENUMERATED_PARAMS {
        return Vec::new();
    }
    let mut candidates = Vec::new();
    let Some(total) = 3usize.checked_pow(param_count as u32) else {
        return Vec::new();
    };
    if total > MAX_CANDIDATES {
        return Vec::new();
    }
    for mut encoded in 0..total {
        if candidates.len() >= MAX_CANDIDATES {
            eprintln!("lk-llvm: truncated subfunction param candidates at {MAX_CANDIDATES} for {param_count} params");
            break;
        }
        let mut kinds = Vec::with_capacity(param_count);
        for _ in 0..param_count {
            kinds.push(match encoded % 3 {
                0 => NativeScalarKind::I64,
                1 => NativeScalarKind::Bool,
                _ => NativeScalarKind::StrPtr,
            });
            encoded /= 3;
        }
        candidates.push(kinds);
    }
    candidates
}

pub(super) fn callsite_param_kind_candidates(
    artifact: &ModuleArtifact,
    function_index: u16,
    param_count: usize,
) -> Vec<Vec<NativeScalarKind>> {
    let mut out = Vec::new();
    for function in &artifact.module.functions {
        let Ok(code) = function
            .code
            .iter()
            .copied()
            .map(Instr::try_from_raw)
            .collect::<Result<Vec<_>, _>>()
        else {
            continue;
        };
        for (pc, instr) in code.iter().copied().enumerate() {
            if instr.opcode() != Opcode::CallDirect
                || instr.b() as u16 != function_index
                || instr.c() as usize != param_count
            {
                continue;
            }
            let start = instr.a() as usize + 1;
            let Some(kinds) = (start..start + param_count)
                .map(|reg| {
                    let reg = u8::try_from(reg).ok()?;
                    local_heap_kind_before(&code, &function.consts.heap_values, pc, reg)
                        .or_else(|| local_register_kind_before(&code, pc, reg))
                })
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if !out.contains(&kinds) {
                out.push(kinds);
            }
        }
    }
    out
}
