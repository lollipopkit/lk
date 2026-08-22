/* Stack-exhaustion diagnostic for native LK binaries.
 *
 * The VM bounds recursion by counting frames: past `LK_MAX_CALL_DEPTH` (100000
 * by default) it raises `call depth limit exceeded`, which `try` can catch. A
 * native binary has no counter — it has the real stack — so runaway recursion
 * walked into the guard page and the process died on SIGSEGV: **exit 139 and not
 * one byte of output**. Same program, same mistake, and one backend explained it
 * while the other said nothing.
 *
 * Rust's own binaries print "has overflowed its stack" because `lang_start`
 * installs this handler during runtime init. A generated binary's `main` is
 * Cranelift's, so that init never runs and the handler is never installed. This
 * file installs it from the entry prologue instead.
 *
 * In C rather than Rust for the same reason `try_trampoline.c` is: `sigaltstack`
 * and `sigaction` are libc shapes whose structs are platform-specific, and lkrt
 * has no `libc` dependency to spell them with. `build.rs` skips this file for
 * bare-metal targets, which have no signals to handle.
 *
 * Everything the handler does is async-signal-safe: `write` and `_exit`, no
 * allocation and no locks. That is also why it cannot raise into the enclosing
 * `try` the way the VM's depth error does — that needs a value on the arena and
 * lkrt's handler stack behind a `RefCell`, neither of which a signal handler may
 * touch. So the native answer is a diagnosed exit, not a catchable error, and the
 * two backends still differ in *that* respect; the difference is written down in
 * docs/semantics.md rather than left for a reader to discover as exit 139.
 */

#include <signal.h>
#include <stddef.h>
#include <stdint.h>
#include <string.h>
#include <sys/resource.h>
#include <unistd.h>

/* The handler needs a stack of its own: the thread's is what just ran out.
 * Sized generously and statically, because allocating here would defeat the
 * point. */
static char lk_alt_stack[65536];

/* The address range a stack overflow can fault in. Established at install time
 * from this frame's address (the prologue runs from `main`, so it is near the
 * base) and `RLIMIT_STACK`. */
static uintptr_t lk_stack_low;
static uintptr_t lk_stack_high;

static const char LK_STACK_MSG[] =
    "Error: stack exhausted: recursion too deep. A native binary is bounded by the real stack, not by "
    "LK_MAX_CALL_DEPTH, and cannot catch this the way the VM does.\n";

static void lk_on_fault(int sig, siginfo_t *info, void *context) {
    (void)context;
    uintptr_t addr = (uintptr_t)info->si_addr;
    if (addr >= lk_stack_low && addr <= lk_stack_high) {
        /* `write` and `_exit`: the two calls a handler may make. Exit 1, which is
         * what the VM exits with for the same program. */
        ssize_t written = write(2, LK_STACK_MSG, sizeof(LK_STACK_MSG) - 1);
        (void)written;
        _exit(1);
    }
    /* Not the stack — a genuine bad access. Put the default action back and
     * return, so the fault re-triggers and the process dies exactly as it did
     * before this file existed. Reporting it as "stack exhausted" would be a
     * message that names the wrong cause. */
    signal(sig, SIG_DFL);
}

void lk_install_stack_guard(void) {
    stack_t alt;
    memset(&alt, 0, sizeof(alt));
    alt.ss_sp = lk_alt_stack;
    alt.ss_size = sizeof(lk_alt_stack);
    alt.ss_flags = 0;
    if (sigaltstack(&alt, NULL) != 0) {
        return;
    }

    char here;
    struct rlimit limit;
    /* An unlimited stack still faults somewhere; 64 MiB is a bound wide enough
     * to cover a real overflow and narrow enough that a wild pointer elsewhere
     * in the address space is not mistaken for one. */
    size_t span = (size_t)64 * 1024 * 1024;
    if (getrlimit(RLIMIT_STACK, &limit) == 0 && limit.rlim_cur != RLIM_INFINITY) {
        span = (size_t)limit.rlim_cur;
    }
    uintptr_t base = (uintptr_t)&here;
    /* One page of slack above, because the prologue's frame is not the very top.
     * Saturating below, so a small `base` cannot wrap the range open. */
    lk_stack_high = base + 4096;
    lk_stack_low = (base > span + 4096) ? base - span - 4096 : 0;

    struct sigaction action;
    memset(&action, 0, sizeof(action));
    action.sa_sigaction = lk_on_fault;
    action.sa_flags = SA_ONSTACK | SA_SIGINFO;
    sigemptyset(&action.sa_mask);
    sigaction(SIGSEGV, &action, NULL);
    /* Some platforms (macOS) report a guard-page hit as SIGBUS. */
    sigaction(SIGBUS, &action, NULL);
}
