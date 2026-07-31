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

## What the fuzzer still cannot reach

Its vocabulary is fixed, so it finds regressions in shapes it already knows, not
new categories. Known gap: every list it generates is a `List<Int>`, so it
cannot produce two different list carriers both flowing into functions that
mutate them — which is the shape of the open typed-list boxing divergence
recorded in `docs/semantics.md`. Adding a second carrier is worth doing *after*
that is fixed; until then it would only make the gate red.
