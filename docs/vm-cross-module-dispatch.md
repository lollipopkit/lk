# Cross-module trait dispatch

`x.method()` resolves through one runtime table, `VmContext::methods`, keyed by
declaring-module scope → type name → method name. The table is shared by every
module running under one context, which is what makes dispatch work at all:
`main.lk` can call `c.area()` on a `Circle` whose `impl` lives in `shape.lk`,
because loading `shape.lk` registers that impl into the shared table.

Each entry records **where its body lives** (`MethodImpl`):

| variant | body | called against |
| --- | --- | --- |
| `Local { module, function }` | function index in `module` | the executing state — directly when `module` *is* the executing module, otherwise as described below |
| `Imported(RuntimeCallable)` | function index in the callable's own module | that module's own `Arc<Mutex<RuntimeModuleState>>`, with arguments and the result marshalled across heaps |

## The hard case: an `impl` reached from another module's frame

```lk
// shape2.lk
fn render(q) { return q.area(); }

// main.lk
use { render } from "./shape2";
struct Sq { s: Int }
trait Area { fn area(self) -> Int; }
impl Area for Sq { fn area(self) -> Int { return self.s * self.s; } }
println("{}", render(Sq { s: 4 }));   // 16
```

At the moment of dispatch the executing frame belongs to `shape2`, but the body
belongs to `main`. Three things tie a function index to its module, and only
one of them is a real obstacle:

- **The function table** — handled by passing the declaring module down, which
  is what `MethodImpl::Local` carries the module for. Getting this wrong is what
  the code did before: index *N* was resolved against whoever was executing, so
  this very program recursed into `shape2`'s function #N until the stack
  overflowed.
- **Constants** — not an obstacle at all: a `ConstPool` belongs to the
  `Function`, not the module.
- **Globals** — the real one. Global access is by slot index into the executing
  state's table, and slot numbering is per module.

`Imported` does not face this because an imported module's state is *not*
executing: it sits in an `Arc<Mutex<..>>` and the call takes it out. The
declaring module of a `Local` entry, when it differs from the executing one, is
mid-execution somewhere up the Rust stack, so its state is unreachable.

## How globals are handled

The compiler records, per impl method, how its reachable subtree uses globals
(`ImplMethod::{writes_globals, reads_globals}`, computed by
`Compiler::record_impl_method_global_use`). Reachability follows `CallDirect`
and `MakeClosure`; an indirect call is not followed and counts as
`writes_globals`, which is what makes `reads_globals` *complete* for every
method the flag clears.

A cross-module dispatch then runs the body in the current heap with a global
table shaped like the declaring module's and **only the proven-read slots
seeded** — for most methods, none at all. The `pc`-keyed inline caches get a
fresh scope for the call, since a foreign function's pcs mean nothing in the
host's cache arrays.

Two things this deliberately does not do:

- **It never seeds the whole table.** A module's globals include its imports,
  and importing one reads the exporting module's heap — which is checked out of
  its mutex exactly when that module is the one executing, i.e. precisely the
  situation here. Seeding blind fails with `heap object N out of bounds`.
- **It refuses a body that writes a global.** The write would land in the
  temporary table and vanish on restore, silently diverging the module's state.
  Such a method reports that, naming `Type::method`.

## What is still refused

- An impl method that writes a module global.
- An impl method that can reach code the static walk cannot see: any `Call`,
  `CallNamed`, or `CallMethodK` in the subtree. This is conservative — a
  `println` inside a method is enough — and only ever costs coverage, never
  correctness. Narrowing it wants the call-site target facts the analysis
  already computes (`PerfCallTargetKind`): a call proven to reach a *native*
  cannot execute a `SetGlobal` at all.
