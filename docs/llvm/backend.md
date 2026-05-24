# LLVM Backend

The current LLVM target uses `Module32Artifact` as its input boundary. A small
native-lowerable subset of entry functions can return an `i64`, `f64`, `bool`,
`nil`, short string, long string literal, simple const list, or simple const map
from scalar loads, integer/float arithmetic, integer and float comparisons,
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

Those scalar returns are printed through `printf` using the same user-facing
spellings as the VM path for the covered values. Unsupported shapes are rejected
with a compile error; LLVM output must not embed a serialized `.lkm` payload or
call back into the Instr32 VM.

Native integer and float division/modulo preserve the VM divisor-zero boundary:
static folding refuses zero divisors, and scalar block lowering emits a
divisor-zero guard instead of directly relying on LLVM `sdiv`, `fdiv`, or `frem`
semantics.

Executable output uses the same `Module32Artifact` boundary:

```sh
lk compile exe FILE.lk
```

For native-lowerable shapes, the CLI compiles the direct LLVM IR to a native
executable with `clang`. Unsupported shapes fail before executable emission; the
CLI no longer generates a host executable launcher.

Future native AOT work must continue expanding this `Module32Artifact` lowering
surface without adding a second runtime bridge.
