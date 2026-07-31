# Gates: what each one is the only thing that catches

`cargo test --workspace --all-features` is the source of truth for unit and
integration tests, and it is **not** the whole gate set. Several checks live
outside it, each because it answers a question the others structurally cannot.
This page exists because a green `cargo test --workspace` was once read as "all
gates pass" and a regression shipped past it.

## The set

| Gate | Command | What only this catches |
| --- | --- | --- |
| Workspace tests | `cargo test --workspace --all-features` | Everything with a named test. |
| Format | `cargo fmt --all -- --check` | — |
| Lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | CI injects `RUSTFLAGS=-D warnings`, so **test-target** warnings fail CI; a plain `cargo clippy --workspace` does not compile tests. |
| Lint, `no_std` faces | `cargo clippy -p lk-core --no-default-features --all-targets -- -D warnings`, same for `-p lkrt` | `--all-features` never compiles the `no_std` face; the bare-metal targets do. |
| `no_std` build | `cargo build -p lk-core --no-default-features` | A `use` deleted from under its `#[cfg(feature = "std")]` makes the *next* item std-only, silently. |
| LK source formatting | `lk fmt --check` | 36 of 97 `.lk` files were not in the shape the tool produces — the feature shipped and no workflow ran it. |
| AOT native-lowering coverage | `AOT_COVERAGE_REQUIRE_FULL=1 bash scripts/aot_coverage.sh` | A program that stops lowering natively still prints the right answer, ~3x slower. **No differential test can see it.** |
| AOT differential suites | `cargo test -p lk-cli --test aot_differential_test --test clif_differential_test --test hybrid_compile_test` | VM vs. native disagreement on the pinned corpus. |
| Generative differential fuzz | see below | Feature *combinations* nobody wrote a case for. |
| Bare metal (ARM, x86) | `cd bare-metal && LK_BIN=… cargo run --release`, `cd bare-metal-x86 && LK_BIN=… python3 check_pci.py` | That the `no_std` VM *works*, not merely compiles. Note `LK_BIN`: the build defaults to the **installed** `lk`, not the one you just built. |
| Performance | `cargo build --profile dist -p lk-cli` then `bench/run_workload_bench.sh` | A hard 10% geomean gate; see `bench/README.md`. |

## Generative differential fuzz

`cli/tests/aot_fuzz_differential_test.rs` builds random programs that combine
containers, closures, `try`, `defer`, hybrid helpers and cross-function calls,
runs them under the VM and natively, and compares stdout, exit status and
stderr. It is the only gate that *combines* features rather than testing one at
a time.

It runs in its own CI workflow (`.github/workflows/correctness.yml`: 500 cases
under ASan/UBSan, plus a second job seeded with the run id), **not** in
`cargo test --workspace`, and its seed comes from the environment:

```bash
LK_FUZZ_SEED=44 cargo test -p lk-cli --test aot_fuzz_differential_test        # one seed, ~30s
LK_FUZZ_CASES=200 LK_FUZZ_SEED=987654 cargo test -p lk-cli --test aot_fuzz_differential_test
```

**Run several seeds after touching `aot/lower/src/{lib,sig,function}.rs`.** A
change to the fixpoint's parameter lattice made three of five seeds fail with

```
Cranelift codegen failed: call to lk_fn_6 passes 2 machine argument(s), declared with 3
```

while the workspace tests, the coverage gate, the 61-program VM/native sweep,
both bare-metal acceptances and the perf gate were **all green**. The parameter
observation table also decides a callee's rendered arity (an erased closure's
environment and captures observe through it), which is exactly the kind of
second job a single-feature test does not exercise.

A failure prints the seed and the full generated program, so reproduction is
`LK_FUZZ_SEED=<seed> cargo test -p lk-cli --test aot_fuzz_differential_test`.

## 计时断言:单样本是硬币,松预算是摆设

两种坏法都在这个仓库里出现过,而且互为对方的"修法":

- **单样本 + 紧预算 = flaky。** `compiling_many_functions_stays_linear` 断言
  `large < small * 3`,在 `cargo test --workspace --all-features`(几十个测试线程
  抢核)里失败过一次,单跑连过 5 次。一条会因为**别的原因**变红的门禁比没有门禁
  更糟 —— 它教会所有人重跑,而重跑的习惯一旦养成,真回归也会被重跑掉。
- **单样本 + 松预算 = 摆设。** `lsp/tests/perf_latency_test.rs` 的 6 条延迟断言,
  余量是 67x 到 **2381x**(`semantic_tokens(example workspace main)` 实测 21µs,
  预算 50ms)。比被测量高三个数量级的预算不可能失败,所以它什么也没说 —— LSP 慢
  10 倍(交互工具"跟手"和"不跟手"的分界)六条全过。

两种都源于同一个选择:**用一次墙钟采样做判据**。噪声让你不敢收紧预算,松预算又
让断言失去意义。

规矩:**取 N 次里的最小值,再把预算收到观测值的约 10 倍。** 最小值是这里正确的估
计量 —— 调度噪声、缺页、降频只会让某次更慢,所以最小的样本最接近被测的工作量。噪
声去掉之后,预算才敢收到"能抓住 10 倍回归、抓不到机器间差异"的位置。

配套两条:

- 把**观测值和日期**写在预算旁边(`// Observed 3.3ms (debug, 2026-08-01).`),
  预算漂成摆设时看得见。
- 新加或收紧一条计时断言之后**反向验一次**:把预算调到实测值以下,确认它真的会
  红。一条从没红过的断言和一条不可能红的断言,从外面看是一样的。

## What the fuzzer still cannot reach

Its vocabulary is fixed, so it finds regressions in shapes it already knows, not
new categories. Known gap: every list it generates is a `List<Int>`, so it
cannot produce two different list carriers both flowing into functions that
mutate them — which is the shape of the open typed-list boxing divergence
recorded in `docs/semantics.md`. Adding a second carrier is worth doing *after*
that is fixed; until then it would only make the gate red.
