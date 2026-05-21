# LK 通用 VM 性能优化计划

要优化通用性能,而不是特定任务;hot path放到之后做,现在需要架构层面上的改进;
直到lk的通用性能接近lua(1.1x);

## 项目约束

本计划遵守当前 `AGENTS.md` 约束：

- 及时更新文档，包括 `README`、`docs/` 下文件和 `AGENTS.md`。
- 不遵循最小修改原则；如果发现基础小优化，可以一起改。
- 单个文件不能超过 1500 行。
- 越基础越优先，越重要越优先。
- 可以参考 `lua-5.5.0/src/` 下的实现。
- 当前项目未发布，除非特别说明，不需要保持向前兼容。
- 除 LLVM 部分外不能使用 `unsafe`。
- 若触及 `website/`，需要同步 `website/src/spec/LANG.md` 和
  `website/src/spec/LANG_zh.md`，并运行 `cd website && bun run build`。

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

## 阶段 1：统一 PerformanceFacts

目标：把当前分散在 `FunctionBuilder`、peephole、AOT lowering 和 packed decoder
里的事实收敛成统一结构，挂到 `FunctionAnalysis` 或其旁路结构上。

需要记录的 facts：

- value/register type：`Int`、`Float`、`Bool`、`String`、`List`、`Map`、`Nil`、
  `Unknown`。
- container facts：list/map 的 value type、已知长度、是否来自空容器并可被首次写入
  采纳类型。
- key facts：const string key、interned key、template string-int key、已知 hash/id。
- call facts：exact call target、arity、return type、是否可 inline、是否可复用 frame
  layout。
- ownership facts：value 是否 escape、是否仅临时使用、是否可 move、是否必须 clone。
- liveness facts：register 在某个 pc 之后是否还会读取、临时值是否可跳过写入。
- invalidation facts：map/list mutation、branch merge、call side effect 后需要清掉哪些
  facts。

实现方式：

- 第一版只迁移当前已经能证明的 facts，不新增激进推断。
- 保留现有测试，新增 facts 级测试，直接断言同一源码能产出稳定 facts。
- 所有事实必须经过 block merge，不允许跨分支保留 stale fact。
- LKB round-trip 需要能保留必要的 `FunctionAnalysis`，不能让缓存文件丢失优化依据。

验收：

- `FunctionAnalysis` 能输出 typed lowering 所需 facts。
- `FunctionBuilder` 中的 `int_regs`、`float_regs`、`map_value_types` 等局部集合开始被
  `PerformanceFacts` 替代或同步生成。
- 新测试覆盖直线代码、if/else merge、loop mutation、call side effect。

## 阶段 2：typed lowering 架构化

目标：typed bytecode 由 `PerformanceFacts` 稳定生成，而不是每个 expr/compiler 分支
各自维护一套临时推断。

优先迁移已有能力：

- integer arithmetic：`AddInt`、`SubInt`、`MulInt`、`ModInt`、`AddIntImm`。
- typed compare/branch：`CmpIntJmp`、`Cmp*ImmJmp`、`AddIntImmJmp`。
- list：`ListIndexI`、`ListSetI`、typed list value feeding arithmetic。
- map：`MapGetInterned`、`MapGetDynamic`、`MapHasK`、typed map value feeding arithmetic。
- string：`StrLen`、`StartsWithK`、`ContainsK`、known-cap concat。

如何优化：

- 编译器先读取 facts，再决定生成 typed op；typed op 失败时保留 generic fallback。
- peephole 只做局部规范化，不再承载跨 block 或跨函数事实推断。
- AOT lowering 读取同一 facts，避免 VM 与 AOT 各自发明不同规则。
- 暂不新增 workload 专用 opcode；确实需要新 opcode 时，必须证明它是通用语言形态。

验收：

- 已有 typed op 的生成条件集中、可测试、可解释。
- block merge 后 stale typed op 不再出现。
- 同一源码在 VM bytecode 和 LLVM lowering 中使用同一事实来源。

## 阶段 3：Call Frame 通用化

目标：把 call fast path 从“很多入口各自处理”改成统一的 call site plan。

新增或收敛的概念：

```text
CallSitePlan
  exact target pointer / function id
  positional arity
  named argument layout
  return layout
  frame layout
  captures / capture specs
  frame info
  optional inline eligibility
```

如何优化：

- 第一次调用解析 closure/function 元数据，后续命中 IC 时复用 `CallSitePlan`。
- opcode 与 packed call 都只能进入共享 `call_common`，不能各自复制 fast path 逻辑。
- `TinyCallPlan` 保留为 leaf optimization，但不再继续扩张为 benchmark 专用执行器。
- 小函数 inline 由 SSA/facts 决定，而不是继续手写 GCD、binary search、prime 这类任务形态。
- named call 的 plan 也纳入统一 call site 缓存，减少每次构造 provided/default 映射。

验收：

- exact closure call 命中后不重复读取 closure metadata、frame captures、frame info。
- opcode path 和 packed path 的 call 行为由同一套测试覆盖。
- counters 能区分 call miss、IC hit、inline hit、generic fallback。

## 阶段 4：`Val` 与 register 写入成本

目标：减少通用动态值路径里的 clone/refcount 和无意义 register write。

如何优化：

- 引入统一写入策略：能 move 就 move，必须共享才 clone。
- `Int`、`Float`、`Bool`、`Nil`、short string 保持 cheap-copy。
- `Arc` 容器和 heap string 不进入短生命周期临时值路径。
- dead temp 消除由 liveness facts 驱动，不再写在某个 packed fusion 特例里。
- map/list/string 写入先判断 value ownership，优先原地更新或 move into container。
- register write 统一经过 helper，counter 和 debug 检查都在同一层完成。

验收：

- `Val::clone`、heap clone、register write 有稳定 counters。
- 临时寄存器后续不读取时，可以在 opcode/packed 两边一致跳过写入。
- list/map/string mutation 不因为跳过临时值写入改变可见语义。

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

每轮实现必须做：

1. 明确本轮属于哪个架构阶段。
2. 先补 facts/counters/tests，再改 lowering/runtime。
3. 避免新增 benchmark 专用分支；如果必须新增，先证明它是通用语言形态。
4. 更新 `plan.md` 本轮记录。
5. 若发生代码实现，运行 quick benchmark 并更新 `bench/README.md` 当前性能表。

常规验证命令：

```bash
cargo test -p lk-core
cargo fmt --all -- --check
cargo build --release -p lk-cli
git diff --check
```

行数检查：

```bash
rg --files -0 -g '!target/**' -g '!references/**' -g '!website/node_modules/**' \
  -g '!vsc-ext/lsp/node_modules/**' \
  | xargs -0 wc -l \
  | awk '$1 > 1500 { print }'
```

quick benchmark：

```bash
RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh
```

记录指标：

- wall time：VM/Lua、AOT/Lua、AOT/VM。
- dispatch：opcode steps、branches、typed branches。
- value movement：`Val` clones、heap clones、immediate clones、register writes。
- runtime fallback：generic op fallback、BC32 fallback、AOT helper fallback、call fallback。
- correctness：checksum、unit tests、doc build if touched。

## 当前历史状态

详细逐轮性能表以 `bench/README.md` 的 Latest Quick Comparison 为准；本节只保留阶段结论和最近落地记录，避免计划文件超过 1500 行。

- 阶段 1 已完成：`PerformanceFacts`、container/key/call/ownership/liveness facts 已进入 `FunctionAnalysis`，LKB 能保留必要 analysis，typed lowering、peephole、AOT 和 VM runtime 已逐步改为消费同一事实来源。
- 阶段 2 已开始：已有 integer arithmetic、typed compare/branch、list/map/string lowering 正在从局部集合迁移到 facts 查询；新增 typed op 必须证明是通用语言形态。
- 阶段 3 已开始：call target 分类、call site plan、positional call frame、named/default seed 和 register window 解析持续收敛；目标是让 opcode path 与 packed path 共享同一调用协议。
- 阶段 4 已开始：register copy policy、liveness-driven move/take、dead write、metrics gating 和 call-arg ownership 已接入通用 helper；继续减少 heap clone、register write 和 runtime scan。

最新验证（第一百三十轮）已通过：

- `cargo fmt --all -- --check`
- `cargo check -p lk-core`
- `cargo test -p lk-core arithmetic -- --nocapture`
- `cargo test -p lk-core packed -- --nocapture`
- `cargo test -p lk-core call -- --nocapture`
- `cargo test -p lk-core`
- `cargo build --release -p lk-cli`
- `git diff --check`
- 单文件行数检查：`core`、`cli`、`stdlib` 下 `.rs` 文件不超过 1500 行。
- `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh`
