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
coroutines are core control flow, not an optional OS-facing module (unlike
`task`/`chan`, which front the tokio-backed async runtime and are unrelated
to this feature: coroutines are purely synchronous and cooperative within a
single VM instance).

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
