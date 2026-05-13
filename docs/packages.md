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

## Module Roots

Package imports resolve to:

1. `src/mod.lk`
2. `src/<package-name>.lk`

Example:

```lk
import util;
return util.answer();
```

File imports such as `import "foo";` remain relative to the current file.

## CLI

- `lk init [name]` creates a package.
- `lk pkg add <name> <owner/repo> [--tag v1] [--branch main] [--rev SHA]` adds a dependency.
- `lk pkg fetch` downloads dependencies into `$LK_HOME/git` or `~/.lk/git` and writes `Lk.lock`.
- `lk pkg update [name]` refreshes dependencies.
- `lk pkg tree` prints resolved package modules.
