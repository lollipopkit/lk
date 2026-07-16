use lk_core::syntax::{ParseOptions, parse_program_source};
use lk_core::vm::{Compiler, ModuleArtifact};

fn dump(source: &str) -> String {
    let program = parse_program_source(source, ParseOptions::default()).expect("parse");
    let module = Compiler::compile_module(&program).expect("compile");
    let artifact = ModuleArtifact::new(Vec::new(), &module).expect("artifact");
    let mir = lk_aot_lower::lower_with_hybrid(&artifact, false).expect("lower");
    lk_aot_mir::render(&mir)
}

#[test]
fn dump_map_mir() {
    for (name, src) in [
        ("map_new_len", "let m = {\"a\": 1, \"b\": 2};\nreturn m.len();\n"),
        ("map_get", "let m = {\"a\": 1, \"b\": 2};\nreturn m[\"a\"];\n"),
    ] {
        eprintln!("\n===== {name} =====\n{}", dump(src));
    }
}
