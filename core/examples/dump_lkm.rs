use lk_core::vm::{Module32Artifact, disassemble_module32};

fn main() {
    let Some(path) = std::env::args_os().nth(1) else {
        eprintln!("usage: dump_lkm <module.lkm>");
        std::process::exit(2);
    };
    let path = std::path::PathBuf::from(path);
    let input =
        std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let artifact = Module32Artifact::from_json_str(&input)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));
    let module = artifact
        .into_module()
        .unwrap_or_else(|error| panic!("failed to convert {}: {error}", path.display()));
    println!("{}", disassemble_module32(&module));
}
