# LK Packages and Workspaces

LK packages use `Lk.toml` and `Lk.lock`, modelled after Cargo manifests.


## `lk pkg add` 按形状认来源,认不出就当场拒绝

```sh
lk pkg add dep owner/repo                  # GitHub
lk pkg add dep https://gitlab.com/a/b.git  # 任意 git 主机
lk pkg add dep ../dep                      # 本地包
```

`<SOURCE>` 此前被原样写成 GitHub 仓库名,于是 `lk pkg add dep ../dep` 写出
`dep = "../dep"`,失败在很久以后才由 git 报出来:
`repository 'https://github.com/../dep.git/' not found`。清单从一开始就有 `path`
和 `git` 两种写法,只是 `add` 拼不出来。

判据:含 `://` 或 `git@` 开头 → git URL;`./`、`../`、`/`、`~` 开头 → 本地路径;
恰好一个 `/` 且两边非空且无空白 → GitHub `owner/repo`;其余**当场拒绝**并列出这
三种写法。`--branch/--tag/--rev` 用在本地路径上也拒绝 —— 本地包没有 revision 可
钉,清单里留着它只会让人以为钉住了。

## 未解析的依赖要说清是哪一种

`lk pkg check` / `lk pkg tree` 此前对所有未解析依赖都印
"`<missing; run lk pkg fetch>`"。对 `path` 依赖那是**做不到的建议** —— 目录就在
那儿。现在分三种:

- `not fetched; run lk pkg fetch` —— git/GitHub 依赖还没取下来。
- `the path points at a directory that does not exist` —— `path` 指向的目录不存在,
  fetch 造不出来。
- `found, but the package has no library entry; add src/mod.lk (or src/<name>.lk)`
  —— 目录在,缺的是**库入口**。注意 `lk pkg init` 生成的是 `src/main.lk`,那是
  *应用*入口;一个要被别人依赖的包需要 `src/mod.lk` 或 `src/<name>.lk`。

`lk pkg add` 写出的清单也不再带 `workspace = false` —— `Lk.toml` 是给人读和改的
文件,每条依赖上挂一个什么都没说的字段是噪声。

## `lk compile` 在有依赖的包里会失败,而且是在编译期说清楚

`lk compile` 先试原生降低;降不下来就回落到 **Tier 0 打包**(把程序源码和 VM 一
起塞进一个可执行文件)。而 Tier 0 只塞**一个文件**:被 `use` 进来的模块源码从来
没被打进去。于是一个有依赖的包"编译成功",跑起来是

    lk: execution failed

现在两件事都修了:

- **打包前就拒绝**:程序里只要有 `use "路径"` 或非 stdlib 的 `use 名字`,
  `lk compile` 当场报错,说明 Tier 0 只带一个文件、建议 `lk 文件` 直接跑或把程序
  写成一个文件。stdlib 的 `use math;` 不受影响 —— 打进去的 VM 自带整个标准库。
- **打包出来的二进制会说原因**:`lk_vm_eval` 出错时只返回 NULL,消息被丢掉,所以
  wrapper 只能印 "execution failed"。C ABI 新增 `lk_vm_last_error(vm)`(借用 VM
  里的字符串,不用 free),wrapper 改印真实原因,例如 `lk: Module 'dep' not found`。

顺带一条语言事实:**LK 没有 `pub`**,模块里定义的东西默认全部导出。写
`pub fn f()` 是语法错误。

## 构建产物放在包根,不放进 `src/`

```sh
cd my-pkg
lk compile            # -> my-pkg/my-pkg
lk compile bytecode   # -> my-pkg/my-pkg.lkm
```

`lk compile` 不带 FILE 时会把入口解析成 `<包>/src/main.lk`,而输出路径此前是"入
口去掉扩展名" —— 于是一个几十 MB 的可执行文件(以及 `.lkm`)被丢进**源码目录**,
就躺在它编译自的那个文件旁边,下一次 `git add .` 顺手就提交了。

现在:**由清单解析出入口的构建**(即包构建),产物放在包根,名字取包目录名 ——
和 `go build` 把二进制放进模块目录而不是 `src` 下面是同一个规矩。**用户点名了文
件**的构建保持原样(`lk compile foo.lk` → `foo`),点名一个文件本来就意味着"就放
它旁边";`./main.lk` 这种散文件同理。`--output` 永远优先。


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
