//! Compiles the C parts of the runtime into the crate's object set: the native
//! protected-call trampoline (`src/try_trampoline.c`) and the stack-exhaustion
//! guard (`src/stack_guard.c`). It hoists `setjmp` into a C frame for the Cranelift
//! backend, which cannot emit the `returns_twice` call itself (see the C file).
//! Bundled into `liblkrt.a`/the rlib, so the `lkrt_rt_try_call` symbol links
//! wherever the runtime does.

fn main() {
    println!("cargo:rerun-if-changed=src/try_trampoline.c");
    println!("cargo:rerun-if-changed=src/stack_guard.c");

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

    // `stack_guard.c` rides along for the same reason and with the same
    // exclusion: it needs `sigaltstack`/`sigaction`, and a bare-metal target has
    // no signals for them to install.
    cc::Build::new()
        .file("src/try_trampoline.c")
        .file("src/stack_guard.c")
        .compile("lk_try_trampoline");
}
