# Cross-module trait dispatch

`x.method()` resolves through one runtime table, `VmContext::methods`, keyed by
type name → method name. The table is shared by every module running under one
context, which is what makes dispatch work at all: `main.lk` can call
`c.area()` on a `Circle` whose `impl` lives in `shape.lk`, because importing
`shape.lk` registers that impl into the shared table.

Each entry records **where its body lives** (`MethodImpl`):

| variant | body | called against |
| --- | --- | --- |
| `Local { module, function }` | function index in `module` | the executing state, when `module` *is* the executing module |
| `Imported(RuntimeCallable)` | function index in the callable's own module | that module's own `Arc<Mutex<RuntimeModuleState>>`, with arguments and the result marshalled across heaps |

## What does not work, and why

An `impl` declared in module **A** cannot be dispatched while module **B** is
executing:

```lk
// shape2.lk
fn render(q) { return q.area(); }

// main.lk
use { render } from "./shape2";
struct Sq { s: Int }
trait Area { fn area(self) -> Int; }
impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }
println("{}", render(Sq { s: 4 }));   // error, by design — see below
```

Running `A::area` correctly requires A's globals and A's heap. Both live in
A's `RuntimeModuleState`, and at the moment of dispatch that state is owned by
an `Executor` further up the Rust stack — `Executor` takes a module's state by
value (`core::mem::take`) and hands it down the call chain. There is no handle
to it in the context, and there cannot be a `&mut` to it either, because an
outer frame is holding one.

The `Imported` variant does not have this problem only because an imported
module's state is *not* currently executing: it sits in an `Arc<Mutex<..>>` and
the call takes it out.

So the case is refused with a precise error rather than approximated. The two
approximations both silently produce wrong answers:

- **Resolve the index against the executing module.** This is what the code did
  before. Function #N in B is a different function, and the failure is silent —
  the repro above recursed into `render` itself until the stack overflowed.
- **Resolve the index against A but run it in B's state.** Constants and
  callees would be right and *globals would be wrong*, because global access is
  by slot index into the executing state.

## What it would take

The precondition is that a module's runtime state stops travelling with the
execution flow: states become context-owned and re-entrant, so a nested call
into a module already on the stack pushes a frame onto that module's state
instead of needing ownership of it. That is an execution-model change, not a
dispatch change, and it has to answer for the hot loop — the current design's
whole point is that an executor owns its state outright and never locks per
instruction.

Until then `MethodImpl::Local` carries its declaring module so the mismatch is
*detected*. Detecting it is what turned a stack overflow into an error message.
