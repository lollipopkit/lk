# LK VM 重构交接进度

本文只记录当前快照、已验证事实、未完成风险和下一步执行顺序。`plan.md` 是架构契约，不写日常流水账；本文也要保持短小，避免旧 session 历史压过当前事实。

## 当前总体状态

主线已经从旧 VM 兼容迁移转为新架构收口，同时进行 LLVM true native AOT 修复。

### 架构完成度（对照 plan.md 12 项执行顺序）

| # | 项目 | 状态 |
|---|------|------|
| 1 | RuntimeVal/HeapValue/Callable/typed container | ✅ 已建立 |
| 2 | Slot-based HeapStore, GC 标记遍历 | ✅ 已建立 |
| 3 | Instr32, typed const pool, encoder/decoder | ✅ 已建立 |
| 4 | Compiler + Executor 最小可运行 | ✅ 完整端到端 |
| 5 | literal/load/move/arithmetic/branch/closure/return | ✅ 已覆盖 |
| 6 | shared stack call ABI | ✅ 已覆盖 |
| 7 | closure upvalue cell | ✅ 已覆盖 |
| 8 | per-task STW GC | ✅ 已覆盖 |
| 9 | VM handler stack, TryBegin/TryEnd/Raise | ✅ 已覆盖 |
| 10 | global/context slot, typed container fast path | ✅ 已覆盖 |
| 11 | legacy 残留清理 | 🔵 已审计，无旧 Op/BC32/quickening/Frame32 残留 |
| 12 | LLVM true native AOT | 🟡 进行中 |

### 当前已验证项

- `cargo test -p lk-core --lib` → **732 passed, 0 failed** ✅
- `cargo test -p lk-core --features llvm llvm::tests:: -- --nocapture` → **206 passed, 0 failed** ✅
- `cargo test -p lk-stdlib` → **113 passed, 0 failed** ✅
- `cargo build -p lk-cli --features llvm` → **通过** ✅
- `cargo check -p lk-core --features llvm` → **通过** ✅
- `cargo check -p lk-cli --features llvm` → **通过** ✅
- `target/debug/lk compile exe /tmp/wc_until_sort.lk --output /tmp/wc-until-sort && /tmp/wc-until-sort` → **通过** ✅
- `target/debug/lk compile exe examples/general/word_count.lk --output /tmp/lk-word-count && /tmp/lk-word-count` → **输出 `word_count: all assertions passed`** ✅
- `target/debug/lk compile exe examples/stdlib/map_demo.lk --output /tmp/lk-map-demo && /tmp/lk-map-demo` → **输出 `map_demo: all assertions passed`** ✅
- `target/debug/lk compile exe examples/general/sort_search.lk --output /tmp/lk-sort-search && /tmp/lk-sort-search` → **输出 `sort_search: all assertions passed`** ✅
- `cargo test -p lkrt` → **0 passed, 0 failed** ✅
- `examples/` sweep → **TOTAL=47 OK=40 VM_FAIL=7 COMPILE_FAIL=0 NATIVE_FAIL=0 DIFF=0**；剩余 VM_FAIL 发生在 VM/typechecker/import resolver 层，不是 native lowering 失败；当前额外 VM_FAIL 来自新增 `examples/stdlib/comprehensive.lk` 的 `Compiler32 undefined callable __lk_call_method`。
- LLVM native stdlib 方向已切到 typed `lkrt` runtime + 纯 LK stdlib source，禁止继续把完整 stdlib 方法语义散落手写到 LLVM 后端 ✅
- 最小 native executable 可链接 `lkrt`，`nm` 可见 `lkrt::link_anchor/version`，`strings` 未发现 `execute_module`、`Module32Artifact`、`Instr32 VM`、`VmContext`、`lk_core::vm`、`compile_program32`、`rt::init_runtime`、`VM runtime` 等 shell/runtime 痕迹 ✅
- 无旧 Op runtime、BC32、packed executor、quickening、旧 fused opcode 残留 ✅

## 未完成风险

### LLVM 后端（第 12 项）

当前 `examples/` 覆盖不是完成证明，且最近完整 sweep 暴露了外围与 LLVM 混合问题。LLVM stdlib 收口方向已经调整为 typed `lkrt` + 纯 LK stdlib source，不能继续在 LLVM 后端手写完整 stdlib 方法体：

1. **spec/语言 shape 覆盖审计不足** —— 需要对照 parser/type checker/compiler 支持的语法和 runtime value shape，列出 LLVM 必须 native-lower 的完整矩阵。
2. **unsupported reason 收口不足** —— examples 和 LLVM 单测之外仍可能存在会落到泛化 `unsupported Instr32/native value shape` 的组合；需要逐类压成 true native lowering 或明确 reason。
3. **stdlib native 架构迁移** —— `lkrt` link boundary 已作为第一步，后续要把纯 stdlib 方法迁到 LK source/monomorphization，host primitives 只走 registry。
4. **examples 剩余失败需分类修复** —— `import*.lk` 从 repo root 运行仍是 resolver/cwd 问题；`json_process.lk`、`string_methods.lk`、`named_args.lk` 当前 VM/typechecker/example 自身失败；`error_handling.lk` 已恢复 native executable 与 VM 一致输出。
5. **callable/container/object 显示语义仍需系统化** —— 后续若要求 callable/object 等非 scalar return 也 native-lower，需要实现与 VM display 一致的真实 native 表示。
6. **文件大小边缘** —— `facts.rs`、`blocks.rs`、`straightline_value.rs` 仍接近 1500 行，后续改动必须继续拆分。
