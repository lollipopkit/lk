/* Protected-region trampoline for the Cranelift backend (deep-coverage plan G).
 *
 * Cranelift cannot emit `setjmp` (a `returns_twice` call its SSA/regalloc model
 * does not support). This trampoline hoists the `setjmp` into a C frame that
 * outlives the try-body call and is the `_longjmp` target: it drives the same
 * lkrt protocol the generated code otherwise would (`lkrt_rt_try_push` →
 * `_setjmp` → body / `lkrt_rt_try_pop`).
 *
 * The try-body is a lowered `lk_fn_N` taking one argument: the address of the
 * caller's word buffer, which it reads its own inputs out of. So this file has
 * no idea how many values a region crosses, and nothing here caps it.
 */

/* lkrt runtime hooks (Rust `#[no_mangle] extern "C"`, linked from the same
 * staticlib). `_setjmp` is the BSD-semantics variant (no signal-mask
 * save/restore) matching lkrt's `_longjmp` raise path; declared with a `void*`
 * buffer to avoid the platform `jmp_buf` array type — ABI-compatible, as the
 * callee only reads the buffer through the pointer. */
extern int _setjmp(void *env);
extern void *lkrt_rt_try_push(void);
extern void lkrt_rt_try_pop(void);

/* Runs `body(argv)` under a fresh try frame, for the `try { … } catch e { … }`
 * *statement* — where the body produces no value and the only question is
 * whether it finished.
 *
 * Returns 1 when the body returned, 0 when it raised. The caught value stays
 * where `lkrt_rt_current_error` can be asked for it, so the caller reads it
 * only on the path that needs it.
 *
 * The body reads its inputs out of `argv` itself (`body_signature` in
 * `aot/codegen/src/clif.rs`), which is why there is no arity here. There used to
 * be: a switch casting `body` to one of nine `(long long, …)` prototypes, with a
 * trapping `default`. It reloaded a buffer the caller had already filled, and it
 * put a ceiling of eight on how many values a region could cross — past which
 * the lowering refused a program that was otherwise fine. */
long long lkrt_rt_try_region(const void *body, const long long *argv) {
    void *buf = lkrt_rt_try_push();
    if (_setjmp(buf) == 0) {
        ((void (*)(const long long *))body)(argv);
        lkrt_rt_try_pop();
        return 1;
    }
    /* The raise path's pop already happened inside lkrt's raise. */
    return 0;
}
