use crate::llvm::subfunction::{
    compile_native_i64_list_subfunction, compile_native_ptr_list_subfunction, compile_native_scalar_subfunction,
};
use crate::vm::Module32Artifact;

pub(in crate::llvm) fn prepend_subfunctions(
    artifact: &Module32Artifact,
    ir: String,
    recursive_indices: &[u16],
    additional_subfn_indices: &[u16],
) -> String {
    let mut subfunction_ir = String::new();
    for &index in recursive_indices {
        if let Ok(Some(fn_ir)) = compile_native_scalar_subfunction(artifact, index as usize, recursive_indices) {
            subfunction_ir.push_str(&fn_ir);
        } else if let Ok(Some(fn_ir)) = compile_native_i64_list_subfunction(artifact, index as usize) {
            subfunction_ir.push_str(&fn_ir);
        } else if let Ok(Some(fn_ir)) = compile_native_ptr_list_subfunction(artifact, index as usize) {
            subfunction_ir.push_str(&fn_ir);
        }
    }

    let mut all_subfn_indices: Vec<u16> = recursive_indices.to_vec();
    all_subfn_indices.extend(additional_subfn_indices.iter().copied());
    let mut emitted_subfn: Vec<u16> = Vec::new();
    for &index in additional_subfn_indices {
        if recursive_indices.contains(&index) || emitted_subfn.contains(&index) {
            continue;
        }
        if let Ok(Some(fn_ir)) = compile_native_scalar_subfunction(artifact, index as usize, &all_subfn_indices) {
            subfunction_ir.push_str(&fn_ir);
            emitted_subfn.push(index);
        } else if let Ok(Some(fn_ir)) = compile_native_i64_list_subfunction(artifact, index as usize) {
            subfunction_ir.push_str(&fn_ir);
            emitted_subfn.push(index);
        } else if let Ok(Some(fn_ir)) = compile_native_ptr_list_subfunction(artifact, index as usize) {
            subfunction_ir.push_str(&fn_ir);
            emitted_subfn.push(index);
        }
    }

    if subfunction_ir.is_empty() {
        ir
    } else {
        format!("{subfunction_ir}\n{ir}")
    }
}
