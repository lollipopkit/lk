# LK Standard Library

LK stdlib modules are Rust crates registered through `ModuleProvider`. Each
top-level module has its own crate under `stdlib/crates/`; parent namespaces
such as `io`, `net`, and `encoding` expose child namespaces through runtime
exports.

## Module Boundaries

- `fs` owns path-level filesystem operations such as `read`, `write`,
  `metadata`, `read_dir`, and removal/rename/copy helpers.
- `io.file` owns opened `File` resources: `open`, `create`, `read`,
  `read_to_string`, `write`, `flush`, and `close`.
- `os` is intentionally narrow: platform and clock helpers only.
- `env`, `path`, and `process` split out environment lookup, path manipulation,
  and process execution/state.
- `encoding` is a parent namespace for data formats and byte/text encodings:
  `json`, `yaml`, `toml`, `base64`, `hex`, and `url`.
- Concurrency is Go-shaped (see `docs/concurrency.md`): the `go` statement /
  `spawn` global start goroutines, `chan` owns channel operations, and
  `task` owns task management (`await`, `try_await`, `join_all`, `sleep`).
  Failures raise (v2 error model) — there are no `[ok, value]` pairs.

## Common Modules

- `hash`: `sha256`, `sha1`, `crc32`, `fnv64`.
- `regex`: match, find, captures, replace, and split helpers.
- `random`: integers, floats, booleans, bytes, list choice, and list shuffle.
- `uuid`: UUID v4 generation and validation.
- `http`: synchronous client returning `{status, headers, body}` maps.

## Rust Module Exports

Stdlib modules use the derive/attribute macro pair from `lk_stdlib_common`.
Put `#[stdlib_export]` on each exported function so runtime exports, catalog
metadata, and LSP hover stay next to the implementation:

```rust
#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "env", docs = "Environment variable helpers")]
pub struct EnvModule;

#[lk_stdlib_common::stdlib_exports]
impl EnvModule {
    #[stdlib_export(
        name = "get",
        params(key: String),
        returns = String?,
        docs = "Returns an environment variable, or nil if it is not set."
    )]
    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        /* native implementation */
    }
}
```

The macro generates `ModuleProvider`, `register`, `metadata`, runtime exports,
and LSP hover metadata from the exported methods. For runtime builtins such as
`task.spawn` or `time.sleep`, add `runtime_builtins = true`:

```rust
#[lk_stdlib_common::stdlib_exports(module = "time", runtime_builtins = true)]
impl TimeModule {
    #[stdlib_export(params(ms: Int | Float), returns = Nil)]
    fn sleep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        /* native implementation */
    }
}
```

Simple fixed-arity functions can use the ergonomic ABI. The macro emits a
zero-allocation wrapper that checks arity once and passes indexed values:

```rust
#[stdlib_export(params(source: String), returns = Slice)]
fn from_string(source: RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    /* native implementation */
}
```

Variadic, named-argument, or full-state functions should keep the raw
`NativeArgs` ABI. Nested namespace modules such as `io.std`, `net.tcp`, and
`encoding.json` use the same per-function exports. Parent namespaces declare
children on `#[stdlib_exports]` so the macro wires the runtime map and registers
child metadata:

```rust
#[lk_stdlib_common::stdlib_exports(
    children(json = JsonModule, yaml = YamlModule, toml = TomlModule)
)]
impl EncodingModule {}
```

For named parameters, list accepted names explicitly and keep semantic
validation in the native implementation:

```rust
#[stdlib_export(
    params(value: Int, min?: Int = 0, max?: Int = 100),
    named(min, max),
    returns = Int
)]
fn clamp(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    /* native implementation */
}
```

## Named Parameters

A parameter is declared `named(...)` when its **position is not enough to say
what it means**. Three shapes qualify, and the rule is mechanical enough that
`every_ambiguous_parameter_is_named` enforces the first one:

1. **Two or more parameters of the same type**, past the first. Nothing at the
   call site distinguishes them, so swapping them is silent — the program keeps
   running and answers something else.
2. **A boolean switch.** `true` at a call site says nothing about which
   behaviour it selects.
3. **An optional parameter whose absence changes what happens**, rather than
   merely supplying a default.

The first parameter — the thing being operated on — is never named.
`string.len(text: s)` is noise; the subject of a call is what a call is about,
and its position says so.

Named parameters are **optional at the call site**: `math.clamp(5, 1, 3)` and
`math.clamp(5, min: 1, max: 3)` are both accepted, so declaring them breaks
nothing and only gives the caller a way to be explicit.

### Why the rule earns its keep

These three operations are siblings, and the same `(2, 3)` means three things:

```lk
"abcdef".substring(2, 3)        // "cde"  — the third argument is a *length*
[1,2,3,4,5,6].slice(2, 3)       // [3]    — the third argument is an *end*
bytes.slice(b, 2, 3)            // "c"    — the third argument is an *end*
```

No amount of care at the call site distinguishes `substring(s, 2, 3)` from
`slice(xs, 2, 3)`; only the declaration knows, and only a name carries the
declaration to where the code is read. `substring(s, start: 2, length: 3)`
cannot be misread.

(That `substring` counts a length while its two siblings take an end is a
separate question — a real inconsistency, and changing it would change what
existing programs compute. Naming the parameter makes the current answer
legible; it does not decide the larger question.)

## Examples

```lk
use fs;
use bytes;
use { file } from io;
use { json, base64 } from encoding;

let raw = fs.read("config.json");
let text = bytes.to_string_utf8(raw);
let cfg = json.parse(text);

let out = file.open("out.txt", "write");
file.write(out, base64.encode(text));
file.close(out);
```

Top-level `json`, `yaml`, and `toml` modules are removed. Use
`use { json, yaml, toml } from encoding;` instead.
