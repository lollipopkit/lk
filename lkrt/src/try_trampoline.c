/* Protected-region trampoline for the Cranelift backend (deep-coverage plan G).
 *
 * Cranelift cannot emit `setjmp` (a `returns_twice` call its SSA/regalloc model
 * does not support). This trampoline hoists the `setjmp` into a C frame that
 * outlives the try-body call and is the `_longjmp` target: it drives the same
 * lkrt protocol the generated code otherwise would (`lkrt_rt_try_push` →
 * `_setjmp` → body / `lkrt_rt_try_pop`).
 *
 * The try-body is a lowered `lk_fn_N`; only integer/pointer-width parameters
 * are supported (the Cranelift lowering rejects float/carrier params and passes
 * each argument as one `i64` word), so a fixed arity switch covers every
 * callable shape without touching the FP registers.
 */

/* lkrt runtime hooks (Rust `#[no_mangle] extern "C"`, linked from the same
 * staticlib). `_setjmp` is the BSD-semantics variant (no signal-mask
 * save/restore) matching lkrt's `_longjmp` raise path; declared with a `void*`
 * buffer to avoid the platform `jmp_buf` array type — ABI-compatible, as the
 * callee only reads the buffer through the pointer. */
extern int _setjmp(void *env);
extern void *lkrt_rt_try_push(void);
extern void lkrt_rt_try_pop(void);

/* Runs `body()` under a fresh try frame, for the `try { … } catch e { … }`
 * *statement* — where the body produces no value and the only question is
 * whether it finished.
 *
 * Returns 1 when the body returned, 0 when it raised. The caught value stays
 * where `lkrt_rt_current_error` can be asked for it, so the caller reads it
 * only on the path that needs it. */
static void lk_call_void_body(const void *body, long long argc, const long long *a) {
    switch (argc) {
    case 0:
        ((void (*)(void))body)();
        return;
    case 1:
        ((void (*)(long long))body)(a[0]);
        return;
    case 2:
        ((void (*)(long long, long long))body)(a[0], a[1]);
        return;
    case 3:
        ((void (*)(long long, long long, long long))body)(a[0], a[1], a[2]);
        return;
    case 4:
        ((void (*)(long long, long long, long long, long long))body)(a[0], a[1], a[2], a[3]);
        return;
    case 5:
        ((void (*)(long long, long long, long long, long long, long long))body)(a[0], a[1], a[2], a[3], a[4]);
        return;
    case 6:
        ((void (*)(long long, long long, long long, long long, long long, long long))body)(a[0], a[1], a[2], a[3],
                                                                                           a[4], a[5]);
        return;
    case 7:
        ((void (*)(long long, long long, long long, long long, long long, long long, long long))body)(
            a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
        return;
    case 8:
        ((void (*)(long long, long long, long long, long long, long long, long long, long long, long long))body)(
            a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]);
        return;
    default:
        /* The lowering caps arity and rejects above it, so this is
         * unreachable. */
        __builtin_trap();
    }
}

long long lkrt_rt_try_region(const void *body, long long argc, const long long *argv) {
    void *buf = lkrt_rt_try_push();
    if (_setjmp(buf) == 0) {
        lk_call_void_body(body, argc, argv);
        lkrt_rt_try_pop();
        return 1;
    }
    /* The raise path's pop already happened inside lkrt's raise. */
    return 0;
}
