# LK Packages and Workspaces

LK packages use `Lk.toml` and `Lk.lock`, modelled after Cargo manifests.

## Package Manifest

```toml
[package]
name = "app"
version = "0.1.0"
edition = "2026"

[dependencies]
util = "owner/repo"
math_ext = { github = "owner/math-ext", tag = "v0.1.0" }
local = { path = "deps/local" }
```

By default, string dependencies are GitHub repositories. `owner/repo` resolves to
`https://github.com/owner/repo.git`.

## Workspaces

```toml
[workspace]
members = ["crates/*"]

[workspace.dependencies]
util = { path = "crates/util" }
```

Workspace members are packages with their own `Lk.toml`. A member package is
imported by its package name.

See `examples/lk-example-workspace` for a runnable workspace with one app and
two member packages (`mathlib` and `greetings`).

## Module Roots

Package imports resolve to:

1. `src/mod.lk`
2. `src/<package-name>.lk`

Example:

```lk
use util;
return util.answer();
```

File uses such as `use "foo";` remain relative to the current file.
They do not require `Lk.toml`; use them for files under the importing file's
directory. File uses are still explicit: files are not automatically visible
to each other.

Parent-directory imports are intentionally rejected. For example, from
`src/nested/test.lk`, `use "../root";` is invalid. If nested code needs to
depend on code outside its subtree, make that code a package/workspace member and
use a bare package use instead:

```lk
use util;
return util.answer();
```

## CLI

- `lk init [name]` creates a package.
- `lk pkg add <name> <owner/repo> [--tag v1] [--branch main] [--rev SHA]` adds a dependency.
- `lk pkg fetch` downloads dependencies into `$LK_HOME/git` or `~/.lk/git` and writes `Lk.lock`.
- `lk pkg update [name]` refreshes dependencies.
- `lk pkg tree` prints resolved package modules.
