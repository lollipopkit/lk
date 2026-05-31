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
- `cargo test -p lk-core llvm --features llvm` → **144 passed, 0 failed** ✅
- `internal.lk` LLVM 编译为可执行文件，运行结果正确（输出 `12\n11`） ✅
- `fib.lk` 和 `general/fib.lk` LLVM 编译通过 ✅
- `examples/syntax/match.lk` LLVM 编译为可执行文件，native 输出与 VM 一致 ✅
- `examples/syntax/operators.lk` LLVM 编译为可执行文件，native 输出与 VM 一致 ✅
- 无旧 Op runtime、BC32、packed executor、quickening、旧 fused opcode 残留 ✅

## 未完成风险

### LLVM 后端（第 12 项）

当前已验证可 `lk compile exe` 并与 VM 输出一致的 examples：

- `examples/fib.lk`
- `examples/general/fib.lk`
- `examples/general/recursive.lk`
- `examples/general/test_list_sum.lk`
- `examples/general/test_min_recursive.lk`
- `examples/general/test_pure_assert.lk`
- `examples/lk-example-workspace/crates/greetings/src/mod.lk`
- `examples/lk-example-workspace/crates/mathlib/src/mod.lk`
- `examples/stdlib/os_demo.lk`
- `examples/syntax/internal.lk`
- `examples/syntax/match.lk`
- `examples/syntax/named_args.lk`
- `examples/syntax/named_params.lk`
- `examples/syntax/null_coalescing.lk`
- `examples/syntax/numeric_auto_promotion.lk`
- `examples/syntax/operators.lk`
- `examples/syntax/template_strings.lk`

主要失败模式：
1. **运行时全局依赖**（concurrency_demo/config_parser/word_count/ranges/unsupported）—— 需要的全局函数没有 `native_static_global` 映射，被正确拒绝。
2. **动态容器方法调用**（higher_order/sort_search/control_flow/for_loop_patterns/pattern_matching）—— `Contains` 静态 control-flow 形态已覆盖；剩余高频缺口主要是 ToIter/SliceFrom/NewRange/MapRest。
3. **println 动态 Call / subfunction text**（closure/struct_trait/trait_impl）—— `named_args.lk`、`named_params.lk`、`null_coalescing.lk`、`template_strings.lk`、`recursive.lk` 已修复；其他样例仍需分别补 facts 或 subfunction opcode。
4. **scalar facts 分类失败** — `numeric_auto_promotion.lk` 已修复；剩余示例仍包括 higher_order/list_ops/closure/error_handling/struct_trait/trait_impl 等。
5. **direct call / method lowering 缺口** — 剩余样例仍需要更完整的 dynamic method call、multi-arg println 或 subfunction text path 覆盖。

### 已修复的 LLVM 后端问题

1. Not 指令在 scalar_facts 中接受 I64/F64/StrPtr（之前只接受 Bool/Nil）
2. native_builtin_return_kind 新增 Panic 支持
3. GetIndex 支持未知动态容器回退
4. Call 不要求所有 Builtin 参数是静态值
5. CoreCallMethod 默认返回 I64
6. 自递归 callee 自动检测 + I64 回退 hints
7. subfunction.rs 新增参数类型候选、StrPtr 参数、ToString/ConcatString text-part 保留
8. scalar_blocks.rs 在发射 @lk_fn_X 调用前预检查 subfunction 编译可行性
9. native main nil return 静默，避免 examples 末尾多输出 `nil`
10. 清理 scalar_facts 未加开关 debug stderr 输出
11. 修复 `match.lk` native 输出：assert direct call 不再回退，比较 lowering 优先使用当前 block 的本地类型，避免 stale facts 把整数/字符串寄存器误判
12. 修复静态 Test 的 untaken-path 标记：遇到多 predecessor merge 起点不再把后续可达路径跳过
13. control-flow `GetIndex` 可在 boundary 后回溯只读 heap const int list，静态 list loop 的 LLVM 回归恢复
14. control-flow template string equality 支持动态 `Text` 与静态字符串比较，覆盖 nullish/template interpolation assert
15. block lowering 对静态 I64 二元运算保留折叠值，支持多参数 `println("{} + {} = {}", ...)`
16. `container.rs` 拆出 `container/index.rs`，保持单文件低于 1500 行
17. 静态 direct-call 折叠可在同一 basic block 内恢复 heap-const list 参数，并支持静态 `list.skip(1)`；`examples/general/test_list_sum.lk` 已通过 native/VM diff
18. 自递归 hint 探测支持按参数组合尝试 I64/F64/Bool/list-like profile；`examples/general/recursive.lk` 中 `contains(List<Int>, Int) -> Bool`、`length(List<Int>) -> Int` 已通过 native/VM diff
19. Float opcode lowering 支持 I64/F64 混合 operand，通过 `sitofp` 生成 double 运算；`examples/syntax/numeric_auto_promotion.lk` 已通过 native/VM diff
20. CFG merge block 统一视为静态事实边界，避免 `match.lk` 中分支字符串事实污染合流后的比较；`examples/syntax/match.lk` 已恢复 native/VM diff
21. control-flow `Contains` 支持静态 string/list membership，包括 heap-const int list 的 `DynamicIntList` 回查；`examples/syntax/operators.lk` 已通过 native/VM diff

### scalar_facts.rs 文件大小

当前 LLVM 相关主要文件均低于 1500 行：`scalar_facts.rs` 1411 行，`scalar_blocks.rs` 1498 行，`scalar_block_helpers.rs` 1456 行，新增 `scalar_contains.rs` 54 行。`core/src/vm/exec32/container.rs` 已拆到 1372 行，新增 `container/index.rs` 169 行。

## 下一步建议

1. 继续扩展 subfunction/block 编译器处理剩余 `println`/text direct-call 动态路径。
2. 完善 closure/higher_order/list_ops/error_handling 等剩余 scalar facts 分类失败。
3. 推进动态 list/string method lowering，优先 `ToIter`、`SliceFrom`、`NewRange` 这类 examples 高频形状。
4. 对每个 unsupported shape 选择：真正 native lower，或给出明确 unsupported reason。
5. 达到足够覆盖率后做 LLVM 编译验证和 benchmark。
