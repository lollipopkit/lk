use crate::vm::Module32Artifact;

use crate::llvm::{
    dynamic_containers::{
        native_dynamic_container_helpers, native_dynamic_f64_list_helpers, native_dynamic_i64_list_helpers,
        native_dynamic_i64_map_helpers, native_dynamic_ptr_list_helpers,
    },
    scalar::subfunctions::prepend_subfunctions,
};

pub(super) fn finish_scalar_ir(
    artifact: &Module32Artifact,
    mut ir: String,
    extra_globals: &str,
    recursive_indices: &[u16],
    additional_subfn_indices: &[u16],
) -> String {
    ir.push_str("exit:\n  ret i32 0\n");
    ir.push_str("lk_assert_fail:\n");
    ir.push_str("  call void @abort()\n");
    ir.push_str("  unreachable\n");
    ir.push_str("lk_divisor_zero:\n");
    ir.push_str("  ret i32 1\n");
    ir.push_str("}\n");
    ir.push_str(native_dynamic_container_helpers());
    ir.push_str(native_dynamic_i64_list_helpers());
    ir.push_str(native_dynamic_f64_list_helpers());
    ir.push_str(native_dynamic_i64_map_helpers());
    ir.push_str(native_dynamic_ptr_list_helpers());
    ir.push_str(extra_globals);
    prepend_subfunctions(artifact, ir, recursive_indices, additional_subfn_indices)
}
