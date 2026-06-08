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
