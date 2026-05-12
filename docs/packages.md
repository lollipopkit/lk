# LKR Packages and Workspaces

LKR packages use `Lkr.toml` and `Lkr.lock`, modelled after Cargo manifests.

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

Workspace members are packages with their own `Lkr.toml`. A member package is
imported by its package name.

## Module Roots

Package imports resolve to:

1. `src/mod.lkr`
2. `src/<package-name>.lkr`

Example:

```lkr
import util;
return util.answer();
```

File imports such as `import "foo";` remain relative to the current file.

## CLI

- `lkr init [name]` creates a package.
- `lkr pkg add <name> <owner/repo> [--tag v1] [--branch main] [--rev SHA]` adds a dependency.
- `lkr pkg fetch` downloads dependencies into `$LKR_HOME/git` or `~/.lkr/git` and writes `Lkr.lock`.
- `lkr pkg update [name]` refreshes dependencies.
- `lkr pkg tree` prints resolved package modules.
