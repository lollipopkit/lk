# LLVM Backend

The current LLVM target uses `Module32Artifact` as its input boundary. A small
native-lowerable subset of entry functions can return an `i64`, `f64`, `bool`,
`nil`, short string, long string literal, simple const list, or simple const map
from scalar loads, integer/float arithmetic, integer and float comparisons,
mixed integer/float arithmetic promotion for float opcodes,
simple `Test` / `Jmp` control flow including source-level conditional, match with range/guard/or patterns,
range `for` including inclusive/negative-step ranges and `break` / `continue`, static-list `for`, static-string `for`, and static-map entry `for` expressions/statements, static template string `ToString` / `ConcatString` chains, static nullish/logical short-circuit expressions, static
int/float/string comparison folding, bool/nil `Not` and equality checks, static const list/map shape checks, static string/list/map
length, static string/list/map `GetIndex`, static string/list `SliceFrom`, static string/list/map `Contains`,
static list/map equality, static object identity equality, static map rest
destructuring including source-level static `if let` list/map destructuring, static object construction and field access, static list/map/object
`SetIndex`, static `NewList` / `NewMap` / `NewRange` construction,
simple scalar module global slots used by top-level variables, static scalar and statically
displayable string/list/map truthiness branches, static success-path `TryBegin` / `TryEnd`, static truthiness `Not`, and
straight-line static string globals and string equality checks.
Control-flow block lowering also supports the covered template-string equality
shape where dynamic `Text` parts are compared against a static string literal,
including dynamic string and integer interpolations, plus static string/list
`Contains` inside branchy source such as assertions.
Straight-line entry functions can also
inline simple direct function calls when positional arguments and the callee
return value stay within those statically displayable values, including
caller-side f64/bool/nil/string arguments, callee-local i64/f64 arithmetic,
callee i64/f64 comparisons, callee static string equality, callee static
list/map equality, callee static string/list `SliceFrom`, callee static
string/list/map/object `GetIndex`, string/list/map `Contains`, callee static `SetIndex`, callee static `NewList` / `NewMap`,
static `CallNamed` with fully supplied named arguments, reads from
static module globals, callee static `MapRest` / `NewRange`, and callee writes back to
static module globals, callee static i64 branch selection, callee static
truthiness branch selection, static closure construction with `UpvalCell` capture loads/stores,
entry/callee static `TryBegin` / `TryEnd` success paths, static handler-local
`Raise`, source-level static optional access, and callee `bool` / `nil` returns.
For that covered subset,
`lk compile llvm FILE.lk` writes `FILE.ll` with a direct native `main`.
Native `print` / `println` lowering covers static formatted calls with multiple
`{}` placeholders when the argument values remain statically represented by the
block compiler.
Native stdlib lowering includes static OS string helpers for
`os.hostname()`, `os.arch()`, `os.os()`, `os.dir_current()`, `os.dir_temp()`,
`os.dir_list(path)`, plus `os.env.get(name[, default])`, `os.clock()`, and
`os.epoch()`. These are lowered directly as native constants or C runtime calls
for the currently covered scalar/string shapes.
Static direct-call folding may also recover immutable heap-const list arguments
inside the same basic block, which covers statically bounded recursive list
methods such as `list.skip(1)` without lowering through the VM runtime.
Self-recursive function hinting tries scalar and list-like parameter profiles, so
recursive helpers with mixed signatures such as `contains(List<Int>, Int) ->
Bool` can be classified without falling back to the VM runtime.
Dynamic native containers are represented as monomorphized layouts instead of a
tagged runtime value. The current covered layouts include `List<i64>`,
`List<f64>`, `List<bool>`, pointer/text lists, text-length lists used by joins,
`Map<str,i64>`, `Map<str,f64>`, `Map<str,bool>`, `Map<str,str>`,
`Map<i64,i64>`, `Map<i64,f64>`, `Map<i64,bool>`, and `Map<i64,str>`;
static string lists can be indexed with a dynamic `i64` index and lower to
direct string pointer selection. Covered dynamic lists, dynamic pair lists
including `StrPtr,F64` and `I64,F64` field layouts, and dynamic maps can be
displayed as direct returns and as nested `ArgList` return elements without
falling back to the Instr32 runtime.
String-valued dynamic map writes copy runtime text through `strdup` before
storing into ptr slots, so loop-local template buffers do not alias later
iterations.
For `Map<i64,i64/f64/bool/str>` and `Map<str,i64/f64/bool/str>`, `map.has` and
`map.delete` also lower natively: delete materializes a fresh dynamic map
storage for the returned `without` map and preserves the removed value. Missing
dynamic map `get` results carry a present bit for integer, float, bool, and
string pointer values, including receiver-method calls such as
`without.get("missing")`, so nested returns print `nil` instead of the zero
value. Dynamic `Map<str,str>` also has ptr-value set, direct index, values,
display, and missing `get` lowering, with runtime text copied through `strdup`
before it is stored. Optional scalar map-get results can be recovered into
`ArgList` returns without falling back to the Instr32 runtime.
`DynamicList<i64>` and `DynamicList<bool>` also support monomorphized
`list.contains`, `list.index_of`, `list.reverse`, `list.pop`, `list.push`,
`list.slice`, `list.insert`, `list.remove_at`, and `list.set` lowering through
i64-slot helpers while preserving element-specific return/display shape,
including nested `[new_list, old_value]` returns. `List<bool>` also lowers
receiver `concat([false])` and module `list.sort(xs)` through the same i64-slot
ABI, with bool-specific display shape preserved at returns.
`DynamicList<f64>` and dynamic pointer/string lists support the same module
mutator family from register-recovered builtin calls, including `list.slice`,
`list.insert`, `list.remove_at`, `list.set`, and `list.push`; f64 paths use
double-list helpers and string paths use ptr-list helpers. This path is intentionally
kept distinct from static `List<i64>` module folding: only storage rooted at
`NewList`, `ListPush`, or an empty `LoadHeapConst []` is treated as mutable
dynamic storage, so non-empty static heap lists continue to use static module
helper folding.

Non-nil scalar returns are printed through `printf` using the same user-facing
spellings as the VM path for the covered values. A nil return is silent, matching
the CLI VM path. Unsupported shapes are rejected with a compile error; LLVM
output must not embed a serialized `.lkm` payload or call back into the Instr32
VM.

Native integer and float division/modulo preserve the VM divisor-zero boundary:
static folding refuses zero divisors, and scalar block lowering emits a
divisor-zero guard instead of directly relying on LLVM `sdiv`, `fdiv`, or `frem`
semantics.

Control-flow scalar block lowering must treat static branch facts as path-local.
When a statically known `Test` has an untaken target that is also a merge point,
the merge remains reachable and must not be skipped. For values loaded before a
control-flow boundary, the backend may recover narrowly proven immutable shapes
such as heap-const integer lists for native list indexing and membership checks,
but it must not preserve arbitrary mutable register facts across branches.

Executable output uses the same `Module32Artifact` compile-time boundary:

```sh
lk compile exe FILE.lk
```

For native-lowerable shapes, the CLI compiles the direct LLVM IR to a native
executable with `clang` and links the typed `lkrt` native runtime static
library. Unsupported shapes fail before executable emission; the CLI no longer
generates a host executable launcher.

Future native AOT work must continue expanding this `Module32Artifact` lowering
surface without adding a VM runtime bridge. The final executable may link Rust
`std`, libc/libm, and `lkrt`, but it must not embed a serialized `.lkm` payload,
`Module32Artifact`, the Instr32 executor, `VmContext`, parser, type checker, or
compiler. See `docs/llvm/native-stdlib.md` for the native stdlib boundary.
