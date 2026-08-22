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
| `Imported(RuntimeCallable)` | function index in the callable's own module | that module's own `Arc<Mutex<RuntimeModuleState>>`, with arguments and the result marshalled across heaps — unless that module is already executing, see below |

### `Imported` when the module is already on the stack

Borrowing a module's state means *moving* it out of its mutex until the call
returns, so nothing can enter that module again in the meantime. Two ordinary
programs do exactly that:

```lk
// shape.lk — a method calling another method on self
impl Sq { fn area(self) -> Int { return self.side * self.side; }
          fn twice(self) -> Int { return self.area() * 2; } }
```

```lk
// a.lk — out to another module and back
impl A { fn base(self) -> Int { return self.v; }
         fn viab(self) -> Int { return helper(self); } }   // helper() calls x.base()
```

Both failed with `module expected 83 globals, got 0`: the re-entering call got
the `Default::default()` placeholder left in the mutex, which is indistinguishable
from a real state that happens to be empty. The placeholder now carries
`borrowed_for_call`, so "in use" is something it says about itself rather than
something a later length check infers.

Knowing that, the two cases take the two paths that already existed:

- **The module is the one executing** (`twice` → `self.area()`) — no borrowing is
  needed at all; the live state *is* that module's. Runs like a local closure.
- **The module is elsewhere on the stack** (`viab` → `helper` → `base`) — run it
  the way any foreign body is run, below: current heap, globals seeded to the
  declaring module's shape. That needs the module, not the module's state.

A body that *writes* a module global still cannot take the second path, and is
refused by name rather than writing into a table that is about to be discarded.

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

## A function value crossing a module boundary (2026-07-31)

`apply(double, 5)`, where `apply` came from another file, is the same question
asked about an ordinary function rather than an impl method — and it used to be
refused outright, because a bare closure is a `function_index` into *its own*
module's table.

It is now promoted at the crossing: the value becomes a `RuntimeCallable`
carrying its defining module, so the index still means what it meant. The
executor is the only place that knows which module the arguments come from, so
it is what supplies it (`ClosureCopy::Promote`); a crossing that cannot name a
source module — a channel payload, a stdlib HOF re-entering the VM — still
refuses, and says that is why.

What the promoted callable does **not** get is that module's live state: the
caller's state belongs to a frame further down the Rust stack and cannot be
taken while it is running. It gets a fresh, empty one instead — arguments copied
in, result copied out, which is what every `RuntimeCallable` call already does.

That leaves the globals, and the same three answers as above, from the same
walk (`analysis::function_global_use`, one implementation for both callers):

- reads a global → refused, naming the global (a fresh state has nil there, and
  nil is a wrong answer, not a slow one);
- writes a global → refused (the write would land in a table nobody reads);
- makes a call the walk cannot follow → refused **as its own case**, not as a
  write. `println` is the everyday one. Nothing is known to be wrong there, only
  unproven, and reporting a write would be a guess stated as a fact.

The captures come along, copied into the callable's own heap, so a capturing
`|x| x + n` crosses too.
