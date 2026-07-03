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
other = { git = "https://git.example/other.git", rev = "a1b2c3d" }
local = { path = "deps/local" }
```

LK uses **decentralized git+lockfile dependencies** (Deno/Go style): every
dependency is a git repository, and there is no central registry to run, publish
to, or sign against. By default, string dependencies are GitHub repositories —
`owner/repo` resolves to `https://github.com/owner/repo.git`. The detailed form
accepts `github`, `git` (any git URL), or `path` (a local directory), plus an
optional `branch` / `tag` / `rev` to pin a revision.

`lk pkg fetch` clones each git/GitHub dependency into the `$LK_HOME/git` (or
`~/.lk/git`) cache, checks out the requested `branch`/`tag`/`rev` when given, and
records the resolved `HEAD` revision in `Lk.lock` so builds are reproducible.
`path` and workspace dependencies are local and need no fetch. `lk pkg update
[name]` re-resolves one or all dependencies.

## Procedural Macro Providers

`Lk.toml` can register isolated process providers for procedural macros. The
compiler sends a versioned JSON request on stdin and expects a versioned JSON
response on stdout. Commands that look like paths are resolved relative to the
manifest directory; plain command names resolve through `PATH`.

```toml
[macros]
trusted_dependencies = ["helper_macros"]

[macros.derive.MakeAnswer]
command = "./tools/derive-make-answer"
args = ["--json"]
timeout_ms = 5000
max_output_bytes = 1048576

[macros.attribute.route]
command = "lk-route-macro"

[macros.function_like.sql]
command = "lk-sql-macro"
```

External derive providers append generated items after the annotated struct.
External attribute providers can transform, replace, or remove a single
annotated item. Function-like providers expand `name!(...)`, `name![...]`, and
`name!{...}` invocations to token streams before normal parsing. Provider
responses can report deterministic dependency metadata; `lk macro expand --deps`
prints the collected dependencies as JSON.

Dependency metadata participates in cache invalidation. LK fingerprints each
reported `path`/`digest` pair plus the resolved file state when the dependency
path is readable. Direct native execution writes a `.proc-macro-deps.json`
sidecar beside cached native executables and rebuilds stale entries. The LSP
workspace cache stores the same fingerprint and drops preloaded analysis when a
macro dependency file changes or appears after being missing.

Providers declared by dependencies are not executed automatically. A package must
opt in with `[macros].trusted_dependencies`, naming each dependency whose
provider commands may run. Trusted dependency function-like providers are
available through the dependency namespace, for example
`helper_macros::sql!("select 1")`. Trusted dependency derive and attribute
providers use their declared names because current derive/attribute syntax is not
path-shaped; providers declared by the current package win name collisions.

Run `lk pkg check` before publishing or sharing a macro package. It validates the
package graph, provider macro names, path-like provider command paths,
`timeout_ms` / `max_output_bytes` bounds, and `[macros].trusted_dependencies`.
Trusted dependencies must resolve to package/workspace members and declare at
least one derive, attribute, or function-like provider.

To share a macro package, publish it as a git repository and depend on it by git
URL — there is no central publish/registry step. `lk pkg check` validates the
package before you push.

Expanded token streams keep token-level macro origins for declarative macro
captures, macro-definition output, `$crate` anchors, and function-like
procedural macro output. Post-parse derive/attribute/cfg expansion also records
item-level AST macro origins. `lk macro expand --origins` prints both source-map
sets as JSON; parse errors caused by macro-generated tokens use the same origin
stack to explain which macro call produced the token.

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

- `lk pkg init [name]` creates a package.
- `lk pkg add <name> <owner/repo> [--tag v1] [--branch main] [--rev SHA]` adds a dependency.
- `lk pkg fetch` clones git/GitHub dependencies into `$LK_HOME/git` or `~/.lk/git` and pins their resolved revisions in `Lk.lock`.
- `lk pkg update [name]` re-resolves one or all dependencies.
- `lk pkg check` validates package graph and macro provider distribution metadata.
- `lk pkg tree` prints resolved package modules.
