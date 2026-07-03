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

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* LK_H */
