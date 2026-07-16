//! Compiles the native protected-call trampoline (`src/try_trampoline.c`) into
//! the crate's object set. It hoists `setjmp` into a C frame for the Cranelift
//! backend, which cannot emit the `returns_twice` call itself (see the C file).
//! Bundled into `liblkrt.a`/the rlib, so the `lkrt_rt_try_call` symbol links
//! wherever the runtime does.

fn main() {
    println!("cargo:rerun-if-changed=src/try_trampoline.c");
    cc::Build::new()
        .file("src/try_trampoline.c")
        .compile("lk_try_trampoline");
}
