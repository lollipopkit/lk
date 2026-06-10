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
