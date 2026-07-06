# Coroutines / `yield`

Stackless coroutines (plan.md 4.5's payoff from the M2.5 stackless-VM work —
see `docs/vm-stackless.md`). A coroutine wraps a plain LK function and lets
its execution suspend mid-call via `yield`, to be resumed later — the classic
Lua-style `coroutine.create`/`resume`/`yield` model, exposed here as three
global functions plus a `yield` expression.

## API

- `coroutine_create(fn) -> Coroutine` — wraps `fn` (a plain LK closure, not a
  native function) as a coroutine. Doesn't run any code yet.
- `coroutine_resume(co, ...args) -> [ok, value]` — same `[ok, value]`
  convention as `pcall`. On the *first* resume, `args` seed the function's
  parameters like an ordinary call (arity-checked). On later resumes, only
  `args[0]` (or `nil` if omitted) is delivered as the value the paused
  `yield` expression evaluates to. `ok` is `false` if the coroutine raised an
  uncaught error (the coroutine is then dead, same as after a normal return)
  or if `co` isn't resumable (already running, or already dead).
- `coroutine_status(co) -> "suspended" | "running" | "dead"`.
- `yield expr` — an expression. Suspends the *entire* call chain inside the
  running coroutine (not just the innermost function — nested LK calls yield
  correctly too) and hands `expr`'s value out through `coroutine_resume`.
  Evaluates, once resumed, to whatever value that `coroutine_resume` call
  passed.

These are global builtins (no `use` needed), like `pcall`/`error`/`assert` —
coroutines are core control flow, not an optional OS-facing module. They are
purely synchronous and cooperative within a single VM instance; the bridge to
the tokio-backed `task`/`chan` runtime is the `sched` stdlib module (below).

## Example

```lk
fn counter(n) {
    let i = 0;
    while (i < n) {
        yield i;
        i = i + 1;
    }
    return "done";
}

let co = coroutine_create(counter);
let step = coroutine_resume(co, 3);
while (step[0] && coroutine_status(co) != "dead") {
    println(step[1]);       // 0, 1, 2
    step = coroutine_resume(co);
}
println(step[1]);           // "done"
```

Two-way value passing (`let x = yield v;` receives the next resume's
argument) and error propagation (`error(...)` inside a coroutine surfaces as
`[false, value]` from `coroutine_resume`, exactly like `pcall`) are covered
by `examples/syntax/coroutines.lk`, which also doubles as the differential
conformance corpus (VM==bytecode and VM==AOT gates).

## Restrictions (v1)

- **`yield` only parses at the top of an expression** (statement position,
  `let` value, ternary branches, …), not nested inside an arbitrary
  sub-expression — `yield a + b` means `yield (a + b)`, but `1 + yield 2` is
  a parse error. This mirrors Rust's own (nightly) `yield` restriction.
- **Any function may contain `yield`** — there's no `async fn`/generator
  marker. Calling such a function directly (not through
  `coroutine_resume`) raises a catchable runtime error the moment it tries to
  yield ("yield used outside a running coroutine"), not a compile error.
- **Yielding across a native-call boundary is a runtime error** — the same
  structural restriction as Lua's classic "cannot yield across a C-call
  boundary". Concretely: `pcall`, stdlib higher-order-function callbacks
  (`list.map`/`filter`/`reduce`, …), and `CallMethodK`'s trait-method/
  callable-property dispatch all re-enter the VM through a *separate*
  `Executor` (see `runtime_callable.rs`), not the coroutine's own call
  chain — a `yield` reached through one of those is caught and reported the
  same way as a yield with no coroutine at all.
- **No AOT lowering yet** — `Yield` isn't part of the native-lowerable
  subset. `lk compile` on a program using coroutines falls back to the
  Tier 0 VM bundle automatically (program-grain fallback, same as any other
  `Unsupported` construct — see `progress.md`'s M4.2 notes); it still
  produces a working, self-contained binary, just not natively-compiled.

## The `sched` cooperative scheduler (`use sched;`)

Because natives can't yield (previous section), a blocking channel `recv(c)`
inside a coroutine can't transparently suspend it Go-style — it would block
the whole VM thread. The `sched` module is the stackless answer: its builders
never block and never yield, they only **construct wait descriptors** (small
tagged lists), user code suspends itself explicitly with `yield`, and
`sched.run` interprets the descriptors:

- `sched.run(...fns_or_cos) -> List` — wraps plain functions as coroutines
  and drives everything round-robin until all coroutines (including any they
  `sched.spawn`) finish. Returns one `[ok, value]` per argument, in order;
  one coroutine erroring doesn't stop the rest. When every coroutine is
  parked, it blocks on a tokio select over all parked channel/task arms,
  bounded by the earliest sleep deadline.
- `yield sched.recv(c)` → `[ok, value]` (like the blocking `recv`; `[false,
  nil]` once the channel closes).
- `yield sched.send(c, v)` → Bool (like the blocking `send`; `false` if the
  channel closed).
- `yield sched.sleep(ms)` → Nil, parking on a timer instead of the thread.
- `yield sched.pause()` → Nil, an explicit fairness point (back of the run
  queue).
- `yield sched.spawn(f, ...args)` → the new coroutine's handle; `args` seed
  `f`'s parameters.
- `yield sched.join(co)` → that coroutine's `[ok, value]` once it finishes
  (only for coroutines managed by this `sched.run`).
- `yield sched.await(t)` → `[ok, value]` of a tokio `Task` (as produced by
  task-returning natives, e.g. `net.tcp`'s async ops) without blocking the
  sibling coroutines.

```lk
use sched;
use chan as ch;

let c = chan(1);
let producer = || {
    for i in 0..4 { yield sched.send(c, i); }
    ch.close(c);
    return "done";
};
let consumer = || {
    let n = 0;
    while (true) {
        let r = yield sched.recv(c);
        if (!r[0]) { break; }
        n = n + 1;
    }
    return n;
};
let results = sched.run(producer, consumer);   // [[true, "done"], [true, 4]]
```

Semantics worth knowing:

- Parked channel/task waits may legitimately block forever if nothing ever
  delivers (an external tokio task may send at any time — same stance as Go).
  A **join cycle**, however, is provably stuck and reported as a deadlock
  error instead of hanging.
- Yielding anything that isn't a `sched` descriptor inside `sched.run` is a
  catchable error — generator-style coroutines belong to bare
  `coroutine_resume`, not the scheduler.
- Don't resume scheduler-managed coroutines manually mid-`sched.run`; the
  scheduler owns their lifecycle (worst case is a clear runtime error, but
  it's still misuse).
- Channel values are deep-copied across `send`/`recv` (channels are cross-VM
  by design), so coroutine handles can't travel through channels — share
  them through captured maps/lists instead.

`examples/stdlib/sched_demo.lk` is the runnable, differential-gated corpus
for all of the above.

## The `select` statement

Go-style multi-way channel choice, as an expression:

```lk
let got = select {
    case v <- recv(c) if enabled => v + 1;   // recv arm, binding + guard
    case send(out, 7) => "sent";             // send arm
    default => "nothing-ready";              // optional; omit to block
};
```

`select` is pure parse-time sugar (the same treatment as `try`/`catch` →
`pcall`): the parser lowers it onto the `select$block` runtime builtin plus
ordinary lets and conditionals, so the resolver, type checker, VM compiler,
and AOT need no dedicated support. Semantics pinned by that lowering:

- Channel operands, send values, and guards evaluate **eagerly, once, in
  source order** (the Go rule), whether or not their arm fires.
- Guards evaluate *before* any arm is chosen, so a recv binding is not in
  scope inside its own guard; any truthy guard value enables the arm.
- A recv binding receives the **value** (`nil` once the channel closes — the
  Go zero-value analogue). Case bodies are single expressions, like match
  arms, evaluated in the enclosing scope.
- Without `default`, `select` **blocks the OS thread** until an arm is ready
  — like the blocking `recv`/`send` globals, and like them it must not be
  used inside `sched.run` coroutines (park with `yield sched.recv/send`
  instead; a cooperative multi-way wait is a possible future `sched.any`).
- Selecting on a **closed** channel is a catchable error (`chan.close`
  removes the channel — same stance as blocking `recv`). With **every arm
  guarded off and no default**, the select evaluates to `nil` rather than
  deadlocking (a deliberate deviation from Go's panic).

`examples/syntax/select.lk` is the differential-gated corpus.
