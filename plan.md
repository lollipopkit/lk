# LK 通用 VM 性能优化计划

要优化通用性能,而不是特定任务;hot path放到之后做,现在需要架构层面上的改进;
直到lk的通用性能接近lua(1.1x);

## 当前判断

LK 和 Lua 的差距不是单个 workload 或某个 opcode 的问题，而是通用 VM 的固定成本
偏高。之前已经做过若干 packed hot-slot、tiny call、map/list/string 局部优化，
这些证明了方向有效，但继续堆特定形态会让系统越来越难维护。

当前阶段要暂停新增 benchmark 专用优化，先做架构层改造：

- 让 compiler、VM、AOT 共用同一套性能事实。
- 让 typed lowering 来自稳定 facts，而不是局部临时集合或后端猜测。
- 让 call frame 有统一、可缓存、低分配的调用协议。
- 让 `Val` 和 register 写入有统一 ownership/liveness 策略。
- 让后续 hot path 只是消费这些基础能力，而不是继续手写特例。

`bench/README.md` 里的 Latest Quick Comparison 是当前性能表。每轮代码实现后都要
更新该表，并在本文件记录本轮架构收益和验证结果。

## 优化总原则

1. 架构优先于特例：当前不新增只服务单个 workload 的 `TinyCallPlan`、packed
   fusion 或 benchmark 名称驱动优化。
2. facts 优先于猜测：任何 typed op、dead temp、call fast path 都必须能追溯到
   可验证的分析事实。
3. 通用成本优先：先降 dispatch、clone、register write、call frame、runtime
   fallback 等每条业务语句都会付出的成本。
4. VM/AOT 共享：同一个性能事实应同时驱动 bytecode/BC32 和 LLVM lowering。
5. 可观测优先：每轮都记录 counters；wall time 有噪声时，以 opcode steps、
   typed branches、clone、register writes、fallback counters 判断方向。

## 阶段 1：`PerformanceFacts` 成为唯一性能事实源

目标：把已经存在的 `PerformanceFacts` 从“多个执行路径可选消费的附加信息”提升为
compiler、VM、packed/BC32 和 AOT 的唯一性能事实来源。后续 typed lowering、copy
policy、dispatch plan、call frame plan 都必须能追溯到 `FunctionAnalysis.perf`，而不
能在局部路径重新推断一套不可验证状态。

当前代码基础：

- `FunctionAnalysis` 已携带 SSA、escape summary、region plan 和 `PerformanceFacts`。
- `PerformanceFacts` 已有 value/register/container/key/copy/move/control-flow 字段：
  `PerfValueFact`、`PerfRegisterFact`、`PerfKeyFact`、`PerfRegisterCopyFact`、
  `PerfLocalCopyFact`、`PerfContainerMoveFact`、`PerfControlFlowFacts`。
- `compiler/ssa/pipeline.rs` 已能从 SSA 和 escape analysis 生成基础 value facts。
- `compiler/builder.rs` 已在 finalize 阶段同步 register facts，并补 liveness/copy facts。
- LKB 已有 `FunctionAnalysis` 和 `PerformanceFacts` 的 encode/decode 路径。

技术方案：

- SSA 层只负责源级事实：value kind、escape class、return region、简单 phi join。
- bytecode builder 层只负责寄存器事实：register kind、local slot、container adoptable、
  const/interned key、string-int key、copy/move/dead-write/control-flow。
- runtime 和 lowering 层只能查询 facts，不允许把“当前看到的 opcode 序列”当作跨 block
  类型证明。
- 为 facts 增加稳定 query API，而不是让调用方直接拼字段逻辑：
  `value_kind(reg)`、`container_value_kind(reg)`、`known_key(pc)`、
  `copy_policy(pc)`、`container_move_policy(pc)`、`same_block(a, b)`、
  `dead_write(pc)`。
- 所有 invalidation 都在 facts 生成层完成：branch merge、loop back edge、map/list
  mutation、global write、unknown call side effect 必须清掉对应 register/container/key
  facts。
- LKB round-trip 是同等入口：从缓存加载的函数必须和源码编译函数得到同样的 facts-driven
  lowering 条件。

实施顺序：

1. 给 `PerformanceFacts` 补 query API 和单元测试，先覆盖已有字段，不新增激进推断。
2. 把 compiler 中仍直接读取 `int_regs`、`float_regs`、`map_value_types`、
   `list_value_types`、`string_int_keys` 的 typed 决策改成调用 facts query。
3. 给 branch merge、loop mutation、unknown call side effect 增加 facts regression tests。
4. 给 LKB 增加 round-trip 测试：保存再加载后，typed lowering 所需 facts 不丢失。
5. profile 输出保留 facts 命中/失效 counters，后续每轮能解释 typed op 增减。

验收标准：

- 同一源码在 fresh compile 和 LKB reload 后，关键 `PerformanceFacts` 一致。
- typed lowering 不再依赖无法从 `FunctionAnalysis.perf` 解释的临时集合。
- if/else merge、loop mutation、call side effect 不产生 stale typed op。
- `cargo test -p lk-core analysis compiler lkb -- --nocapture` 覆盖 facts 级回归。

禁止事项：

- 不为了某个 workload 在 builder、peephole、packed decoder 或 AOT 中手写旁路事实。
- 不在 runtime 根据历史值反推静态事实；runtime 可以 quicken，但不能替代 facts。
- 不新增无法被 LKB 保存的优化依据。

## 阶段 2：typed lowering 全部由 facts 决策

目标：让 typed bytecode/BC32/AOT lowering 全部由 `PerformanceFacts` 的稳定 query 决策。
packed hot slot 和 quickening 可以消费 typed op，但不能继续扩张为主要推断系统。当前
性能差距里的 opcode/branch 数量很高，阶段 2 要先减少 generic dispatch 和动态 tag
检查，而不是继续堆 workload fusion。

当前代码基础：

- 已有 numeric typed op：`AddInt`、`SubInt`、`MulInt`、`ModInt`、`AddIntImm`。
- 已有 typed branch：`CmpIntJmp`、`Cmp*ImmJmp`、`AddIntImmJmp`。
- 已有 container/key typed op：`ListIndexI`、`MapGetInterned`、`MapGetDynamic`、
  `MapHasK`、`MapSetInterned` 等。
- packed/BC32 decoder 已能识别 typed op，并有 hot slot/fusion 执行层。
- AOT 已有部分 map/list/string typed lowering，但仍存在 VM/AOT 规则分叉风险。

技术方案：

- compiler lowering 只通过 facts query 选择 typed op：
  - numeric：两个 operand 都是 `Int` 才生成 int op；`Float` join 后生成 float path；
    `Unknown` 必须保留 generic op。
  - branch：比较输入和 immediate 来源由 facts 证明后生成 typed branch；merge 后降级。
  - list：list value kind 和 index kind 同时可证时生成 typed list op。
  - map/key：const/interned/string-int key fact 可证时生成 typed map op。
  - string：string predicate、length、string-int key 只从 key/string facts 生成。
- peephole 只做同一 basic block 内的规范化，例如 immediate folding、相邻 branch 形态整理；
  不承载跨 block 类型推断。
- packed/BC32 只做编码和局部组合执行：它可以把 typed op 组合成更少 dispatch，但组合条件
  必须来自已经生成的 typed bytecode 和 `same_block` facts。
- quickening 只作为 runtime guard cache：用于反复出现的 dynamic site，不作为 compiler
  facts 的替代品。
- AOT lowering 读取同一 facts query，无法 native lowering 时显式进入 helper fallback，并
  记录 fallback counter。

实施顺序：

1. 建立 `TypedLoweringFacts` 或等价 helper，集中暴露 numeric/branch/container/key/string
   query。
2. 迁移 numeric 和 branch lowering，先覆盖 opcode 数量最高的 loop/branch 形态。
3. 迁移 map/list/string lowering，先覆盖 const key、interned key、string-int key、
   list int index。
4. 让 packed decoder 删除或隔离独立类型判断，只保留 typed opcode 组合和 `same_block`
   安全检查。
5. 让 AOT lowering 调用同一 query helper，并在 tests 中比较 VM bytecode 与 AOT lowering
   使用的 facts。

验收标准：

- typed op 生成条件集中在 facts query/helper 中，可单测、可解释。
- block merge 后不会生成 stale `AddInt`、`CmpIntJmp`、`MapGet*`、`ListIndexI`。
- profile 中 `Typed`、`Branches`、`Containers` 的变化能对应到具体 facts 命中。
- VM 和 AOT 对同一源码的核心 type/key/container facts 一致。

禁止事项：

- 不新增 benchmark 名称驱动 opcode。
- 不把 packed fusion 当成阶段 2 的主要收益来源；fusion 只能消费通用 typed op。
- 不允许 AOT 为弥补 VM 缺口复制另一套类型推断。

## 阶段 3：Call Frame 与 `CallSitePlan` ABI 稳定化

目标：把 call path 从“共享 helper 已经很多”推进到稳定 ABI：call site miss 构建 plan，
call site hit 只消费 plan；opcode、packed、普通 `Val::call` fallback 都尽量共享同一 frame
activation、argument seed、return layout 和 diagnostics metadata 生命周期。

当前代码基础：

- `CallSitePlan` 已包含 closure pointer、function pointer、arity、return layout、
  tiny plan、captures、capture specs、frame info。
- `NamedCallSitePlan` 已缓存 named layout 入口。
- `FunctionRuntimePlan`、`FrameActivation`、`StackWindow`、`FrameStateSetup`、
  `FrameExecutionParts` 已形成 frame setup 链路。
- opcode 与 packed call 已大量收敛到 `call_common`。
- `TinyCallPlan` 已存在，但包含若干 workload-like leaf 形态，后续不继续扩张。

技术方案：

- positional call：
  - miss 时解析 closure metadata、captures/capture specs、frame info、param layout、
    return layout，构建 `CallSitePlan`。
  - hit 时只校验 closure identity/arity/return layout，不再重复 clone frame metadata 或重建
    capture 信息。
  - frame setup 统一从 `FunctionRuntimePlan -> StackWindow -> FrameActivationParts` 进入。
- named call：
  - plan 中保存 named 参数到 callee register 的映射、default thunk 需求、optional nil 位置。
  - hit 时不再每次重建 provided/default 映射；只做输入 named len/layout guard 和 seed。
  - default thunk 执行也借用 plan 中的 frame info/captures，不重新构造元数据。
- native call：
  - fast native、named native、generic native 共享 return layout 和 error path。
  - native miss/fallback 必须经过 `call_common`，避免 opcode/packed 两套返回协议。
- inline/tiny call：
  - `TinyCallPlan` 冻结为现有 leaf optimization，不新增 GCD、prime、binary-search 等任务形态。
  - 后续小函数 inline 只能由 SSA/facts 决策，结果进入通用 call plan 或 typed bytecode。

实施顺序：

1. 给 `CallSitePlan`/`NamedCallSitePlan` 增加命中路径 tests：确认 hit 不重新读取/clone
   frame info、captures 和 named layout。
2. 把 remaining opcode/packed call wrapper 收窄为参数适配层，return/pending-pc/error 都进入
   `call_common`。
3. 让普通 `Val::call`/`call_named` fallback 尽量复用同一 argument seed 和 frame info 借用策略。
4. 给 profile 增加或整理 call counters：miss、closure IC hit、named IC hit、native fast hit、
   generic fallback、tiny/inline hit。
5. 对 call-heavy workload 用 profile 验证 call counters 下降，再看 wall time。

验收标准：

- exact closure IC hit 后不重复读取 closure metadata、frame captures、frame info。
- opcode path 和 packed path 的 positional/named/native call 由同一组行为测试覆盖。
- call profile 可以解释每个 workload 的 call miss/hit/fallback 分布。
- `TinyCallPlan` 没有新增 benchmark 专用形态。

禁止事项：

- 不在 opcode 和 packed call 中复制 return/pending-pc/error 协议。
- 不新增按函数名、workload 名或固定业务算法识别的 call plan。
- 不为了减少 clone 破坏 call stack diagnostics 或 default thunk 语义。

## 阶段 4：`Val` 与 register movement 由 liveness/ownership 驱动

目标：减少通用动态值路径里的 clone/refcount、动态 tag check 和无意义 register write。
阶段 4 的核心不是继续给每个 helper 加 metrics gate，而是让 register/local/container 写入
统一消费 `PerformanceFacts` 里的 liveness、copy、move、dead-write 事实。

当前代码基础：

- register 写入已通过 `assign_reg_*_with_metrics()`、`write_register_*_with_metrics()`、
  `copy_*_for_register_with_metrics()` 收敛。
- `FrameStateSetup` 和 `FrameExecutionParts` 已携带 `collect_metrics`，frame runtime 不再在
  内层重复读取全局 metrics gate。
- `PerformanceFacts` 已有 `dead_writes`、`register_copies`、`local_copies`、
  `container_moves`、`local_slots`。
- `Val::access_with_metrics()`、`BinOp::eval_vals_with_metrics()`、container arithmetic helper
  已有显式 metrics gate，metrics 关闭时保留快路径。

技术方案：

- register write：
  - 所有写入 helper 接收 pc 或预解析 copy policy，能用 `dead_write(pc)` 证明无后续读取时跳过。
  - pure temp 写入优先消除；必须保留 observable value 时才写 register。
  - immediate/cheap-copy 值不进入 heap copy 计数和 Arc refcount 路径。
- register/local move：
  - `register_copies[pc].move_source` 为真时使用 take/move；否则 copy。
  - `local_copies[pc].move_source` 为真时 local load/store 使用 move-aware helper。
  - runtime scan 只能作为 debug/test fallback，生产路径优先 facts。
- container move：
  - `container_moves[pc]` 决定 map/list set/push 是否 move key/value。
  - map/list/string helper 保留语义边界：失败路径必须 restore 被 take 的 key/value。
  - heap-backed value 只有在必须共享、escaping 或 failure recovery 需要时 clone。
- dynamic `Val` path：
  - `Int`、`Float`、`Bool`、`Nil`、short string 维持 cheap-copy。
  - `Arc` 容器、heap string、closure、object 不进入短生命周期临时 clone，优先借用或 move。
  - generic fallback 继续存在，但必须显式消费 `collect_metrics` 和 copy policy。

实施顺序：

1. 给 write/copy/move helper 增加 facts-driven 入口；旧无 facts wrapper 只保留测试兼容或
   非 frame runtime 调用。
2. 把 opcode 与 packed 的 move/local/container 写入改为使用 pc-indexed facts，不再运行时扫描。
3. 给 list/map/string mutation 增加失败恢复测试，证明 move key/value 不改变错误语义。
4. 用 copy profile 验证 `Clones`、`HeapClone`、`CopyHeap`、`LocalHeap`、`ContHeap` 的变化。
5. 只有当 counters 证明主要 clone/write 成本下降后，再看 quick bench wall time。

验收标准：

- copy profile counters 稳定且能解释本轮变化。
- dead temp 后续不读取时，opcode 与 packed path 一致跳过写入。
- list/map/string mutation 的成功和失败路径语义不变。
- 生产 frame runtime 中不新增全局 metrics gate 读取或 runtime liveness scan。

禁止事项：

- 不为了局部 wall time 正向而绕过统一 copy policy。
- 不在 packed fusion 中单独实现 dead temp 消除；必须由 facts 驱动。
- 不使用 `unsafe` 改写 `Val` 或 register movement；LLVM 以外仍保持 safe Rust。

## 阶段 5：容器与字符串基础布局

目标：把 map/list/string 从泛型 runtime helper 逐步降成可由 facts 驱动的通用布局操作。

如何优化：

- const/interned string key 进入统一 key fact，避免重复构造 key。
- string-int template key 记录 prefix、int source 和容量估计，后续由 VM/AOT 共用。
- map/list value type 由 mutation invalidation 管理，避免错误保留旧类型。
- map counter update、list int access、string predicate 只能作为 facts lowering 的结果，
  不能写成 workload 名称特例。
- 参考 Lua table/string 的紧凑热路径，但保留 LK 语义边界。

验收：

- map/list/string lowering 不依赖 workload 名称。
- AOT 和 VM 看到相同 key/container facts。
- counters 能区分 key allocation、map lookup、list clone、string allocation。

## 阶段 6：VM/AOT 共用中间层

目标结构：

```text
AST
  -> SSA
  -> PerformanceFacts
  -> typed bytecode / BC32
  -> VM

AST
  -> SSA
  -> PerformanceFacts
  -> LLVM lowering
  -> AOT
```

如何优化：

- 所有新 typed lowering 先接入 `PerformanceFacts`，再分别接 VM/AOT 消费端。
- LLVM helper 只作为无法 native lowering 的 fallback，不作为默认容器/字符串路径。
- LKB 需要保存足够的 analysis/facts，使加载缓存后仍能走相同优化路径。

验收：

- 同一源码编译到 VM 和 AOT 时，核心 type/key/call facts 一致。
- AOT 不再为了弥补 VM-only 优化重复实现另一套分析。
- fallback 次数可统计，并能在 benchmark/profile 中看到。

## 每轮执行规则

常规验证命令：

```bash
cargo test -p lk-core
cargo fmt --all -- --check
cargo build --release -p lk-cli
```

行数检查：

```bash
rg --files -0 -g '!target/**' -g '!references/**' -g '!website/node_modules/**' \
  -g '!vsc-ext/lsp/node_modules/**' \
  | xargs -0 wc -l \
  | awk '$1 > 1500 { print }'
```

benchmark：

```bash
bench/run_workload_bench.sh
```

记录指标：

- wall time：VM/Lua、AOT/Lua、AOT/VM。
- dispatch：opcode steps、branches、typed branches。
- value movement：`Val` clones、heap clones、immediate clones、register writes。
- runtime fallback：generic op fallback、BC32 fallback、AOT helper fallback、call fallback。
- correctness：checksum、unit tests、doc build if touched。

## 本轮记录：阶段 1 facts query 收口

范围：阶段 1，继续把 `PerformanceFacts` 从可选附加信息收口为 typed/container/key
lowering 的稳定查询入口，避免 compiler/AOT 直接拼底层字段。

本轮完成：

- 新增 `core/src/vm/analysis_queries.rs`，集中提供 `value_kind`、`value_type`、
  `list_value_kind`、`list_value_type`、`list_known_len`、`map_value_type`、
  `known_key`、`dead_write` 等 query API。
- compiler container/type 决策改为通过 `PerformanceFacts` query 读取事实；AOT
  string-int key lowering 改为 `known_key(pc)`。
- default thunk 参数类型 seeding 改为复用 `apply_type_fact`，避免函数入口和默认参数入口
  分叉维护寄存器/container facts。
- `Expr::List` lowering 同步记录 list length fact，使 fresh compile 与 LKB reload 都能
  通过 query 得到相同 list facts。
- unknown call side effect 现在会保守清理 container facts，避免别名参数调用后继续使用旧的
  list/map value fact 生成 stale typed op。
- 增加 regression 覆盖：container facts query、unknown call side effect、branch mutation
  后 stale typed arithmetic、LKB round-trip 后 facts query 保持可用。

验证结果：

- `cargo test -p lk-core vm::compiler::tests::general_optimizations -- --nocapture`：49 passed。
- `cargo test -p lk-core vm::liveness::tests -- --nocapture`：15 passed。
- `cargo test -p lk-core vm::lkb::tests -- --nocapture`：5 passed。
- `cargo test -p lk-core`：877 passed, 3 ignored。
- `cargo fmt --all -- --check`：passed。
- `cargo build --release -p lk-cli`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.658x`，AOT/Lua `1.947x`，AOT/VM `1.174x`。最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 2 typed lowering helper 收口

范围：阶段 2，继续把 numeric/container typed lowering 的判断集中到可解释的 facts-driven
helper，减少调用点直接拼 `reg_known_*` 和局部类型集合的分支。

本轮完成：

- 新增 `core/src/vm/compiler/typed_lowering.rs`，集中承载 `select_arith_flavor`、
  `expr_value_fact`、`reg_value_fact`、`reg_known_*`、container value fact 查询和直接调用
  return fact 查询。
- `map_facts.rs` 回退为 container fact 生成/更新层，不再同时承载 typed lowering 查询逻辑。
- 新增 `emit_in_place_numeric_op`，把 compound assignment、self-assign inline 和 loop delta
  cache 中重复的 `reg_known_int` typed arithmetic 分支收口到同一入口。
- 保留当前 conservative fallback：无法由 facts 证明 `Int` 时继续生成 generic arithmetic，
  不新增 benchmark 名称驱动 opcode。

验证结果：

- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：154 passed。
- `cargo test -p lk-core`：877 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.670x`，AOT/Lua `1.984x`，AOT/VM `1.188x`。最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 2 container lowering helper 收口

范围：阶段 2，继续把 list/map/string access 和 length 的 opcode 选择收口到
`typed_lowering.rs`，减少表达式 lowering 调用点直接组合 `reg_known_list/map` 与局部 facts。

本轮完成：

- `typed_lowering.rs` 新增 `ContainerKind`、`reg_container_kind`、`list_known_len_for_reg`、
  `emit_len_for_value`、`emit_field_access_for_reg`、`emit_typed_list_access`、
  `emit_typed_map_access` 等 helper。
- `Expr::Access` 改为通过 `emit_field_access_for_reg` 统一选择 `MapGetInterned`、
  `MapGetDynamic`、`ListIndexI`、`AccessK`、`IndexK` 和 generic `Access`。
- `list.get`/`map.get` 的实际 opcode 选择改为委托 typed lowering helper；list length 读取同时
  支持 `PerformanceFacts::list_known_len` 与 builder 本地 length fact。
- `len` 方法调用改为通过 `emit_len_for_value` 统一选择 `ListLen`、`MapLen`、`StrLen` 或 generic
  `Len`。
- 保持现有语义和 conservative fallback，不新增 benchmark 名称驱动 opcode；尚未把所有方法调用
  pattern guard 收口，下一步可继续整理 `expr_call.rs` 和 range fold 入口。

验证结果：

- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：154 passed。
- `cargo test -p lk-core`：877 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.645x`，AOT/Lua `1.967x`，AOT/VM `1.196x`。最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 2 method guard facts 收口

范围：阶段 2，继续清理 compiler 中 container/method typed lowering 的直接事实读取，让方法调用
和 range fold 入口只表达语义形态，container 判定统一进入 `typed_lowering.rs`。

本轮完成：

- `typed_lowering.rs` 新增 `literal_method_name`、`unshadowed_module`、`known_list_expr`、
  `known_map_expr`、`expr_numeric_fact` 等 query helper。
- `expr_call.rs` 的 `list.get/set/push`、`map.get/has/set`、`len`、`math.floor(div)` 相关 guard
  改为通过 typed lowering helper 判断，不再直接读取 `reg_known_list/map` 或拼
  `expr_known_int/float`。
- `stmt/ranges.rs` 的 `ListFoldAdd`、`MapValuesFoldAdd` 入口改为通过
  `known_list_expr`/`known_map_expr` 判断容器事实。
- 删除未使用的 `reg_known_list`、`reg_known_map` 旧入口；当前 compiler 目录中
  `reg_known_list/map` 和 `expr_known_int/float` 只保留在 `typed_lowering.rs` 内部。
- 保持现有 conservative fallback 和现有 opcode 集，不新增 benchmark 名称驱动优化。

验证结果：

- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：154 passed。
- `cargo test -p lk-core`：877 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.682x`，AOT/Lua `2.000x`，AOT/VM `1.189x`。本轮为 typed lowering 架构收口，
  wall time 未改善，最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 4 local load move facts

范围：阶段 4，把 local load 也接入 `PerformanceFacts.local_copies` 的 pc-indexed move
policy，使 local load/store 都能由同一事实源决定是否 move，减少 heap-backed `Val` clone。

本轮完成：

- `annotate_local_copy_facts` 新增 `LoadLocal` 分析：当 local slot 在 load 后不可达、且不会在
  kill 前被 closure capture 或跨控制流边界读取时，记录 `PerfLocalCopyFact { move_source: true }`。
- runtime helper 新增 `assign_reg_from_local_load_or_take_with_metrics` 与
  `local_load_may_take_source`，生产路径只读取 analysis fact；没有 analysis 时保守 copy。
- opcode `LoadLocal` 改为通过 `local_load_may_take_source` 决定 copy 或 take。
- packed hot/cold `LoadLocal` 通过 `packed_instr_pc` 映射回原始 instruction pc，再读取同一
  `local_copy` fact，避免 packed path 复制另一套 liveness 扫描。
- 增加 regression 覆盖：liveness 能标记 dead local load source，live local load source 保持 copy；
  普通 VM 和 packed VM 在 fact-approved `LoadLocal` 上都不产生 local heap clone。

验证结果：

- `cargo test -p lk-core vm::liveness::tests -- --nocapture`：17 passed。
- `cargo test -p lk-core vm::vm_test::bytecode -- --nocapture`：23 passed, 1 ignored。
- `cargo test -p lk-core`：881 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.692x`，AOT/Lua `2.010x`，AOT/VM `1.187x`。本轮 copy counters 有定向改善，
  但 wall time 未改善，最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 4 container move facts source-of-truth

范围：阶段 4，把 list/map mutation 的 move opcode 决策从 compiler 局部临时性判断收口到
`PerformanceFacts.container_moves`，保证最终 `ListPushMove`、`MapSetMove`、
`MapSetInternedMove` 都能由 facts 解释。

本轮完成：

- `expr_call.rs` 的 `list.push` 不再直接生成 `ListPushMove`，统一生成 `ListPush`，由
  liveness/container move facts rewrite。
- `expr_map.rs` 的 `emit_map_set` 不再用 `expr_result_is_temporary` 直接生成
  `MapSetMove`/`MapSetInternedMove`，统一生成普通 `MapSet`/`MapSetInterned`。
- 删除 compiler 层 `expr_result_is_temporary` move 判定入口；compiler 目录中不再直接
  `emit(Op::*Move)`。
- `annotate_container_move_facts` 对最终已 rewrite 的 `ListPushMove`、
  `MapSetInternedMove`、`MapSetMove` 回填 `PerfContainerMoveFact`，使 LKB/final analysis
  仍可解释 move opcode。
- 增加 regression：list/map fast path 测试不仅检查最终 move opcode，还检查对应 pc 的
  `analysis.perf.container_move(pc)`。
- 保持现有 opcode 集和 conservative fallback，不新增 benchmark 名称驱动优化。

验证结果：

- `cargo test -p lk-core vm::liveness::tests -- --nocapture`：17 passed。
- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：154 passed。
- `cargo test -p lk-core vm::lkb::tests -- --nocapture`：5 passed。
- `cargo test -p lk-core`：881 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.690x`，AOT/Lua `1.995x`，AOT/VM `1.181x`。本轮是 source-of-truth 收口，
  wall time 基本持平，最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 4 BC32 dead-write Nop elision

范围：阶段 4，把 dead-write facts 产生的 `Nop` 从“标准 bytecode 保持 pc 稳定”推进到
“BC32 packed path 不再为它付 dispatch 成本”。标准 `Function.code` 仍保留 `Nop`，
保持 LKB/debug/facts pc-indexed 语义；packed `code32` 生成时省略 `Nop`，并在 decoded
instruction 上保存原始 bytecode pc，保证 register/local/container copy policy 仍按原始 pc
读取 `PerformanceFacts`。

本轮完成：

- `Bc32Function::try_pack` 对 `Op::Nop` 记录 0 word，生成 packed words 时不再 emit
  Move-like Nop。
- `Bc32DecodedInstr` 新增 `source_pc`；`Bc32Decoded::from_words_with_source_pcs` 让 pack
  阶段把压缩后的 instruction 映射回原始 bytecode pc。
- packed runtime 的 `packed_instr_pc`、move-call argument seeding 改为通过 `source_pc`
  读取原始 pc，避免 `Nop` 省略后污染 `register_copies`、`local_copies` 等 pc-indexed facts。
- 更新 BC32 decode/packed hot slot regression：确认 `Nop` 被省略、decoded source pc 保留。
- 增加 VM regression：`Nop` 被 BC32 省略后，packed `Move` 仍使用原始 pc 的
  `PerfRegisterCopyFact`，不会退回 heap clone。
- 不改 opcode 语义，不压缩标准 bytecode，不新增 benchmark 专用 hot path。

验证结果：

- `cargo test -p lk-core vm::bc32::tests -- --nocapture`：34 passed。
- `cargo test -p lk-core vm::vm::runtime::frame::run::packed -- --nocapture`：26 passed。
- `cargo test -p lk-core vm::vm_test::bytecode -- --nocapture`：24 passed, 1 ignored。
- `cargo test -p lk-core`：883 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.668x`，AOT/Lua `1.962x`，AOT/VM `1.176x`。本轮减少 packed Nop dispatch，
  wall time 小幅正向，最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 5 constant len lowering

范围：阶段 5，把常量字符串/list/map 的 `.len()` 从 runtime `Len`/`StrLen`
收口到 typed lowering helper，在不执行 receiver 的前提下只折叠已经是 `Expr::Val`
的常量值，避免跳过有副作用的表达式。目标是降低通用字符串/容器长度查询的
dispatch 和 register write 成本，尤其是循环里的常量长度读取。

本轮完成：

- `typed_lowering.rs` 新增 `constant_len_expr` 和 `emit_const_int`，集中处理可证明的
  常量长度。
- `expr_call.rs` 的 zero-arg `.len()` 在 receiver 为常量 `Val::ShortStr`、
  `Val::Str`、`Val::List`、`Val::Map` 时直接生成整数常量。
- `split(...).join(...).len()` 的同分隔符 identity special case 在原 receiver 是常量时
  也直接生成整数常量。
- 保守避免折叠非 `Expr::Val` 的 list/map literal receiver，避免改变潜在副作用求值。
- 增加 string fast path regression，确认常量字符串 `.len()` 不再生成 runtime
  `StrLen`/`Len`，且不会加载 receiver 字符串。

验证结果：

- `cargo test -p lk-core vm::compiler::tests::string_fast_paths -- --nocapture`：3 passed。
- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：156 passed。
- `cargo test -p lk-core`：885 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.634x`，AOT/Lua `1.933x`，AOT/VM `1.184x`。本轮减少可证明常量 `.len()` 的
  runtime dispatch，wall time 小幅正向，最新表已同步到 `bench/README.md`。

## 本轮记录：阶段 3 call-site runtime plan ABI

范围：阶段 3，把 positional closure call hit 路径从“call IC 保存 closure/function
identity，再从函数对象准备 runtime metadata”推进到“`CallSitePlan` 直接携带
`FunctionRuntimePlan`”。目标是让 call-site hit 消费稳定 ABI 计划，减少 call path
里 runtime dispatch sites、region plan 等 metadata 的重复准备分叉。

本轮完成：

- `CallSitePlan` 新增 `runtime: FunctionRuntimePlan`，与 closure ptr、function ptr、
  return layout、captures、frame info 一起成为 positional call ABI 的一部分。
- `FunctionRuntimePlan::from_function` 从 test-only 提升为 VM 内可用构造入口，call-site
  plan miss 时一次性固化 function runtime layout。
- `ClosureFastCache` 新增 `prepare_cached_function_runtime`，call hit 可以直接使用 plan
  中的 runtime plan，只保留 function-site cache sizing 和 packed-hot key 准备。
- `invoke_vm_closure_fast_unchecked`、raw boundary、`Vm::exec_function_positional_fast_span_*`
  增加可选 runtime plan 参数；opcode/packed call 仍通过同一 `call_common` 路径。
- 增加 call-site regression，确认 plan 固化 runtime function key/reg count，IC clone 仍复用
  同一个 shared call-site plan。

验证结果：

- `cargo test -p lk-core vm::vm::caches -- --nocapture`：21 passed。
- `cargo test -p lk-core vm::vm_test::functions -- --nocapture`：13 passed, 1 ignored。
- `cargo test -p lk-core vm::vm::runtime::frame::run::packed -- --nocapture`：26 passed。
- `cargo test -p lk-core`：885 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；正式复跑两次分别为
  VM/Lua geometric mean `1.690x` 和 `1.691x`。本轮是 call ABI 架构收口，但 wall time
  相比上一轮 `1.634x` 未改善，最新表记录第二次复跑结果：AOT/Lua `1.986x`，
  AOT/VM `1.175x`。

## 本轮记录：阶段 5 facts-proven LICM for pure container/string queries

范围：阶段 5，把现有 loop-invariant expression cache 从算术/字面量扩展到 facts 可证明的
纯容器/字符串查询。目标是降低循环体内稳定 `map.get`、`map.has`、`list.get`、`.len()`、
`starts_with`、`contains` 的 dispatch/container/string 成本，同时不把可变 receiver 的查询
错误提升到循环外。

本轮完成：

- `loop_invariants.rs` 新增 `call_expr_safe_to_loop_hoist`，只允许 facts 能证明 receiver 类型且
  所有依赖名在循环内稳定的纯查询进入 LICM。
- 对 module 形式和 method 形式都生效：`map.get(map, key)`/`map.has`、
  `list.get(list, index)`、`obj.get/has`、`obj.len()`、`str.starts_with/contains("literal")`。
- 新增 `stmt_mutates_name`/`expr_mutates_name`，把 `map.set(x, ...)`、`x.set(...)`、
  `list.set(x, ...)`、`x.push(...)` 等 receiver mutation 视为阻止提升的写入。
- 稳定字符串 receiver 可通过 typed facts 或 `const_env` 中的不可变字符串证明。
- 增加 regression：稳定分支里的 `map.get` 提升到 loop guard 前；receiver 在循环里被
  `set` 时保持在循环体内；稳定字符串 `starts_with` 提升到循环外。

验证结果：

- `cargo test -p lk-core vm::compiler::tests::general_optimizations -- --nocapture`：52 passed。
- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：159 passed。
- `cargo test -p lk-core`：888 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.640x`，AOT/Lua `1.945x`，AOT/VM `1.186x`。相对上轮 `1.691x` 有正向改善，
  但仍未达到 `1.1x`。
- `PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：用于取证，不作为最新表。profile 中
  `route_permission_check` 的 container ops 从上一轮约 `90005` 降到 `10`，opcode 从约
  `1204142` 降到 `1114141`，解释了该 workload 从 `4.338x` 降到正式 run 的 `3.036x`。

## 本轮记录：阶段 5 typed register-index access 与 String facts

范围：阶段 5，把 facts 已证明的 register-index list/string access 从通用 `Access`
路径拆到专用 opcode。目标是让基础 typed lowering 不再只覆盖常量下标和 map dynamic key，
并确保新 opcode 进入 BC32/LKB/packed hot，而不是落在 packed cold path 造成循环回退。

本轮完成：

- 新增 `ListIndex(dst, base, index)` 和 `StrIndex(dst, base, index)` opcode，并补齐
  debug name、analysis category、liveness、peephole remap/dead-write、BC32、LKB 编解码。
- `FunctionBuilder` 新增 `string_regs`，让 `Type::String` 参数、字符串常量和 move/load/store
  传播进入同一套本地 facts；`emit_len_for_value` 和 field access 可消费 String facts。
- `emit_field_access_for_reg` / `emit_typed_list_access` 在 base 和 index facts 可证明时发出
  `ListIndex`/`StrIndex`；动态负数下标保持原 `Access` 语义，返回 `nil`。
- 主解释器和 packed cold path 支持新 opcode；packed hot decode/exec 支持 `ListIndex`、
  `StrIndex`，并让 `SubInt -> ListIndex -> SubInt` 继续复用既有
  `CmpIntSubAccessSub` fusion，避免 `sliding_window_sum` 掉回 cold path。
- 增加 regression：动态 list access 使用 `ListIndex`；String 参数动态 index 使用 `StrIndex`；
  LKB roundtrip 保留两个新 opcode；BC32 typed ops roundtrip 覆盖 `ListIndex`/`StrIndex`。

验证结果：

- `cargo test -p lk-core vm::compiler::tests -- --nocapture`：160 passed。
- `cargo test -p lk-core vm::vm::runtime::frame::run::packed::decode::decode_tests -- --nocapture`：
  26 passed。
- `cargo test -p lk-core`：890 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.654x`，AOT/Lua `1.674x`，AOT/VM `1.013x`。第一次实现只走 packed cold 时曾回退到
  `1.694x`，补齐 packed hot/fusion 后回到接近上一轮 `1.640x` 的区间，但本轮 wall time
  没有形成正式几何均值改善，不能作为接近 `1.1x` 的完成证据。

## 本轮记录：阶段 6 LLVM typed index lowering 对齐

范围：阶段 6，修复阶段 5 新增 typed register-index access 后 VM/AOT 分叉的问题。当前
bytecode 已经能由 facts 生成 `ListIndex`/`StrIndex`/`ListIndexI`/`StrIndexI`，但 LLVM
backend 没有对应 lowering，导致 `coverage bench/workloads_business_algorithms.lk` 报
`AOT entry: fallback (unsupported opcode in LLVM backend: ListIndex ...)`。本轮目标是让
AOT 消费同一套 typed opcode，而不是因为 VM-only 优化退回 fallback。

本轮完成：

- LLVM 主 opcode lowering 支持 `ListIndex`、`StrIndex`、`ListIndexI`、`StrIndexI`。
- typed index lowering 复用现有 `lk_rt_index` helper；该 helper 已对 list/string 负索引返回
  `nil`，与 VM typed index 语义一致。
- LLVM backend 的 destination/read/write 分析补齐 typed index opcode，覆盖 values、
  containers、string key、string length 等局部事实失效扫描。
- `lowers_index_and_len` regression 扩展为同时覆盖 generic `IndexK` 和四个 typed index
  opcode，确认它们都 lower 到共享 index helper。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk` 现在报告
  `AOT entry: native-lowerable`，不再因 `ListIndex` 退回 AOT fallback。

验证结果：

- `cargo test -p lk-core lowers_index_and_len -- --nocapture`：1 passed。
- `cargo test -p lk-core`：890 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`、
  `core/src/vm/compiler/peephole.rs`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.625x`，AOT/Lua `1.959x`，AOT/VM `1.206x`。本轮修复 AOT native coverage，VM 几何均值
  相对上一轮 `1.654x` 小幅正向，但 AOT 由于 typed index 走 helper 后在若干 workload 上
  明显慢于 VM，下一轮应继续把 facts-proven list/string typed index 从 helper fallback 推向
  更低成本的 LLVM native/container path。

## 本轮记录：阶段 6 typed index-len deferred materialization

范围：阶段 6，把阶段 5/6 新增的 typed register-index opcode 接入 LLVM backend 已有的
`index_result_feeds_only_len` 中间层。此前 generic `Index` 在 `x[i].len()` 形态下可以延迟
indexed value 物化，直接生成 `IndexLen` 或 ASCII char length 计算；`ListIndex`/`StrIndex`
则总是先调用 `lk_rt_index`，再对结果求 `Len`。这会让 facts-proven typed opcode 反而绕过
既有 AOT 中间事实。

本轮完成：

- `ListIndex`、`StrIndex`、`ListIndexI`、`StrIndexI` 的 LLVM lowering 改为调用统一的
  `emit_index_values_or_defer_len`。
- generic `Index` 和 typed index 共享同一套 deferred materialization 逻辑，避免 VM/AOT
  在 index-to-len 形态上继续分叉。
- 当 typed index 的结果只喂给 `Len` 时，AOT 不再先生成 `lk_rt_index` 调用，而是记录
  `KnownReg::IndexedValue` 或 `KnownReg::IndexedAsciiCharLength`，由后续 `emit_len` 生成
  `lk_rt_index_len` 或原生 ASCII 长度 select。
- `lowers_index_and_len` regression 扩展确认 typed index feeding len 会少一次
  `lk_rt_index` 物化，并生成 deferred `lk_rt_index_len`。
- 保持语义边界：负数下标仍通过 `lk_rt_index_len`/ASCII unsigned range check 得到长度 `0`，
  与 indexed value 为 `nil` 后 `.len()` 的结果一致。

验证结果：

- `cargo test -p lk-core lowers_index_and_len -- --nocapture`：1 passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：`AOT entry: native-lowerable`。
- `cargo test -p lk-core`：890 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo fmt --all -- --check`：passed。
- `git diff --check`：passed。
- 行数检查：本轮新增/修改的 Rust 文件均低于 1500 行；仓库仍有既有生成/锁文件超过限制：
  `Cargo.lock`、`vsc-ext/lsp/package-lock.json`、`tree-sitter-lk/src/parser.c`、
  `tree-sitter-lk/src/grammar.json`、`tree-sitter-lk/src/node-types.json`、
  `core/src/vm/compiler/peephole.rs`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.699x`，AOT/Lua `2.020x`，AOT/VM `1.188x`。本轮是 AOT 中间层收口；官方 quick run
  受低置信度 rows 和 VM 端噪声影响，wall time 没有形成正向证据。AOT/VM 相对上一轮
  `1.206x` 到 `1.188x` 略有改善，但绝对 AOT/Lua 仍偏慢，后续需要继续降低 helper-heavy
  container/string path 成本，而不能把当前结果视作接近 `1.1x`。

## 本轮记录：阶段 6 facts-proven integer conditional move

范围：阶段 6，补齐一个通用的控制流降本 primitive。此前 facts-proven `if a < b { x = y }`
会生成整数比较跳转、条件块 `Move`，顶层写入还会在分支内 `DefineGlobal`。这类形态在 VM 端
至少多一次 branch dispatch 和一次块跳转，在 AOT 端也无法表达为 SSA-friendly conditional
select。本轮新增 typed `CMoveInt` opcode，让“整数比较成立时移动整数寄存器”成为
bytecode/BC32/LKB/packed/AOT 都能识别的架构能力。

本轮完成：

- 新增 `Op::CMoveInt { dst, src, a, b, kind }`，compiler 对 facts-proven 单语句
  `if` 条件赋值进行 lowering；顶层 global write 通过 `CMoveInt` 后无条件 `DefineGlobal`
  当前寄存器值保持语义。
- VM opcode dispatcher、packed hot slot、packed cold fallback、BC32 encode/decode、LKB
  encode/decode、liveness、analysis opcode 分类全部接入 `CMoveInt`。
- LLVM backend 将 `CMoveInt` lowering 为 sentinel-guarded `select i1`，并补齐 destination
  invalidation 与 values/container/string facts 读写依赖扫描。
- 增加 regression 覆盖：compiler lowering、BC32 typed roundtrip、LKB roundtrip、packed
  hot-slot decode、LLVM `select` lowering。

验证结果：

- `cargo test -p lk-core known_int_single_assignment_if_lowers_to_cmove_int`：1 passed。
- `cargo test -p lk-core`：894 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.651x`，AOT/Lua `1.922x`，AOT/VM `1.165x`。本轮 quick run 相比上一轮官方记录有正向
  几何均值，但 coverage 中业务 workload 仍未出现 `CMoveInt` top opcode，说明这次收益不能
  归因于新 conditional move，必须按噪声/邻近改动谨慎看待。距离 `1.1x` 仍明显不足。

## 本轮记录：阶段 5 dynamic map key ownership policy

范围：阶段 5，拆分动态 map 写入 key ownership 与全局 string intern 策略。当前 workload
profile 显示 `two_sum_map`、`histogram_group_count`、`log_parse_filter`、`inventory_reorder`
等仍有大量 map/string/container 计数；检查代码后确认动态 `MapSet`/`MapSetMove` 对短字符串
key 仍走 `string_key_arcstr()`，会把 `"n${i}"`、`"b${bucket}"` 这类临时动态 key 写入全局
intern cache。常量 key 的 `MapSetInterned` 路径仍应保持 interned 行为，本轮只调整动态写入。

本轮完成：

- `Val` 新增 `dynamic_string_key_arcstr()`：`Str` 复用已有 `ArcStr`，`ShortStr` 直接构造
  owned `ArcStr`，不经过全局 intern cache。
- VM opcode `MapSet` / `MapSetMove` 改用动态 key ownership 策略；`MapSetInterned` /
  `MapSetInternedMove` 保持原有常量 key interned 策略。
- packed hot 的 `MapSet` / `MapSetMove` 同步改用动态 key ownership，避免 packed 路径和
  opcode 路径语义/成本分叉。
- 增加 regression，确认动态短字符串 key 插入后仍可通过普通 string lookup 命中 map。

验证结果：

- `cargo test -p lk-core dynamic_short_key_insert_matches_string_lookup`：1 passed。
- `cargo test -p lk-core`：895 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.649x`，AOT/Lua `1.920x`，AOT/VM `1.164x`。相对上一轮 `1.651x` 只有小幅/噪声级正向，
  但 `two_sum_map` 从 `1.358x` 到 `1.286x`、`inventory_reorder` 从 `2.329x` 到 `2.144x`
  符合动态 map key 成本下降方向。距离 `1.1x` 仍明显不足，下一轮应继续处理 list/map/string
  container hot path 的分配与 helper-heavy 成本。

## 本轮记录：阶段 5 facts-proven typed for-in lowering

范围：阶段 5，把非 range `for-in` 从统一 `ToIter + Len + Index` 推进到 facts-proven typed
iteration。此前即使 iterable 已被 compiler facts 证明为 `List` 或 `String`，for-in 仍会先进入
通用 iterable 规范化，再走 generic `Len`/`Index`。本轮目标是让基础容器/字符串循环消费
已有 facts，减少通用 dispatch，同时在 body 会修改 iterable 变量时保守回退，避免破坏现有
快照/别名语义。

本轮完成：

- 新增 facts-proven list/string for-in lowering：`List` 使用 `ListLen + ListIndex`，`String`
  使用 `StrLen + StrIndex`，不再经过 `ToIter`。
- 如果 iterable 是变量且 loop body 会 assign/mutate 该变量，则不启用 typed direct iteration；
  这保留源集合在循环中被修改时的保守语义。
- `ToStr`、`StrConcatKnownCap`、`StrConcatToStr` 的 builder facts 改为记录 `Type::String`，
  让模板字符串结果能被后续 typed iteration 消费。
- `StrIndex` 的 for-in item 在 compiler facts 中标记为 String，使 `ch.len()` 能继续 lower 到
  typed `StrLen`。
- LLVM `index_result_feeds_only_len` 扩展为同时识别 `Len`、`ListLen`、`MapLen`、`StrLen`，
  避免 `StrIndex -> StrLen` 在 AOT 中先 materialize indexed string 再求长度。
- 增加 regression：string for-in 使用 `StrLen`/`StrIndex` 且不生成 `ToIter`；list for-in
  使用 `ListLen`/`ListIndex` 并保留元素类型事实；mutating list for-in 不进入 typed direct
  iteration；LLVM typed/generic len feeder 都能触发 index-len deferred materialization。

验证结果：

- `cargo test -p lk-core vm::compiler::tests::string_fast_paths -- --nocapture`：4 passed。
- `cargo test -p lk-core vm::compiler::tests::list_fast_paths -- --nocapture`：13 passed。
- `cargo test -p lk-core lowers_index_and_len -- --nocapture`：1 passed。
- `cargo test -p lk-core`：898 passed, 3 ignored。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.637x`，AOT/Lua `1.936x`，AOT/VM `1.183x`。相对上一轮 `1.649x` 有小幅正向；其中
  `string_key_hash` 的 VM 从 `3.118x` 到 `2.737x`，符合 string for-in dispatch 降低方向。
  AOT 曾因 `StrIndex -> StrLen` 未 deferred materialize 回退到 `2.022x` 几何均值，补齐 typed
  len feeder 后回到 `1.936x`。距离 `1.1x` 仍明显不足，下一轮应继续处理 list/map/string
  container op 数量和 AOT helper-heavy map/string path。

## 本轮记录：阶段 5 typed for-in source register ownership

范围：阶段 5，继续收口 facts-proven typed `for-in` 的 ownership 成本。上一轮 profile 显示
`string_key_hash` 虽然已经从 `ToIter + Len + Index` 降到 `StrLen + StrIndex`，但仍有
`LocalHeap=5000` 和 `LoadHeap=5000`。检查 compiler 后确认 typed `for-in` 在 iterable 是变量时
仍先调用 `self.expr(iterable)`，这会重新生成一次 `LoadLocal`，把源字符串从 local slot heap-copy
到临时寄存器。由于 typed lowering 已经在 body 会修改 iterable 变量时保守回退，非 mutating
变量 iterable 可以直接复用已有源寄存器。

本轮完成：

- typed `for-in` 对 `Expr::Var` iterable 优先复用 `lookup(name)` 的源寄存器；只有查不到时才
  回退到普通表达式 lowering。
- 继续保留 body mutates iterable 的回退条件，避免破坏旧的快照/别名语义边界。
- `string_for_in_uses_typed_len_and_index` regression 增加 `LoadLocal` 禁止断言，保证
  facts-proven string for-in 不再为源变量生成局部 heap copy。

验证结果：

- `cargo test -p lk-core vm::compiler::tests::string_fast_paths -- --nocapture`：4 passed。
- `cargo test -p lk-core vm::compiler::tests::list_fast_paths -- --nocapture`：13 passed。
- `cargo test -p lk-core`：898 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1149 code32_words=2026`。
- `PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed，`string_key_hash` copy profile 从
  上一轮 `LocalHeap=5000`、`LoadHeap=5000` 降到 `LocalHeap=0`、`LoadHeap=0`；该 workload
  只剩 baseline 级 `Clones=1221`、`HeapClone=199`、`CopyHeap=96`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.611x`，AOT/Lua `1.931x`，AOT/VM `1.198x`。`string_key_hash` 为 `2.563x`，比上一轮
  `2.737x` 继续下降，但仍触发相对旧 documented baseline `2.298x` 的 regression 警告。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`loop_lowering.rs` 325 行、`string_fast_paths.rs` 101 行、`list_fast_paths.rs`
  334 行、`core/src/llvm/tests.rs` 1499 行，均未新增超过 1500 行的文件。

结论：本轮是一个明确的架构层 ownership 修正，已经用 copy profile 证明消除了 typed
string for-in 的局部 heap copy；wall time 也有小幅正向。但整体 VM/Lua 仍为 `1.611x`，
距离 `1.1x` 仍明显不足。下一轮应继续优先处理 profile 中最大的通用容器成本：`sliding_window_sum`
的 list index/push 路径、`histogram_group_count`/`two_sum_map` 的 map get/set 路径，以及
`stock_max_profit`/`matrix_3x3_multiply` 这类 scalar loop 的 dispatch/branch 成本。

## 本轮记录：阶段 6 integer literal loop-invariant hoisting

范围：阶段 6，修正 loop-invariant hoisting 的基础字面量规则。此前不可变 literal hoisting
已经覆盖 `Nil`、`Bool`、`Float`、`String`，但漏掉 `Int`，导致 scalar loop 里大量整数
常量每次迭代重复 `LoadK`。`matrix_3x3_multiply` 是最明显受害者：矩阵常量在每轮循环中
反复 materialize，而不是在 loop guard 之前共享。

本轮完成：

- `expr_is_immutable_literal` 纳入 `Val::Int`，使整数 literal 可参与 loop-invariant hoisting。
- 新增 `loop_invariant_let_regs`，允许未被重新赋值的 loop-local immutable literal `let` 绑定复用
  已 hoisted 的 expression register；这覆盖 `let a = 2; total += i * a` 这类通用 scalar loop。
- 修复语义边界：range start 永远 materialize 到 fresh register，不能命中 invariant cache，因为
  `ForRangeLoop` 会把 start register 当作可变 idx 推进。
- 新增 regression：整数 literal let 在 range loop body 中不再生成 `LoadK`；同时回归验证
  之前的 branching map helper case，防止 hoisted binding 被错误复用后改变 checksum。

验证结果：

- `cargo test -p lk-core range_loop_hoists_immutable_integer_literals -- --nocapture`：1 passed。
- `cargo test -p lk-core loop_compound_call_map_param_facts_survive_branching_helper_body -- --nocapture`：1 passed。
- `cargo test -p lk-core`：899 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1212 code32_words=2156`。
- release bytecode dump 验证：`matrix_3x3_multiply` 示例中原本循环体内的整数 `LoadK` 被移到
  `RangeLoopI` 之前；循环体从约 42 个 opcode 降到约 28 个 opcode。
- `PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed 且 checksum 全部一致；
  `matrix_3x3_multiply` VM opcode 从上一轮约 `504217` 降到 `270230`，该 workload VM/Lua
  从上一轮约 `4.109x` 降到 `2.616x`。
- `bench/run_workload_bench.sh`：passed，6 samples per engine；VM/Lua geometric mean
  `1.597x`，AOT/Lua `2.065x`，AOT/VM `1.293x`。相对上一轮 `1.611x` 是小幅正向，但 AOT
  因 hoisted literal 增加静态 register/LoadK pressure 而明显退化，需要后续单独处理 AOT
  scalar lowering/register allocation 成本。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`loop_invariants.rs` 564 行、`stmt.rs` 1411 行、`expr.rs` 997 行、
  `builder.rs` 901 行、`general_optimizations.rs` 1467 行；均低于 1500 行。后续不要继续往
  `general_optimizations.rs` 增加测试。

结论：本轮完成了一个基础编译期架构修正，明显降低 scalar loop 中重复整数常量 materialization，
并在 `matrix_3x3_multiply` 上给出强正向证据。整体 VM/Lua 仍为 `1.597x`，距离 `1.1x`
仍不足。下一轮应优先处理不会恶化 AOT 的通用降本：list push/index 的 packed/runtime 成本、
map string-int key 的 VM materialization，以及 AOT 对 hoisted literal/register pressure 的处理。

## 本轮记录：阶段 6 packed access-int arithmetic dead temp write elision

范围：阶段 6，收紧 BC32 packed hot slot 中 `Access/ListIndex -> IntArith` fusion 的寄存器写入成本。
上一轮 profile 显示 `sliding_window_sum` 等 list-heavy workload 仍有大量 list access 和算术组合。
本轮不改变语言语义，也不做 workload-specific lowering；只让 packed decoder 利用已有 decoded
liveness，在 access 临时寄存器被后续证明为 dead 时，允许 fused fast path 直接用 access 出来的
整数参与算术，省掉中间 `access_dst` 写入。

本轮完成：

- `PackedHotKind::AccessIntArith` 增加 `write_access_dst`，由 decoder 根据 `regs_dead_after_pc`
  判断 fused arithmetic 之后 access 临时值是否还活跃。
- `Access + IntArith` 和 `ListIndex + IntArith` 两条 packed fusion 都接入同一写入策略；
  临时寄存器后续仍被读取时继续写入，避免破坏 liveness 语义。
- runtime fast path 只有在纯整数算术成功时才省略 access 临时写入；如果 access 读出的是 `Int`
  但后续算术需要 generic fallback（例如另一侧是 `Float`），会先恢复 `access_dst` 再调用通用算术。
- 增加 regression：dead access temp 的 fusion 标记为 `write_access_dst=false`；live temp 保持
  `write_access_dst=true`；generic fallback 覆盖 `Int access + Float add`，防止读到旧寄存器值。

验证结果：

- `cargo test -p lk-core packed_access_int_arith -- --nocapture`：3 passed。
- `cargo test -p lk-core pricing_helper_keeps_two_map_param_value_facts -- --nocapture`：1 passed。
- `cargo test -p lk-core`：901 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1212 code32_words=2156`。
- `RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed 且 checksum
  全部一致；VM/Lua geometric mean `1.583x`，AOT/Lua `2.056x`，AOT/VM `1.299x`。
  相对上一轮文档基线 `1.597x` 是小幅正向；`sliding_window_sum` 仍为 `2.413x`，说明 list-heavy
  主瓶颈不在这一个临时寄存器写入，而是在 container access/opcode 数量和 list/map helper 成本。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`decode.rs` 1489 行、`hot_values.rs` 906 行、`decode_tests.rs` 1282 行、
  `quickening/tests.rs` 464 行、`packed.rs` 609 行；均低于 1500 行。

结论：本轮是一个小而完整的 runtime architecture cleanup：它把 packed fusion 的临时写入和
decoded liveness 连接起来，语义回归已覆盖 generic fallback，整体 benchmark 也有小幅正向。
但距离 `1.1x` 仍明显不足。下一轮应把重点放回更基础的通用成本：list/index/push 的 container
helper 数量、map dynamic string-int key 路径，以及 AOT/VM 对 hoisted literals 后 register
pressure 的差异。

## 本轮记录：阶段 6 packed mul-add-mod arithmetic fusion

范围：阶段 6，继续降低通用整数循环中的 packed dispatch 与临时寄存器写入成本。当前
`stock_max_profit`、`string_key_hash`、`order_score_pipeline` 等 workload 都包含常见的
线性同余/哈希式整数表达式，例如 `(x * a + b) % m`。本轮不新增语言语义，也不改变静态
bytecode；只在 BC32 decoded hot slot 层把 `MulInt -> AddInt -> ModInt` 识别为一个
packed fusion，并用 liveness 确认两个中间寄存器在最终 `ModInt` 后死亡时才省略临时写入。

本轮完成：

- 新增 `PackedHotKind::MulIntAddIntModInt`，覆盖 `MulInt` 结果被 `AddInt` 消费、再作为
  `ModInt` 左操作数的三段整数算术链。
- `decode_mul_int_hot_slot` 抽出 `MulInt` 相关 hot-slot decode，避免 `decode.rs` 继续膨胀；
  `decode.rs` 从 1500 行风险降到 1399 行。
- fusion 只在 decoded liveness 证明 `mul_dst` 和 `add_dst` 死亡时启用；如果输入不是纯
  `Int`，runtime fallback 会按原顺序执行 `MulInt`、`AddInt`、`ModInt`，保留中间写入和动态语义。
- 增加 regression：decode 层验证 dead-temp 三段算术链会生成 `MulIntAddIntModInt`；
  quickening/runtime 层验证线性同余表达式在 packed 路径上执行正确并命中 hot cache。

验证结果：

- `cargo test -p lk-core packed_hot_slot_fuses_mul_add_mod_int_when_temps_are_dead -- --nocapture`：1 passed。
- `cargo test -p lk-core packed_mul_add_mod_int_fuses_linear_congruential_arithmetic -- --nocapture`：1 passed。
- `cargo test -p lk-core`：903 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1212 code32_words=2156`。
- `RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed 且 checksum
  全部一致；VM/Lua geometric mean `1.575x`，AOT/Lua `2.061x`，AOT/VM `1.308x`。
  相对上一轮 `1.583x` 是小幅正向，但 `stock_max_profit` 本身仍为 `3.657x`，说明该
  workload 的主成本不是这一条三段 fusion，或者收益被 dispatch/loop overhead 与测量噪声吞掉。
- `LK_DUMP_PACKED_STATS=1 LK_WORKLOAD_FILTER=stock_max_profit target/debug/lk bench/workloads_business_algorithms.lk`：
  stock 主运行段出现 `hits=3806984`、`build_successes=56`，确认 packed hot cache 在该 workload
  中活跃。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`decode.rs` 1399 行、`hot_values.rs` 963 行、`decode_tests.rs` 1330 行、
  `quickening/tests.rs` 508 行、`packed.rs` 619 行、`decode/fusions.rs` 677 行，均低于 1500 行。

结论：本轮把常见整数算术链纳入 liveness-gated packed fusion，并顺手降低了 `decode.rs`
的文件规模风险；整体 benchmark 有小幅正向，但距离 `1.1x` 仍明显不足。下一轮不应继续只做
单条 arithmetic fusion，应回到 profile 中更大的结构性成本：`sliding_window_sum` 的 list
index/list push、`two_sum_map`/`histogram_group_count` 的 map get/set 和动态 string key，
以及 AOT 对 VM 已优化路径的落差。

## 本轮记录：阶段 7 string-int map key materialization elision

范围：阶段 7，补齐 `PerformanceFacts` 中已有的 string-int key fact 在 VM runtime 的消费。
此前 liveness 已能识别 `"prefix" + int` / template key，并且 AOT backend 已使用这些 facts
生成不物化 key 的 map helper 调用；VM opcode/packed runtime 仍然只看已经构造好的 key
register，导致 `two_sum_map` 一类动态 string key workload 还要反复执行 `StrConcatToStr`。

本轮完成：

- `Val::cached_str_int_key(prefix, suffix)` 增加线程本地 bounded cache，用于 fact fallback
  路径按常量 prefix 和 int suffix 构造可复用 `ArcStr` key；cache 达到 4096 项后清空，避免
  长时间运行时无界增长。
- `MapGetDynamic`、`MapHas`、`MapSet`、`MapSetMove` 的 opcode 与 BC32 packed hot path
  都能读取 `PerformanceFacts.key_ops[*].string_int`；当前已物化 key 仍优先走原路径，避免
  给普通 map key 热路径增加 TLS cache 查询。
- `annotate_dead_write_facts` 增加 string-int key materialization elision：当
  `StrConcatToStr` 的结果只被带同一 string-int fact 的 map key op 消费，且 concat 目标不是
  suffix register 时，标记该 concat 为 dead write。
- runtime 保留原 bytecode/BC32 source pc，不把 concat 改成 `Nop`；执行时看到 dead-write
  `StrConcatToStr` 会把目标寄存器置为 `Nil`，强制后续 map op 使用 string-int fact fallback。
  这样保持 facts/source-pc 对齐，也避免 prefix register 被误当成已物化 key。
- 增加 regression：字节码层分别覆盖 opcode 与 packed map dynamic 消费 string-int key fact；
  liveness 层覆盖“只被 fact-aware map consumer 使用时可 elide”和“仍有普通读取时必须保留”。

验证结果：

- `cargo test -p lk-core string_int_key -- --nocapture`：12 passed。
- `cargo test -p lk-core map_set_consumes_temporary_key_value_registers -- --nocapture`：1 passed。
- `cargo test -p lk-core`：907 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1212 code32_words=2156`。
- `RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed 且 checksum
  全部一致；VM/Lua geometric mean `1.608x`，AOT/Lua `2.055x`，AOT/VM `1.278x`。
  `two_sum_map` 从上一记录的 `1.335x` 改到 `1.197x`，说明跳过 string key materialization
  对该路径有效；但 `histogram_group_count` 本轮为 `2.473x`，显著拖累几何均值，因此不能把
  本轮称为整体性能进步。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`liveness.rs` 1479 行、`hot_exec.rs` 1351 行、`container_ops.rs` 703 行、
  `vm_test/bytecode.rs` 1051 行、`strings.rs` 221 行，均低于 1500 行。

结论：本轮把 VM runtime 和既有 string-int key facts 接通，并在 `two_sum_map` 上拿到了明确
收益；但 `histogram_group_count` 的 map/string profile 仍不接受，整体几何均值暂时回退到
`1.608x`。下一轮应直接审计 `histogram_group_count` 的实际 bytecode/facts，确认为什么它没有
从 key materialization elision 获益，重点检查 key producer 是否跨 branch/loop 边界导致
dead-write elision 不触发，以及 `MapGetDynamicUpsertAdd` 是否还在重复做 lookup/key 构造。

## 本轮记录：阶段 8 upsert diamond string-int key elision

范围：阶段 8，继续补齐 string-int key facts 与真实 map upsert 控制流之间的连接。上一轮
`two_sum_map` 已验证单一路径 map key materialization elision 有效，但 `histogram_group_count`
仍很慢。复核当前 bytecode 后确认该 workload 的核心形态是：
`StrConcatToStr -> MapGetDynamic -> CmpEq nil -> BoolBranch -> MapSet / AddIntImm -> MapSet`。
`annotate_dead_write_facts` 之前遇到 `BoolBranch` 就停止，因此不会把 key concat 标为 dead write；
同时 branch target 会清空 key tracking，导致两个 `MapSet` 分支不能稳定继承同一 string-int key fact。

本轮完成：

- `annotate_key_facts` 增加 upsert diamond key fact propagation：当 `MapGetDynamic` 后面是
  conservative 的 get/nil-check/set/update-set diamond，两个 `MapSet` 分支会继承同一
  `PerfKeyFact`。
- `annotate_dead_write_facts` 增加 upsert diamond 判定：如果 `StrConcatToStr` 的结果只流入该
  upsert diamond，且最终 map update 后 key register 死亡，就把该 concat 标记为 dead write。
- 判定仍保守限制在同一 map、同一 key register、compare 读取 get 结果、update 表达式读取 get
  结果的形态；不跨任意控制流推广，避免把仍有普通读取的 key materialization 错删。
- 将 string-int key elision 相关 liveness tests 拆到 `core/src/vm/liveness/key_elision_tests.rs`，
  保持 `liveness.rs` 低于 1500 行。
- 增加 regression：upsert diamond 两个 map set 分支继承 string-int key fact；对应
  `StrConcatToStr` 会被标记为 dead write；普通读取仍保留 materialization。

验证结果：

- `cargo test -p lk-core string_int_key -- --nocapture`：14 passed。
- `cargo test -p lk-core`：909 passed, 3 ignored；doc-tests 0 passed。
- `cargo build --release -p lk-cli`：passed。
- `cargo run -p lk-cli -- coverage bench/workloads_business_algorithms.lk`：
  `AOT entry: native-lowerable`，VM totals `functions=9 packed=9/9 ops=1212 code32_words=2156`。
- `RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`：passed 且 checksum
  全部一致；VM/Lua geometric mean `1.558x`，AOT/Lua `2.071x`，AOT/VM `1.330x`。
  `histogram_group_count` 从上一轮 `2.473x` 降到 `1.501x`，说明 upsert diamond elision
  命中了实际瓶颈；`two_sum_map` 保持在 `1.202x` 附近。
- `cargo fmt --all -- --check`：passed。
- 行数检查：`liveness.rs` 1446 行、`liveness/key_elision_tests.rs` 152 行、
  `hot_exec.rs` 1351 行，均低于 1500 行。

结论：本轮把上一轮只覆盖 straight-line map consumer 的 key elision 扩展到通用
get/nil-check/set/update-set upsert diamond，显著修复了 `histogram_group_count`，整体 VM/Lua
几何均值从 `1.608x` 到 `1.558x`。目标仍未完成：`sliding_window_sum`、`stock_max_profit`、
`string_key_hash`、`route_permission_check` 等仍明显慢于 Lua。下一轮优先处理不会依赖单个
workload 名称的基础成本：list index/push/rolling-window 的 container path，或 scalar loop
dispatch/branch 的剩余成本。
