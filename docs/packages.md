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
helper_macros = { version = "0.1.0" }
route_macros = { version = ">=0.2.0, <0.4.0" }
local = { path = "deps/local" }

[registry]
name = "default"
url = "https://registry.lk.example"
include = ["Lk.toml", "src/**"]
```

By default, string dependencies are GitHub repositories. `owner/repo` resolves to
`https://github.com/owner/repo.git`.

Registry dependencies use `version = "x.y.z"` for exact versions or a semver
range such as `version = ">=0.2.0, <0.4.0"`, resolving through `[registry].url`
by default. A dependency can override the registry endpoint with
`registry = "https://registry.example"`.

For exact versions, `lk pkg fetch` requests
`GET [registry].url/api/v1/packages/<name>/<version>` and expects JSON with a
Git `source` URL/path, concrete `rev`, and optional `checksum`. For ranges, it requests
`GET [registry].url/api/v1/packages/<name>` and accepts either a JSON array of
versions or `{ "versions": [...] }`, where each version entry contains
`version`, `source`, `rev`, optional `checksum`, optional `yanked`, and optional
`publish_manifest`. LK validates any supplied publish manifest against the
package name, version, registry URL, and `integrity.sha256` digest before using
that registry version. It selects the highest non-yanked semver version
matching the range, fetches that Git source into the normal `$LK_HOME/git`
cache, checks out the resolved revision, verifies `sha256:<hex>` checksums when
provided, and records the locked revision plus checksum in `Lk.lock`.

`lk pkg index sync` downloads `[registry].url/api/v1/index` with
`X-LK-Registry-Scope: index` and stores a normalized cache at
`$LK_HOME/registry/<registry-name-or-url-key>/index.json` or
`~/.lk/registry/<registry-name-or-url-key>/index.json`. The index snapshot
contains package names, version entries with Git `source`, `rev`, optional
`checksum`, optional `yanked`, optional `publish_manifest`, and macro provider
metadata. Cached publish manifests are validated when the cache is read. This
cache is the local foundation for offline registry resolution and future
project-wide macro package indexing.

`lk pkg fetch --offline` and `lk pkg update [name] --offline` resolve registry
dependencies from that local index cache instead of sending registry HTTP
requests. Offline resolution supports exact versions and semver ranges, skips
yanked entries, then fetches/checks out the indexed Git `source` and `rev` with
the same checksum verification used by online fetch.

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

`lk pkg publish --dry-run` builds the registry publish manifest without a
network upload. It requires `[package].version` to be a semantic version and a
`[registry]` table with an `http://` or `https://` URL. The generated manifest
captures the package name/version, registry target, include globs, resolved
dependency version or revision metadata, macro provider names, and an
`integrity` object whose `sha256` digest is computed over the canonical
manifest payload without the `integrity` field. Registries can use that digest
as the immutable publish identity and signing preimage. `lk pkg publish`
recomputes the digest before upload and refuses a tampered manifest. It sends
the verified manifest as authenticated JSON to
`[registry].url/api/v1/packages` with `Authorization: Bearer <token>`, reading
the token from `LK_REGISTRY_PUBLISH_TOKEN`, `LK_REGISTRY_TOKEN`, or
`LK_PUBLISH_TOKEN`, in that order. Publish requests also send
`X-LK-Registry-Scope: publish`.

`lk pkg yank <name> <version>` marks a registry version as yanked through
`POST [registry].url/api/v1/packages/<name>/<version>/yank`. `lk pkg yank
<name> <version> --undo` reverses the yank through `DELETE` on the same
endpoint. Both commands use the same registry token environment variables as
publish, but prefer `LK_REGISTRY_YANK_TOKEN` before the shared fallback tokens,
and send `X-LK-Registry-Scope: yank`. Range resolution skips versions whose
registry entries have `yanked = true`.

`lk pkg index sync` uses the current package's `[registry]` table and does not
require an auth token by default; registries can still inspect the
`X-LK-Registry-Scope: index` header for scoped access policy.

`lk pkg key generate --out registry-key.json --key-id local-key` creates an
HMAC registry signing key file. `lk pkg key init-keyring --out
registry-keyring.json --key-id key-1` creates a keyring with an active signing
key; `lk pkg key rotate --keyring registry-keyring.json --key-id key-2` adds a
new active key while keeping previous keys trusted for existing signatures; and
`lk pkg key revoke --keyring registry-keyring.json --key-id key-1` marks an old
non-active key as revoked. `lk pkg key generate-asymmetric --private-out
registry-private.json --public-out registry-public.json --key-id ed-key`
creates an Ed25519 private/public signing key pair. `lk pkg serve --addr
127.0.0.1:3899 --storage
./registry --registry-url https://registry.lk.example --signing-keyring-file
registry-keyring.json` starts the built-in registry service and signs generated
publish/index/version responses with the active key. Clients can enforce
registry signatures with `LK_REGISTRY_SIGNING_KEYRING_FILE`, or use
`LK_REGISTRY_PUBLIC_KEY_FILE` with `--signing-private-key-file` for public-only
trust. `LK_REGISTRY_SIGNING_KEY_FILE` / `LK_REGISTRY_SIGNING_KEY_ID` plus
`LK_REGISTRY_SIGNING_SECRET` remain available for single-key local HMAC
testing. Server signing sources cannot be combined with each other.

`lk pkg serve --auth-policy registry-auth.json` enables scoped bearer-token
authorization for registry routes. The policy file is JSON:

```json
{
  "tokens": [
    { "token": "index-token", "scopes": ["index"] },
    { "token": "publish-token", "scopes": ["publish"] },
    { "token": "admin-token", "scopes": ["*"] }
  ]
}
```

When an auth policy is configured, `GET /api/v1/index`, `POST
/api/v1/packages`, and yank/unyank routes all require an `Authorization: Bearer
...` token with the matching `index`, `publish`, or `yank` scope. The older
`--token` flag remains available for local single-token publish/yank testing,
but it cannot be combined with `--auth-policy`.

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
- `lk pkg fetch` downloads Git dependencies and resolves exact or range-based registry `version` dependencies into `$LK_HOME/git` or `~/.lk/git`, then writes `Lk.lock`.
- `lk pkg fetch --offline` resolves registry `version` dependencies from `$LK_HOME/registry/<registry-name-or-url-key>/index.json`.
- `lk pkg update [name]` refreshes dependencies.
- `lk pkg update [name] --offline` refreshes dependencies using the local registry index cache.
- `lk pkg check` validates package graph and macro provider distribution metadata.
- `lk pkg publish --dry-run` validates registry publishing metadata and prints the publish manifest JSON with a `sha256` integrity digest.
- `lk pkg publish` verifies and uploads the publish manifest JSON, including its integrity digest, with `LK_REGISTRY_PUBLISH_TOKEN`, `LK_REGISTRY_TOKEN`, or `LK_PUBLISH_TOKEN`.
- `lk pkg yank <name> <version> [--undo]` yanks or un-yanks a registry package version.
- `lk pkg index sync` downloads and caches the registry package index.
- `lk pkg key generate --out <path> --key-id <id>` creates a JSON HMAC signing key for registry responses.
- `lk pkg key generate-asymmetric --private-out <path> --public-out <path> --key-id <id>` creates an Ed25519 private/public signing key pair.
- `lk pkg key init-keyring --out <path> --key-id <id>` creates a JSON HMAC signing keyring.
- `lk pkg key rotate --keyring <path> --key-id <id>` adds a new active signing key to a keyring.
- `lk pkg key revoke --keyring <path> --key-id <id>` revokes an old non-active key in a keyring.
- `lk pkg serve --storage <dir> --registry-url <url> [--auth-policy <path>] [--signing-keyring-file <path>|--signing-private-key-file <path>]` runs a local registry server.
- `lk pkg tree` prints resolved package modules.
