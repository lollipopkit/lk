/* Native protected-call trampoline for the Cranelift backend (deep-coverage
 * plan G: `try$call`).
 *
 * Cranelift cannot emit `setjmp` (a `returns_twice` call its SSA/regalloc model
 * does not support), so the string-IR path's inline `_setjmp` has no Cranelift
 * equivalent. This trampoline hoists the `setjmp` into a C frame that outlives
 * the try-body call and is the `_longjmp` target: it drives the same lkrt
 * protocol the generated code otherwise would (`lkrt_rt_try_push` → `_setjmp` →
 * body / `lkrt_rt_try_pop`, or `lkrt_rt_current_error` on a caught raise).
 *
 * The try-body is a lowered `lk_fn_N` returning `LkDyn` by value; only
 * integer/pointer-width parameters are supported (the Cranelift lowering rejects
 * float/carrier params and passes each argument as one `i64` word), so a fixed
 * arity switch covers every callable shape without touching the FP registers.
 */

typedef struct LkDyn {
    long long tag;
    long long payload;
} LkDyn;

/* lkrt runtime hooks (Rust `#[no_mangle] extern "C"`, linked from the same
 * staticlib). `_setjmp` is the BSD-semantics variant (no signal-mask
 * save/restore) matching lkrt's `_longjmp` raise path; declared with a `void*`
 * buffer to avoid the platform `jmp_buf` array type — ABI-compatible, as the
 * callee only reads the buffer through the pointer. */
extern int _setjmp(void *env);
extern void *lkrt_rt_try_push(void);
extern void lkrt_rt_try_pop(void);
extern LkDyn lkrt_rt_current_error(void);

static LkDyn lk_call_body(const void *body, long long argc, const long long *a) {
    switch (argc) {
    case 0:
        return ((LkDyn(*)(void))body)();
    case 1:
        return ((LkDyn(*)(long long))body)(a[0]);
    case 2:
        return ((LkDyn(*)(long long, long long))body)(a[0], a[1]);
    case 3:
        return ((LkDyn(*)(long long, long long, long long))body)(a[0], a[1], a[2]);
    case 4:
        return ((LkDyn(*)(long long, long long, long long, long long))body)(a[0], a[1], a[2], a[3]);
    case 5:
        return ((LkDyn(*)(long long, long long, long long, long long, long long))body)(a[0], a[1], a[2], a[3],
                                                                                        a[4]);
    case 6:
        return ((LkDyn(*)(long long, long long, long long, long long, long long, long long))body)(
            a[0], a[1], a[2], a[3], a[4], a[5]);
    case 7:
        return ((LkDyn(*)(long long, long long, long long, long long, long long, long long, long long))body)(
            a[0], a[1], a[2], a[3], a[4], a[5], a[6]);
    case 8:
        return ((LkDyn(*)(long long, long long, long long, long long, long long, long long, long long,
                          long long))body)(a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]);
    default:
        /* The Cranelift lowering caps arity at LK_TRY_MAX_ARGS and falls back to
         * the string-IR path above it, so this is unreachable. */
        __builtin_trap();
    }
}

/* Runs `body(argv[0..argc])` under a fresh try frame. On normal return, writes
 * `*out_ok = 1` and returns the body's `LkDyn`. On a raise inside the body, the
 * `_longjmp` lands here with a non-zero `_setjmp` result: writes `*out_ok = 0`
 * and returns the caught error (`lkrt_rt_current_error`). The failure path's
 * handler pop already happened inside lkrt's raise, matching the string-IR
 * catch arm (which likewise does not pop on the raise path). */
LkDyn lkrt_rt_try_call(const void *body, long long argc, const long long *argv, long long *out_ok) {
    void *buf = lkrt_rt_try_push();
    if (_setjmp(buf) == 0) {
        LkDyn r = lk_call_body(body, argc, argv);
        lkrt_rt_try_pop();
        *out_ok = 1;
        return r;
    }
    *out_ok = 0;
    return lkrt_rt_current_error();
}
