# Handoff — LK LLVM examples native-bin progress

## Objective

`examples/` 下所有 `.lk` 文件最终都要能通过 `lk compile exe` 生成 native binary，并且 binary 输出与 VM 执行输出一致。输出路径必须是纯 LLVM IR -> clang -> bin；不能回退到内置 VM/runtime launcher。

## 本轮完成

- 修正 native main 返回语义：非 nil 返回才打印，nil return 静默，和 CLI VM 的 `first_return_is_nil()` 行为一致。
- 扩展 standalone subfunction lowering：
  - 支持 StrPtr 参数候选推断，避免 named arguments 的字符串参数被当作 i64。
  - `LoadString` 保留静态字符串符号。
  - `ToString` / `ConcatString` 复用 `NativeTextPart`，不再把模板字符串粗暴折叠成 RHS。
  - control-flow merge 处清理静态寄存器事实，避免默认分支字符串污染运行时参数路径。
- 更新测试断言：captured closure 被常量折叠为 bool 时仍验证无 runtime shell；nil return 不再要求打印 `nil`。
- 清理 scalar facts 的未加开关 debug stderr 输出。
- 修复 `examples/syntax/match.lk` native 输出不一致：
  - `assert(cond)` 识别为 direct native assert path，失败时跳到统一 `lk_assert_fail`，不嵌 VM/runtime。
  - block compare lowering 优先使用当前 block 本地类型/heap string 类型，避免 stale scalar facts 把 merge 后寄存器误判。
- 修复静态 Test 的 untaken-path skip：如果 untaken 起点是多 predecessor merge，不再把后续可达路径跳过。
- control-flow `GetIndex` 可在 boundary 后回溯只读 heap const int list，恢复 static-list i64 loop lowering。
- 修复 `examples/syntax/null_coalescing.lk` native lowering：
  - `Text` 与静态字符串比较走真实 LLVM equality，不跨控制流边界恢复错误静态字符串值。
  - heap const list/map/cell/string 通过受限静态容器回溯支持 `?? []` 后的 `.len()`。
- 修复 `examples/syntax/template_strings.lk` native lowering：
  - block lowering 保留静态 I64 二元运算结果，覆盖多参数格式化 `println("{} + {} = {}", ...)`。
  - 动态/静态 template interpolation assert 输出与 VM 一致。
- 修复 `examples/general/test_list_sum.lk` native lowering：
  - 静态 direct-call 折叠只在需要恢复同 basic block 内 heap-const list 参数时触发，避免吞掉普通 direct-call inline 覆盖。
  - `__lk_call_method` 静态求值支持 list `skip(n)`，用于有界递归 list sum。
- 修复 `examples/general/recursive.lk` native lowering：
  - 自递归 hint 探测按参数组合尝试 I64/F64/Bool/list-like profile。
  - `contains(List<Int>, Int) -> Bool` 不再被最终 I64 hint 覆盖，`length(List<Int>) -> Int` 保持正常递归分类。
- 修复 `examples/syntax/numeric_auto_promotion.lk` native lowering：
  - Float opcode facts 接受 I64/F64 混合 operand。
  - LLVM emit 对 I64 operand 生成 `sitofp` 后执行 double 运算。
- 修复 `examples/stdlib/os_demo.lk` native lowering：
  - 增加 `os.hostname/arch/os/dir_current/dir_temp/dir_list` native static builtin 映射。
  - 支持 `os.env.get(name)` 单参数形式并保持 VM/native 输出一致。
- 修复 `examples/syntax/operators.lk` native lowering：
  - control-flow `Contains` 支持静态 string/list membership。
  - 对 block lowering 中的 heap-const int list `DynamicIntList` 回查原始常量，覆盖非 int needle 返回 false。
- 修复 `examples/syntax/match.lk` 回归：
  - CFG 多 predecessor merge block 统一作为静态事实边界。
  - 比较类型选择在本地回溯证明为 `StrPtr` 时避免 stale Bool facts 覆盖，恢复 string equality lowering。
- 拆出 `core/src/vm/exec32/container/index.rs`，把 `container.rs` 从 1500 行以上压回限制内。
- 更新 `docs/llvm/backend.md`：nil return 在 native CLI 输出中静默。

## 当前验证

- `cargo test -p lk-core llvm --features llvm` -> 144 passed, 0 failed。
- `cargo build -p lk-cli --features llvm` -> pass。
- examples 批量探测中已确认 17 个唯一文件输出一致：
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

## 剩余主要失败类别

- runtime globals：`json/yaml/toml/math/map/time/datetime/io/iter/stream/tcp/chan` 等仍被正确拒绝为 unsupported native globals。
- container/string control flow：静态 `Contains` 已覆盖；`ToIter`、`SliceFrom`、`NewRange`、`MapRest` 等在若干 examples 中仍缺 native lowering。
- scalar facts 分类失败：`higher_order`、`list_ops`、`closure`、`error_handling`、`struct_trait`、`trait_impl` 等。
- direct call/method lowering：剩余样例仍需继续扩大 dynamic method、subfunction text path 和 multi-arg `println` 覆盖。
- object/pattern opcodes：`NewObject`、`IsList` 相关 examples 仍未 native-lowerable。

## 下一个建议入口

1. 处理 `scalar block facts could not classify` 中更小样例，如 `closure.lk`、`higher_order.lk` 或 `list_ops.lk`。
2. 继续扩大 list/string method native lowering，优先 examples 中高频的 `ToIter`、`SliceFrom`、`NewRange`。
3. 最后集中推进 runtime globals 和 container iterator lowering，这部分会触及更大 API 面。
