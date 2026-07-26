//! Compiles the native protected-call trampoline (`src/try_trampoline.c`) into
//! the crate's object set. It hoists `setjmp` into a C frame for the Cranelift
//! backend, which cannot emit the `returns_twice` call itself (see the C file).
//! Bundled into `liblkrt.a`/the rlib, so the `lkrt_rt_try_call` symbol links
//! wherever the runtime does.

fn main() {
    println!("cargo:rerun-if-changed=src/try_trampoline.c");

    // Bare-metal targets are skipped, for two reasons that agree.
    //
    // Practically: `setjmp` comes from libc, and a bare-metal target has none
    // unless the board supplies newlib — so building this would need a C
    // cross-compiler that may not exist (`arm-none-eabi-gcc`), failing the
    // build over a symbol nothing there can call.
    //
    // Semantically: the trampoline exists for `try`/`catch`, which the native
    // lowering does not support anyway (`TryBegin` is the standing entry in the
    // AOT coverage allow-list). A bare-metal image that used `try` would fail
    // to lower long before it reached the linker.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("-none") {
        println!(
            "cargo:warning=skipping the try/catch trampoline for {target}: \
             bare-metal targets have no libc `setjmp`, and native `try` lowering is unsupported"
        );
        return;
    }

    cc::Build::new()
        .file("src/try_trampoline.c")
        .compile("lk_try_trampoline");
}
