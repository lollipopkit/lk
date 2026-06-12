fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let src_dir = std::path::Path::new(&dir).join("src");
    println!("cargo::rerun-if-changed=src/parser.c");
    println!("cargo::rerun-if-changed=src/scanner.c");
    println!("cargo::rerun-if-changed=bindings/rust/build.rs");

    cc::Build::new()
        .file(src_dir.join("parser.c"))
        .file(src_dir.join("scanner.c"))
        .include(&src_dir)
        .include(std::path::Path::new(&dir))
        .compile("tree-sitter-lk");
}
