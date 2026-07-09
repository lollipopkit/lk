/* lk.h — C ABI for embedding the LK virtual machine (lk-api `ffi` feature).
 *
 * Build lk-api with `--features ffi` and link the produced static/dynamic
 * library. Each `LkVm` is an isolated instance (no shared global state).
 * A cbindgen config could regenerate this; kept hand-written as the surface
 * is tiny and stable.
 */
#ifndef LK_H
#define LK_H

#ifdef __cplusplus
extern "C" {
#endif

/* Opaque handle to an isolated LK VM instance. */
typedef struct LkVm LkVm;

/* Create a new VM (full stdlib registered). Free with lk_vm_free. */
LkVm *lk_vm_new(void);

/* Evaluate NUL-terminated UTF-8 `src` on `vm`. Returns a newly-allocated C
 * string with the first return value's display (free with lk_string_free),
 * or NULL on parse/runtime error or invalid input. */
char *lk_vm_eval(LkVm *vm, const char *src);

/* Free a VM created by lk_vm_new. */
void lk_vm_free(LkVm *vm);

/* Free a string returned by lk_vm_eval. */
void lk_string_free(char *s);

/* ---- Tier 1 hybrid bridge (docs/llvm/tier1-hybrid.md) --------------------
 * A hybrid native binary embeds its module artifact and calls VM-executed
 * functions through this one-way bridge. Process-singleton by design.
 */

#include <stddef.h>
#include <stdint.h>

/* Argument tags. LK_HYBRID_ARG_BOOL reads the `i` field as 0/1. */
#define LK_HYBRID_ARG_I64 0
#define LK_HYBRID_ARG_F64 1
#define LK_HYBRID_ARG_BOOL 2
#define LK_HYBRID_ARG_STR 3

typedef struct LkHybridArg {
    uint8_t tag;
    union {
        int64_t i;
        double f;
        const char *s;
    } value;
} LkHybridArg;

/* Register the embedded module artifact JSON (NUL-terminated; must outlive
 * every bridge call — wrappers pass a static constant). Decoding is deferred
 * to the first lk_hybrid_call_v. */
void lk_hybrid_register(const char *module_artifact_json);

/* Call VM-executed function `func_index` with `argc` tagged scalar arguments,
 * discarding the result. On any error (bad artifact, unknown function,
 * uncaught VM error) prints the message to stderr and exits nonzero — the
 * uncaught-error behavior of the VM itself. */
void lk_hybrid_call_v(uint32_t func_index, const LkHybridArg *args, size_t argc);

/* Mirror of lkrt's LkDyn ({ i64, i64 } by value): the v2 bridge return
 * carrier. Tags mirror lkrt's DYN_* constants (a conformance test pins
 * them); string payloads are leaked C strings (arena ownership). */
#define LK_HYBRID_DYN_NIL 0
#define LK_HYBRID_DYN_BOOL 1
#define LK_HYBRID_DYN_I64 2
#define LK_HYBRID_DYN_F64 3
#define LK_HYBRID_DYN_STR 4
#define LK_HYBRID_DYN_LIST 5
#define LK_HYBRID_DYN_MAP 6

typedef struct LkHybridDyn {
    int64_t tag;
    int64_t payload;
} LkHybridDyn;

/* Call VM-executed function `func_index` and return its result as an
 * LkDyn-shaped value (v2 bridge). Same error behavior as lk_hybrid_call_v. */
LkHybridDyn lk_hybrid_call_r(uint32_t func_index, const LkHybridArg *args, size_t argc);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* LK_H */
