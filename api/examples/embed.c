/* Minimal C embedding of LK via the lk-api C ABI.
 *
 * Build lk-api as a static lib with the `ffi` feature, then e.g.:
 *   cc api/examples/embed.c -I api/include -L target/release -llk_api -o embed
 * (link flags depend on your platform/toolchain).
 */
#include <stdio.h>
#include "lk.h"

int main(void) {
    LkVm *vm = lk_vm_new();
    char *out = lk_vm_eval(vm, "return 6 * 7;");
    if (out) {
        printf("6 * 7 = %s\n", out); /* -> 42 */
        lk_string_free(out);
    } else {
        fprintf(stderr, "eval failed\n");
    }
    lk_vm_free(vm);
    return 0;
}
