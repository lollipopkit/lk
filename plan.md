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

最近落地：

- 第三十八轮：metrics 层新增 known-enabled 入口，generic opcode、packed BC32、packed hot slot 和 quickening 在 `collect_metrics` 后直接累计计数。profile quick VM/Lua `1.742x`，full quick `1.761x`。
- 第三十九轮：packed BC32 fallback metrics 迁移到 known-enabled 入口，删除旧 wrapper。profile quick `1.851x`，full quick `1.848x`。
- 第四十轮：验证并回退 dispatch 双版本实验，保留 register/container copy helper `#[inline(always)]`。profile quick `1.809x`，full quick `1.897x`。
- 第四十一轮：positional call argument copy 把 `RegisterWindowRef` 解析提前到循环外并删除 `Vm::read_reg()`。profile quick `1.820x`，full quick `1.819x`。
- 第四十二轮：named/default call seed 改为最终映射和 callee frame 写入时 move owned `Val`，只在 default thunk 需要保留前序 seed 时继续 copy；新增 metrics 测试确认 heap-backed named arg 的 `CallArg` clone 为 `1` 次。profile quick `1.835x`，full quick VM/Lua `1.790x`，AOT/Lua `1.909x`，AOT/VM `1.066x`。本轮 workload 对 named/default call 覆盖有限，性能证据以 metrics 测试为主。
- 第四十三轮：验证并回退 register-write metrics known-enabled 实验，因为 quick run 没有正向证据；保留的改动是 `supports_bc32_fast_path()` 优先消费已预构建的 `Bc32Decoded`，正常编译函数进入 frame 时不再每次线性扫描 `code32` 查找 `REG_EXT`。profile quick `1.840x`，full quick VM/Lua `1.862x`，AOT/Lua `1.961x`，AOT/VM `1.053x`。本轮是 call-frame/BC32 入口固定成本清理，single-sample wall time 未证明收益，后续需多样本确认。
- 第四十四轮：packed frame 入口的 cache 准备从对 `f.code.len()` 和 `code32.len()` 的两轮检查收敛成一次 `cache_len = max(...)`，`access_ic` 和 `for_range_ic` 只做一次长度判断。该改动不新增 opcode 或 workload 专用路径，只减少每次 packed frame 进入时的固定分支。profile quick `1.852x`，full quick VM/Lua `1.851x`，AOT/Lua `1.968x`，AOT/VM `1.063x`；结果仍按单样本方向观察处理。
- 第四十五轮：把 opcode 与 packed interpreter 入口的 IC 长度准备收敛到 `VmCaches::prepare_opcode_sites()` 和 `VmCaches::prepare_packed_sites()`。两条执行路径不再各自维护 access/index/global/call/for-range/packed-hot cache sizing 逻辑；第四十四轮的 mixed site length 规则也从 packed loop 移到共享 cache 层。profile quick `1.870x`，full quick VM/Lua `1.824x`，AOT/Lua `1.967x`，AOT/VM `1.078x`；该轮属于 call-frame/cache 协议收敛，单样本 wall time 只作方向观察。
- 第四十六轮：把 cache sizing impl 从接近 1500 行的 `caches.rs` 移到 `caches/runtime_sites.rs`，并让 `ClosureFastCache` 通过 `prepare_function_sites()`、`prepare_packed_hot_for_function()` 和 `reset_function_sites_for_key()` 统一处理 closure fast call 与 nested call 的 function-site cache 准备。`exec.rs` 不再手写 access/index/global/call/for-range/packed-hot/quickening 的 reset/resize 序列；`caches.rs` 降到 1404 行。profile quick `1.864x`，full quick VM/Lua `1.824x`，AOT/Lua `1.931x`，AOT/VM `1.059x`。
- 第四十七轮：nested call cache path 也改为调用 `ClosureFastCache::prepare_function_sites()` 与 `prepare_packed_hot_for_function()`，不再只在 function key 变化时 reset 后依赖 `run_frame` 补齐容量；删除已无调用的 `reset_function_sites_for_key()`。closure fast span 与 nested call 现在使用同一 function-site cache 准备协议。profile quick `1.774x`，full quick VM/Lua `1.831x`，AOT/Lua `1.932x`，AOT/VM `1.055x`；full quick 单样本略回退，按噪声处理，本轮验收重点是 cache 协议一致性。
- 第四十八轮：顶层 `exec_inner` 入口新增 `prepare_top_level_cache_keys()`，把 `packed_hot_ic` 与 `quickening_ic` 的 function-key 失效协议从局部手写分支收敛成统一入口，并在 cache 可变借用之前完成 key 准备。顶层 exec、closure fast span 与 nested call 现在都通过命名 helper 准备 runtime cache，不再散落 reset/clear 逻辑。profile quick `1.803x`，full quick VM/Lua `1.882x`，AOT/Lua `1.971x`，AOT/VM `1.047x`；本轮没有新增 hot-path 特例，full quick 单样本回退按噪声处理，验收重点是 cache key 协议一致性和后续 typed dispatch/call frame 改造的基础面。
- 第四十九轮：`VmCaches::prepare_opcode_sites()` 与 `ClosureFastCache::prepare_function_sites()` 现在同时预备 `quickening` site 容量，quickening 不再完全依赖各个 `execute_*_site()` 在运行中按 pc 懒扩容。新增 runtime cache 测试覆盖 opcode 与 closure function-site 的 quickening sizing。该改动仍属于 cache/typed dispatch 协议收敛，不新增 workload 专用 opcode 或 packed hot path。profile quick `1.835x`，full quick VM/Lua `1.836x`，AOT/Lua `1.950x`，AOT/VM `1.062x`；单样本 wall time 只作方向观察，验收重点是 quickening site 生命周期纳入统一 runtime cache 准备层。
- 第五十轮：`allocate_stack_window()` 改为区分复用槽位和新扩展槽位；`Vec::resize()` 已经把新槽位初始化为 `Nil`，因此窗口清理只覆盖 `[base, old_len.min(new_top))` 这一段旧槽位，避免扩栈时对新槽位二次写入。新增 stack window 测试覆盖旧槽位清理和新槽位 `Nil` 初始化。本轮验证中发现 `slice.fill(Val::Nil)` 会走 `Val::clone` 并污染 clone counters，已改回逐槽赋值以保持 copy metrics 稳定。profile quick `1.821x`，full quick VM/Lua `1.799x`，AOT/Lua `1.925x`，AOT/VM `1.071x`；该轮属于 call-frame/stack window 固定成本清理，不新增 workload 专用路径。
- 第五十一轮：`ClosureFastCache` 的 `region_plan` 缓存纳入 function-site 生命周期。`prepare_function_sites()` 在 function key 变化时会清掉 cached region plan，closure fast span 与 nested call 都先 prepare function-site，再通过 `region_plan_for_function()` 读取 region plan，避免旧 function 的 region plan 被复用到新 function。新增 runtime cache 测试覆盖 key change 后 region plan invalidation。本轮属于 call-frame/cache 正确性和生命周期收敛，不新增 workload 专用路径。profile quick `1.829x`，full quick VM/Lua `1.851x`，AOT/Lua `1.967x`，AOT/VM `1.063x`；single-sample wall time 有回退，按噪声处理，验收重点是消除 stale cache 风险。
- 第五十二轮：新增 `ClosureFastCache::prepare_function_runtime()`，把 function-site sizing、packed-hot function key、region plan 缓存读取收敛为单个函数级 runtime prepare 入口。closure fast span 与 nested call 不再各自连续调用三步准备逻辑；新增 runtime cache 测试确认该入口同时设置 prepared function key、packed-hot key 和 region plan。该轮属于 call-frame/cache 协议收敛，不新增 workload 专用路径。profile quick `1.836x`，full quick VM/Lua `1.831x`，AOT/Lua `1.919x`，AOT/VM `1.048x`；single-sample wall time 只作方向观察，验收重点是减少 runtime cache 准备协议分叉。
- 第五十三轮：顶层 `exec_inner` 入口改为调用 `Vm::prepare_top_level_runtime()`，把 top-level packed-hot key、quickening key 和 `RegionPlan` 读取收敛到 runtime cache 协议层。top-level exec、closure fast span 与 nested call 现在都有命名的 runtime prepare 入口，`exec.rs` 不再单独维护顶层 cache key helper。新增 runtime cache 测试确认顶层入口同时设置 cache keys 并返回 region plan。该轮属于 call-frame/cache/facts 生命周期收敛，不新增 workload 专用路径。profile quick `1.817x`，full quick VM/Lua `1.827x`，AOT/Lua `1.989x`，AOT/VM `1.089x`；single-sample wall time 只作方向观察，验收重点是减少顶层与函数调用入口的协议分叉。
- 第五十四轮：新增结构化 `FunctionRuntimePlan`，让 top-level exec、closure fast span 与 nested call 都从 runtime prepare 结果读取 function key、register count、code length 和 `RegionPlan`。`exec.rs` 不再在三个入口各自直接读取 `n_regs`/analysis region plan；function-site sizing 也改为消费 runtime plan 的 `code_len`。该轮属于 call-frame/facts 准备协议收敛，为后续统一 frame setup 和 typed dispatch plan 打基础，不新增 workload 专用路径。profile quick `1.866x`，full quick VM/Lua `1.826x`，AOT/Lua `1.923x`，AOT/VM `1.053x`；single-sample wall time 按噪声处理，验收重点是元数据读取入口统一且 counters 无异常漂移。
- 第五十五轮：`CallFrame::from_runtime()` 接入 `FunctionRuntimePlan`，top-level exec、closure fast span 与 nested call 的 frame 构造不再手写 `reg_count`/`region_plan` 参数组合，而是统一从 runtime plan 生成 frame layout。新增 frame 测试覆盖 runtime plan 进入 `CallFrame` 后的 reg base、reg count 与 region plan 传播。本轮属于 call-frame setup 协议收敛，不新增 workload 专用路径。profile quick `1.854x`，full quick VM/Lua `1.818x`，AOT/Lua `1.941x`，AOT/VM `1.068x`；single-sample wall time 只作方向观察，验收重点是 frame setup 入口统一且 copy/BC32 counters 无异常漂移。
- 第五十六轮：新增 `StackWindow`，并让 `Vm::allocate_runtime_stack_window()` 从 `FunctionRuntimePlan` 生成 stack window。top-level exec、closure fast span 与 nested call 现在都通过 runtime plan -> stack window -> `CallFrame::from_runtime()` 的同一 frame setup 链路，不再各自拆出 base/count 组合。该轮属于 call-frame/stack-window 协议收敛，不新增 workload 专用路径。profile quick `1.790x`，full quick VM/Lua `1.791x`，AOT/Lua `1.951x`，AOT/VM `1.089x`；single-sample wall time 只作方向观察，验收重点是 stack window 和 frame layout 入口统一且 counters 无异常漂移。
- 第五十七轮：新增 `FrameStateSetup`，并把 top-level exec、closure fast span 与 nested call 的 `FrameState` 构造收敛到 `FrameState::from_frame()`。同步 frame 通过 `FrameStateSetup::synchronized()` 显式声明 drop-back 行为，inline closure fast span 通过 `FrameStateSetup::inline_ephemeral()` 同时声明“不回写 call frame”和 inline return meta，不再先构造 ephemeral frame 后二次设置 return meta。该轮属于 call-frame/frame-state setup 协议收敛，不新增 workload 专用路径。profile quick `1.760x`，full quick VM/Lua `1.849x`，AOT/Lua `1.915x`，AOT/VM `1.035x`；single-sample wall time 只作方向观察，验收重点是 frame state 生命周期语义显式化，后续 typed branch/op dispatch plan 可以挂到同一 setup 入口。
- 第五十八轮：新增 `FrameActivation`，把 `FunctionRuntimePlan`、`StackWindow` 和 `FrameStateSetup` 收敛成一次 frame activation。top-level exec、closure fast span 与 nested call 现在都先构造 activation，再由 activation 生成 `CallFrame` 并提供 `FrameStateSetup`；三个入口不再分别拼装 runtime plan、window 和 setup。该轮属于 call-frame activation 协议收敛，不新增 workload 专用路径。profile quick `1.836x`，full quick VM/Lua `1.843x`，AOT/Lua `1.970x`，AOT/VM `1.069x`；single-sample wall time 只作方向观察，验收重点是 frame activation 成为后续 typed branch/op dispatch plan、call frame metadata 和 cache lifecycle 的统一挂点。
- 第五十九轮：把 positional/named 参数 seed 也纳入 activation/frame 协议。`FrameActivation::seed_positional_from_stack()` 统一处理 caller register window 到 callee stack window 的 positional 参数复制，closure fast span 与 nested call 不再各自手写 source-base 解析和 copy policy；`FrameState::seed_positional_from_values()` 与 `seed_named_call_arg_values()` 统一 top-level/绑定入口的参数写入和 bounds check。该轮属于 call-frame/register-window 协议收敛，不新增 workload 专用路径。profile quick `1.801x`，full quick VM/Lua `1.851x`，AOT/Lua `1.975x`，AOT/VM `1.067x`；single-sample wall time 只作方向观察，验收重点是参数 seed、register write helper 和 call-arg copy policy 进入同一基础层。
- 第六十轮：把可选 `VmContext` call frame 的 push/pop/error wrapping 收敛成 `push_optional_call_frame()` 和 `finish_optional_call_frame()`。top-level exec 与 nested call 不再各自复制“成功 pop、失败生成 call stack report 后 pop”的诊断协议；后续 call path 接入 frame info 时只需进入同一 helper。该轮属于 call-frame diagnostics/lifecycle 协议收敛，不新增 workload 专用路径。profile quick `1.795x`，full quick VM/Lua `1.793x`，AOT/Lua `1.884x`，AOT/VM `1.051x`；single-sample wall time 只作方向观察，验收重点是调用栈错误语义和 frame lifecycle 入口统一。
- 第六十一轮：`FrameActivation` 新增 `into_parts()`，一次性产出 `StackWindow`、`CallFrame` 和 `FrameStateSetup`。top-level exec、closure fast span 与 nested call 不再各自拆 `window/setup/call_frame`，旧的 `setup()`/`into_call_frame()` API 已删除，activation 成为 frame materialization 的唯一入口。本轮仍属于 call-frame lifecycle/typed dispatch 挂点收敛，不新增 workload 专用路径。profile quick `1.751x`，full quick VM/Lua `1.810x`，AOT/Lua `1.969x`，AOT/VM `1.088x`；single-sample wall time 只作方向观察，验收重点是 frame activation 到 frame state 的协议分叉继续减少。
- 第六十二轮：`FrameState` 拆分 function lifetime 与 register-slice lifetime，并让 `FrameActivationParts::frame_state()` 成为统一的 frame-state 创建入口。top-level exec、closure fast span 与 nested call 不再各自手写 `stack[base..base+reg_count]`、`reg_count` 和 `FrameState::from_frame()`，typed dispatch/op helpers 也统一改为双 lifetime 的 `FrameState<'_, '_>` raw pointer 类型。本轮属于 call-frame/frame-state 生命周期协议收敛，不新增 workload 专用路径。profile quick `1.785x`，full quick VM/Lua `1.879x`，AOT/Lua `2.032x`，AOT/VM `1.081x`；single-sample wall time 只作方向观察，验收重点是解除不必要 lifetime 耦合，让后续 frame state/dispatch plan 能从 activation parts 统一挂载。
- 第六十三轮：`StackWindow::base()` 成为 stack-window base 的统一读取口，`FrameActivationParts::stack_base()` 和 positional seed 都通过该接口读取 base；closure fast span 与 nested call 的 stack release base 也改为从 activation parts 获取。`FrameActivation::window()` 已删除，exec 入口不再直接读取 activation window 字段。本轮属于 call-frame/stack-window 生命周期协议收敛，不新增 workload 专用路径。profile quick `1.850x`，full quick VM/Lua `1.883x`，AOT/Lua `2.010x`，AOT/VM `1.068x`；single-sample wall time 只作方向观察，验收重点是 release/seed/frame-state 都通过同一 stack-window abstraction。
- 第六十四轮：`ClosureFastCache::vm_caches()` 成为 closure runtime cache 到 `VmCaches` 的统一视图。closure fast span 与 nested call 不再各自手写 access/index/global/call/for-range/packed-hot/quickening 七个字段映射，exec 入口只请求 cache layer 提供运行时 cache view。本轮属于 runtime cache lifecycle/typed dispatch 协议收敛，不新增 workload 专用路径。profile quick `1.795x`，full quick VM/Lua `1.843x`，AOT/Lua `1.978x`，AOT/VM `1.073x`；single-sample wall time 只作方向观察，验收重点是 closure cache layout 与执行入口解耦。
- 第六十五轮：新增 `RuntimeCacheStore`，把 top-level VM 的 access/index/global/call/for-range/packed-hot/quickening cache 字段收敛为单个 runtime cache store，并提供与 `ClosureFastCache::vm_caches()` 对称的 `vm_caches()` 视图。top-level exec 不再手写 `VmCaches` 七字段映射，`prepare_top_level_runtime()` 也直接通过 store 管理 packed-hot/quickening key。本轮属于 runtime cache lifecycle/typed dispatch 协议收敛，不新增 workload 专用路径。profile quick `1.808x`，full quick VM/Lua `1.766x`，AOT/Lua `1.940x`，AOT/VM `1.099x`；single-sample wall time 只作方向观察，验收重点是 top-level cache layout 与 closure cache layout 收敛到同一抽象。
- 第六十六轮：新增 `InstructionSiteCaches`，把 top-level `RuntimeCacheStore` 和 closure `ClosureFastCache` 共享的 access/index/global/call/for-range/packed-hot/quickening site vectors 收敛成同一个基础结构。两类 runtime cache 现在都通过 shared site store 暴露 `VmCaches` 视图，function key 切换也统一调用 shared site clear，减少后续 typed branch/op dispatch plan 需要适配的 cache layout 分叉。本轮属于 runtime cache lifecycle/typed dispatch 协议收敛，不新增 workload 专用路径。profile quick `1.823x`，full quick VM/Lua `1.816x`，AOT/Lua `1.950x`，AOT/VM `1.074x`；single-sample wall time 只作方向观察，验收重点是 top-level 与 closure site cache 的物理布局统一。
- 第六十七轮：把 `RuntimeCacheStore`、`InstructionSiteCaches` 和 `VmCaches` 从接近 1500 行的 `caches.rs` 搬到 `caches/runtime_sites.rs`，让 runtime cache 数据结构、site sizing、function key invalidation 和 `VmCaches` 视图位于同一模块。`caches.rs` 从 1470 行降到 1388 行，后续 typed branch/op dispatch plan 可以继续扩展 runtime-sites 层而不让主 cache 类型文件再次顶到行数上限。本轮属于 runtime cache lifecycle/typed dispatch 模块边界收敛，不新增 workload 专用路径。profile quick `1.786x`，full quick VM/Lua `1.788x`，AOT/Lua `1.931x`，AOT/VM `1.080x`；single-sample wall time 只作方向观察，验收重点是 cache 基础结构归位和后续架构扩展空间。
- 第六十八轮：`FunctionRuntimePlan` 记录 `code32_len`，并把 runtime site lengths 带入 `CallFrame`/`FrameState`。`run_frame()` 现在从 frame runtime metadata 读取 opcode/BC32 site 长度，再传给 opcode 与 packed 执行器做 cache sizing；执行器不再直接用 `func.code.len()`/`code32.len()` 拼装 cache prepare 参数。本轮属于 function runtime metadata 与 typed dispatch cache sizing 协议收敛，不新增 workload 专用路径。profile quick `1.834x`，full quick VM/Lua `1.815x`，AOT/Lua `1.976x`，AOT/VM `1.089x`；single-sample wall time 只作方向观察，验收重点是执行器消费统一 runtime plan 元数据。
- 第六十九轮：`run_frame()` 成为 runtime dispatch cache 准备入口，进入 packed path 前统一调用 `prepare_packed_sites(opcode_len, packed_len)`，进入 opcode path 前统一调用 `prepare_opcode_sites(opcode_len)`。`run_opcode_code()` 与 `run_packed_code()` 不再接收原始 code length，也不再自行准备 cache site 容量；cache lifecycle 从具体执行器内层上移到 frame dispatch 边界。本轮属于 call-frame/runtime cache/typed dispatch 协议收敛，不新增 workload 专用路径。profile quick `1.877x`，full quick VM/Lua `1.868x`，AOT/Lua `1.953x`，AOT/VM `1.045x`；single-sample wall time 只作方向观察，验收重点是 cache sizing 只消费 frame runtime metadata，opcode 与 packed 执行器协议继续收窄。
- 第七十轮：新增 `RuntimeDispatchSites`，把 opcode 长度和 packed/BC32 长度从匿名 tuple/raw `usize` 参数提升为结构化 runtime metadata。`FunctionRuntimePlan`、`CallFrame` 和 `FrameState` 现在携带同一份 dispatch site 描述，`VmCaches::prepare_opcode_sites()`、`prepare_packed_sites()` 和 closure `prepare_function_sites()` 都消费该结构；旧的 `code_len`/`code32_len` 字段从 frame 状态移除。本轮属于 runtime cache lifecycle 与 typed dispatch metadata 收敛，不新增 workload 专用路径。profile quick `1.865x`，full quick VM/Lua `1.877x`，AOT/Lua `1.977x`，AOT/VM `1.053x`；single-sample wall time 只作方向观察，验收重点是 typed branch/op dispatch 后续可挂到结构化 runtime dispatch metadata。
- 第七十一轮：`RuntimeDispatchSites` 继续承载 packed fast-path eligibility。`run_frame()` 不再本地扫描 `Function` 判断 `named_param_layout`、`code32` 和 `REG_EXT`，而是只读取 frame runtime metadata 的 `packed_enabled()`；`FunctionRuntimePlan` 在 runtime prepare 阶段统一计算该决策，并新增测试覆盖 named 参数禁用 packed path、REG_EXT 缺少 decoded table 时禁用 packed path。本轮属于 runtime dispatch decision metadata 收敛，不新增 workload 专用路径。profile quick `1.881x`，full quick VM/Lua `1.884x`，AOT/Lua `2.011x`，AOT/VM `1.067x`；single-sample wall time 只作方向观察，验收重点是 packed eligibility 从执行器判断下沉到统一 runtime plan。
- 第七十二轮：`ClosureFastCache` 缓存 `RuntimeDispatchSites`，并把该缓存纳入 function-site key 生命周期。closure fast span / nested call 的 `prepare_function_runtime()` 不再每次通过 `FunctionRuntimePlan::from_function()` 重新扫描函数元数据和 BC32 packed eligibility；function key 切换时同时清掉 site caches、packed hot key、region plan 和 dispatch metadata。本轮属于 call-frame/runtime metadata cache 收敛，不新增 workload 专用路径。profile quick `1.828x`，full quick VM/Lua `1.827x`，AOT/Lua `1.930x`，AOT/VM `1.056x`；single-sample wall time 只作方向观察，验收重点是 closure runtime prepare 复用统一 dispatch metadata。
- 第七十三轮：`RuntimeCacheStore` 也缓存 top-level `RuntimeDispatchSites`，并用 function key 管理该缓存生命周期。`Vm::prepare_top_level_runtime()` 不再每次通过 `FunctionRuntimePlan::from_function()` 重建 dispatch metadata，而是复用 runtime cache store 的 `dispatch_sites_for_function()` 后构造 `FunctionRuntimePlan`；`from_function()` 限定为测试 helper。top-level exec、closure fast span 与 nested call 现在都通过缓存层复用统一 dispatch metadata。本轮属于阶段 1/3 的 runtime metadata cache 收敛，不新增 workload 专用路径。profile quick `1.859x`，full quick VM/Lua `1.911x`，AOT/Lua `2.004x`，AOT/VM `1.049x`；single-sample wall time 出现回退，按噪声处理，验收重点是 top-level runtime prepare 不再重复扫描函数元数据和 BC32 packed eligibility。
- 第七十四轮：`RuntimeCacheStore` 继续缓存 top-level `RegionPlan`，并和 top-level `RuntimeDispatchSites` 共享同一个 function-key invalidation。`Vm::prepare_top_level_runtime()` 不再每次直接从 `FunctionAnalysis` 读取并 clone region plan，而是通过 runtime cache store 的 `region_plan_for_function()` 复用 frame metadata；function key 切换会同时清掉 dispatch metadata 和 cached region plan。本轮属于阶段 1/3 的 runtime metadata/cache lifecycle 收敛，不新增 workload 专用路径。profile quick `1.844x`，full quick VM/Lua `1.843x`，AOT/Lua `1.939x`，AOT/VM `1.052x`；single-sample wall time 只作方向观察，验收重点是 top-level 与 closure runtime prepare 的 region metadata 生命周期对齐。
- 第七十五轮：`RuntimeCacheStore` 新增 `prepare_function_runtime()`，把 top-level dispatch metadata、region plan、packed-hot key 和 quickening key 的准备收敛到 store 级入口；`Vm::prepare_top_level_runtime()` 现在只做薄转发，不再手写 runtime plan 组装和 function-key cache 清理。`dispatch_func_key` 同步改名为 `metadata_func_key`，反映 dispatch sites 与 region plan 共享同一元数据生命周期。本轮属于阶段 1/3 的 runtime prepare 协议收敛，不新增 workload 专用路径。profile quick `1.815x`，full quick VM/Lua `1.853x`，AOT/Lua `1.937x`，AOT/VM `1.045x`；single-sample wall time 只作方向观察，验收重点是 top-level 与 closure 都有 cache store 级 runtime prepare 入口。
- 第七十六轮：新增共享 `FunctionMetadataCache`，把 `RuntimeDispatchSites` 与 `RegionPlan` 的 function-key 生命周期从 top-level `RuntimeCacheStore` 和 closure `ClosureFastCache` 的散字段中抽出来。top-level exec、closure fast span 与 nested call 现在复用同一个 metadata cache 类型准备 dispatch metadata 和 region plan，后续 typed branch/op dispatch plan 只需要挂到同一基础结构。本轮属于阶段 1/3 的 runtime metadata/cache lifecycle 收敛，不新增 workload 专用路径。profile quick `1.798x`，full quick VM/Lua `1.837x`，AOT/Lua `1.957x`，AOT/VM `1.065x`；single-sample wall time 只作方向观察，验收重点是 top-level 与 closure runtime metadata 生命周期统一。
- 第七十七轮：`FunctionRuntimePlan` 本身纳入共享 `FunctionMetadataCache`，top-level `RuntimeCacheStore` 和 closure `ClosureFastCache` 不再各自组装 dispatch sites、region plan、reg count 和 function key。旧的 closure `dispatch_sites_for_function()` / `region_plan_for_function()` wrapper 已删除，运行时入口统一从 metadata cache 取得完整 function runtime plan。本轮属于阶段 1/3 的 runtime metadata/cache lifecycle 收敛，不新增 workload 专用路径。profile quick `1.883x`，full quick VM/Lua `1.921x`，AOT/Lua `2.050x`，AOT/VM `1.067x`；single-sample wall time 回退按噪声记录，验收重点是 runtime plan 生命周期统一。
- 第七十八轮：`CallFrame::from_runtime()` 改为直接用 `FunctionRuntimePlan` 构造 frame，不再先调用 `CallFrame::new()` 触发一次 `RuntimeDispatchSites::from_function()` 扫描后再覆盖 dispatch metadata。裸 `CallFrame::new()` 现在必须显式传入 `RuntimeDispatchSites`，避免 frame setup 路径绕过 runtime metadata cache。本轮属于阶段 1/3 的 call-frame/runtime dispatch metadata 收敛，不新增 workload 专用路径。profile quick `1.871x`，full quick VM/Lua `1.855x`，AOT/Lua `1.981x`，AOT/VM `1.068x`；single-sample wall time 只作方向观察，验收重点是 frame setup 不再重复扫描 packed eligibility。
- 第七十九轮：`RuntimeDispatchSites` 新增预计算的 `packed_site_len` 与 `mixed_site_len`，`VmCaches::prepare_packed_sites()` 不再在每次 packed frame 入口重新 `unwrap_or` 和 `max` 计算 cache sizing。`packed_len()` 现在仅作为测试 getter 保留，运行时只消费结构化 dispatch metadata。本轮属于阶段 1/3 的 runtime dispatch/cache sizing metadata 收敛，不新增 workload 专用路径。profile quick `1.934x`，full quick VM/Lua `1.896x`，AOT/Lua `1.969x`，AOT/VM `1.038x`；single-sample wall time 只作方向观察，验收重点是 packed cache sizing 决策从执行器边界移入 runtime metadata。
- 第八十轮：`RuntimeDispatchSites` 新增 `packed_dispatch_code()`，把 packed fast-path 的 code32/decoded 选择封装成统一 dispatch view。`run_frame()` 不再手写 `packed_enabled()` 与 `Function::code32` 的组合判断，而是只消费 runtime dispatch metadata 提供的 packed code view；`packed_enabled()` 也降为测试 getter。本轮属于阶段 1/3 的 runtime dispatch decision 收敛，不新增 workload 专用路径。profile quick `1.824x`，full quick VM/Lua `1.838x`，AOT/Lua `1.963x`，AOT/VM `1.068x`；single-sample wall time 只作方向观察，验收重点是 packed/opcode dispatch 边界继续从 raw function 字段读取转向统一 metadata view。
- 第八十一轮：新增 `RuntimeDispatchMode::{Packed, Opcode}`，让 frame dispatch mode 成为结构化 runtime metadata view。`run_frame()` 现在 `match dispatch_sites.dispatch_mode(f)`，packed code/decoded 仍由 `RuntimeDispatchSites` 提供，opcode fallback 明确表示为 `Opcode` mode；后续 typed branch/op dispatch plan 可以挂到同一 mode 入口，而不是继续散落 `Option` 判断。本轮属于阶段 1/3 的 runtime dispatch protocol 收敛，不新增 workload 专用路径。profile quick `1.924x`，full quick VM/Lua `1.869x`，AOT/Lua `1.970x`，AOT/VM `1.055x`；single-sample wall time 只作方向观察，验收重点是 dispatch mode 语义结构化且 profile counters 无异常漂移。
- 第八十二轮：新增 `FrameDispatchPlan`，把 `RuntimeDispatchSites` 与 `RuntimeDispatchMode` 合并成 frame-level dispatch plan，并让 packed/opcode cache site 准备通过该 plan 调用。`run_frame()` 不再并行维护 `dispatch_sites` 与 mode 两个变量，而是从 `frame.runtime_dispatch_sites().frame_dispatch_plan(f)` 获取统一入口；后续 typed branch/op dispatch plan 可以继续挂在同一个 frame dispatch plan 上。本轮属于阶段 1/3 的 runtime dispatch/cache protocol 收敛，不新增 workload 专用路径。profile quick `1.824x`，full quick VM/Lua `1.891x`，AOT/Lua `1.979x`，AOT/VM `1.046x`；single-sample wall time 只作方向观察，验收重点是 dispatch mode、code view 与 cache sizing 由同一计划对象承载且 profile counters 无异常漂移。
- 第八十三轮：`FrameState` 新增 `runtime_dispatch_plan()`，把 `Function + RuntimeDispatchSites -> FrameDispatchPlan` 的拼装从 `run_frame()` 下沉到 frame state。`run_frame()` 现在只消费 frame 已提供的 dispatch plan，不再直接读取 `RuntimeDispatchSites` 或了解 plan 构造方式；这让后续 typed branch/op dispatch plan 可以继续从 frame setup 层进入。本轮属于阶段 1/3 的 call-frame/runtime dispatch protocol 收敛，不新增 workload 专用路径。profile quick `1.856x`，full quick VM/Lua `1.895x`，AOT/Lua `2.046x`，AOT/VM `1.080x`；single-sample wall time 只作方向观察，验收重点是 frame state 成为 runtime dispatch plan 的统一出口且 profile counters 无异常漂移。
- 第八十四轮：`FrameDispatchPlan` 继续收敛为 frame-level execution view，新增携带 `Function` 引用和 `function()` getter。`run_frame()` 现在只从 dispatch plan 读取执行函数，不再同时从 `FrameState` 单独读取 `func()` 和 dispatch plan；随后删除已无调用的 `FrameState::func()`，避免执行边界保留两个 function metadata 入口。本轮属于阶段 1/3 的 frame dispatch/execution metadata 收敛，不新增 workload 专用路径。profile quick `1.805x`，full quick VM/Lua `1.850x`，AOT/Lua `1.951x`，AOT/VM `1.054x`；single-sample wall time 只作方向观察，验收重点是 function view、dispatch mode 和 cache sizing 都由同一 frame dispatch plan 暴露。
- 第八十五轮：新增 `FrameExecutionParts`，把 `FrameState::execution_parts()` 返回的 register window、captures、capture specs、region plan 和 region allocator 从五元组提升为结构化 execution view。`run_frame()` 现在显式解构该结构体，执行边界不再依赖 tuple 字段顺序约定；后续 typed branch/op dispatch plan、region allocation 和 register ownership 信息可以继续挂到同一 frame execution view。本轮属于阶段 1/3/4 的 frame execution metadata 与 register-window 协议收敛，不新增 workload 专用路径。profile quick `1.845x`，full quick VM/Lua `1.873x`，AOT/Lua `1.996x`，AOT/VM `1.065x`；single-sample wall time 略回退按噪声处理，验收重点是 profile counters 无异常漂移且 frame execution ABI 结构化。
- 第八十六轮：`FrameExecutionParts` 继续承载 frame identity 和 register-window base。`FrameState::execution_parts()` 现在一次性提供 untyped frame token、base、register slice、captures、region plan 和 allocator；`run_frame()` 不再分别从 `FrameState` 读取 raw frame pointer 与 `reg_base()`，而是从 execution view 解构并在边界处 cast 回既有 raw pointer 类型。该轮属于阶段 1/3/4 的 frame execution metadata 收敛，目标是让 typed dispatch、register ownership 和 return handling 后续共用同一个 frame execution view，不新增 workload 专用路径。profile quick `1.842x`，full quick VM/Lua `1.881x`，AOT/Lua `1.967x`，AOT/VM `1.046x`；single-sample wall time 按噪声处理，验收重点是 full core tests 通过且 profile counters 与上一轮一致。
- 第八十七轮：`FrameExecutionParts` 继续吸收 `pc` 与 `FrameDispatchPlan`，`run_frame()` 入口不再分别调用 `frame.pc()` 和 `frame.runtime_dispatch_plan()`，而是只解构 execution view 后进入 packed/opcode dispatch；已无调用的 `FrameState::runtime_dispatch_plan()` 同步删除，避免 frame dispatch metadata 保留第二个出口。该轮属于阶段 1/3/4 的 frame execution metadata 收敛，目标是让 typed branch/op dispatch、register ownership 和 return handling 共享同一个 frame execution view，不新增 workload 专用路径。profile quick `1.919x`，full quick VM/Lua `1.886x`，AOT/Lua `1.957x`，AOT/VM `1.038x`；single-sample wall time 回退按噪声处理，profile counters 与上一轮一致。首次完整 `cargo test -p lk-core` 中 `pricing_helper_keeps_two_map_param_value_facts` 出现一次 `Float(289.0)`/`Int(289)` 瞬时失败，定向重跑和完整重跑均通过。
- 第八十八轮：新增 `FrameRuntimeView`，把 packed/opcode 两个执行器共同需要的 frame raw token、register window、function、base、captures、region metadata 和 `self_ptr` 收成一个共享 runtime view。`run_frame()` 现在只构造一次 runtime view，`run_packed_code()` 与 `run_opcode_code()` 不再各自接收同一组长参数；两个执行器入口只在函数开头解构 view，内层 opcode 行为不变。该轮属于阶段 1/3/4 的 frame execution ABI 收敛，目标是后续 typed branch/op dispatch、register ownership 和 return handling 只扩展一个共享 view，不新增 workload 专用路径。profile quick `1.827x`，full quick VM/Lua `1.808x`，AOT/Lua `1.926x`，AOT/VM `1.065x`；single-sample wall time 只作方向观察，profile counters 与上一轮一致。
- 第八十九轮：`FrameRuntimeView` 继续吸收 packed dispatch code view，新增 `packed_words` 和 `packed_decoded` 字段。`run_frame()` 在进入 packed mode 时把 `RuntimeDispatchMode::Packed` 的 words/decoded 写入 runtime view，`run_packed_code()` 不再单独接收 code32/decoded 参数，而是在入口从共享 view 读取；opcode fallback 会显式清空 packed code 字段。该轮属于阶段 1/3 的 runtime dispatch/execution view 收敛，目标是让 packed/opcode mode metadata、register window 和 region metadata 都从同一 view 进入执行器，不新增 workload 专用路径。profile quick `1.867x`，full quick VM/Lua `1.852x`，AOT/Lua `1.990x`，AOT/VM `1.075x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十轮：`FrameRuntimeView` 继续吸收 runtime cache view，新增 `caches: VmCaches` 字段。`run_frame()` 现在通过 `runtime.caches` 完成 packed/opcode cache 准备，`run_packed_code()` 与 `run_opcode_code()` 不再接收独立 `VmCaches` 参数，而是在执行器入口一次性解构 access/index/global/call/for-range/quickening/packed-hot cache。该轮属于阶段 1/3 的 runtime dispatch/cache/execution ABI 收敛，目标是让 typed branch/op dispatch、cache lifecycle 和 register window 都从同一 frame runtime view 进入执行器，不新增 workload 专用路径。profile quick `1.865x`，full quick VM/Lua `1.817x`，AOT/Lua `1.929x`，AOT/VM `1.061x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十一轮：`FrameRuntimeView` 继续吸收 `FrameDispatchPlan`，新增 `dispatch_plan` 字段，并把 packed/opcode cache prepare 封装成 `prepare_packed_dispatch()` 与 `prepare_opcode_dispatch()`。`run_frame()` 不再直接 match dispatch mode 或直接调用 cache prepare，而是只构造 runtime view 后询问 view 是否进入 packed 执行器，再统一 fallback 到 opcode 执行器。该轮属于阶段 1/3 的 frame dispatch/cache/execution ABI 收敛，目标是让后续 typed branch/op dispatch plan 作为 runtime view 能力扩展，而不是继续扩大 `run_frame()` 的局部协议分叉；不新增 workload 专用路径。profile quick `1.865x`，full quick VM/Lua `1.822x`，AOT/Lua `1.986x`，AOT/VM `1.090x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十二轮：`FrameRuntimeView` 继续吸收 frame pc，新增 `pc` 字段。`run_frame()` 不再维护独立局部 pc，也不再把 `&mut pc` 传给 packed/opcode 执行器；`run_packed_code()` 与 `run_opcode_code()` 直接从 runtime view 读取 pc，并在正常 fallback 到下一执行器或最终 return path 前写回 `runtime.pc`。该轮属于阶段 1/3/4 的 frame execution ABI 收敛，目标是让执行位置、dispatch plan、cache lifecycle、register window 和 return fallback 都由同一个 runtime view 承载，后续 typed branch/op dispatch 与 register ownership 不再扩展长参数列表；不新增 workload 专用路径。profile quick `1.884x`，full quick VM/Lua `1.845x`，AOT/Lua `1.959x`，AOT/VM `1.062x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十三轮：`FrameRuntimeView` 继续吸收 frame fallthrough return，新增 `finish_fallthrough_return()`。`run_frame()` 不再直接调用 `handle_return_common()` 并拆出 frame/raw regs/pc/base/self 指针，而是在 packed/opcode 都没有产生返回值时把最终 return fallback 交给 runtime view。该轮属于阶段 1/3/4 的 frame execution ABI 收敛，目标是让执行器调度、pc、cache lifecycle、register window 和 fallthrough return 都由同一个 runtime view 统一承载，后续 return handling 与 register ownership 可以继续在 view 上演进；不新增 workload 专用路径。profile quick `1.781x`，full quick VM/Lua `1.848x`，AOT/Lua `1.975x`，AOT/VM `1.069x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十四轮：删除 `FrameRuntimeView::func` 重复字段，packed/opcode 执行器改为从 `runtime.dispatch_plan.function()` 读取函数元数据。`FrameRuntimeView` 不再同时保存 dispatch plan 和 function 引用，避免执行边界保留两个 function metadata 入口；`opcode/closure_ops.rs` 显式导入 `Function`，不再依赖父模块 re-export 泄漏。该轮属于阶段 1/3 的 frame dispatch/execution metadata 收敛，目标是让 function view、dispatch mode 和 cache sizing 都只通过 `FrameDispatchPlan` 暴露，后续 typed branch/op dispatch 只扩展一个计划对象；不新增 workload 专用路径。profile quick `1.811x`，full quick VM/Lua `1.817x`，AOT/Lua `1.956x`，AOT/VM `1.076x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十五轮：删除 `FrameRuntimeView` 中重复的 `packed_words`/`packed_decoded` 字段，packed 执行器直接从 `runtime.dispatch_plan.mode()` 取得 `RuntimeDispatchMode::Packed` 的 words/decoded。`prepare_packed_dispatch()` 现在只负责通过 dispatch plan 准备 packed cache sites，不再复制 packed code view 到 runtime view。该轮属于阶段 1/3 的 frame dispatch/execution metadata 收敛，目标是让 packed mode、packed code view 和 cache sizing 都由 `FrameDispatchPlan` 单一入口承载，后续 typed branch/op dispatch 不再扩展 runtime view 的重复 metadata 字段；不新增 workload 专用路径。profile quick `1.819x`，full quick VM/Lua `1.805x`，AOT/Lua `1.926x`，AOT/VM `1.067x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十六轮：`FrameRuntimeView` 继续吸收 `VmContext`，新增 `ctx` 字段。`run_frame()` 不再把 `ctx` 单独传给 `run_packed_code()` / `run_opcode_code()`；两个执行器统一从 runtime view 解构 context。该轮属于阶段 1/3/4 的 frame execution ABI 收敛，目标是让执行器共享的 context、pc、dispatch plan、cache lifecycle、register window 和 return fallback 都由同一个 runtime view 承载；不新增 workload 专用路径。profile quick `1.774x`，full quick VM/Lua `1.731x`，AOT/Lua `1.964x`，AOT/VM `1.134x`；single-sample wall time 只作方向观察，profile counters 无异常漂移。
- 第九十七轮：`FrameRuntimeView` 继续吸收 runtime metrics gate，新增 `collect_metrics` 字段并在 `run_frame()` 构造时读取一次 `vm_runtime_metrics_enabled()`。`run_packed_code()` 与 `run_opcode_code()` 不再各自读取全局 metrics 开关，而是消费同一个 frame runtime view 上的 gate。该轮属于阶段 1/3/4 的 frame execution/observability ABI 收敛，目标是让 dispatch、cache lifecycle、pc、context、register window 和 metrics gating 都由同一个 runtime view 承载；不新增 workload 专用路径。profile quick `1.756x`，full quick VM/Lua `1.884x`，AOT/Lua `2.063x`，AOT/VM `1.095x`；single-sample wall time 回退按噪声记录，profile counters 无异常漂移。
- 第九十八轮：register copy 层新增 known metrics gate 入口，`copy_register_value_with_metrics()`、local load/store copy 和 const/register copy 都可直接消费 frame runtime view 上的 `collect_metrics`。opcode 主循环、packed 主循环和 packed hot-slot 中的 register/const/local copy 路径不再在每次 heap-backed copy 时重新读取全局 metrics 开关；旧 API 保留给测试和非执行器辅助路径。该轮属于阶段 1/4 的 register ownership/observability 成本收敛，不新增 workload 专用路径。profile quick `1.758x`，full quick VM/Lua `1.836x`，AOT/Lua `1.995x`，AOT/VM `1.087x`；profile copy counters 与上一轮同形，说明 metrics gate 传递没有改变 copy 语义。
- 第九十九轮：generic/const/call/container value copy 层继续补齐 known metrics gate API，packed 主循环中的 list slice、list push 和 self-list push copy 直接消费 frame runtime view 上的 `collect_metrics`。旧 API 保留给尚未迁移的 opcode/container/call 辅助路径，避免一次性扩大语义风险。该轮属于阶段 1/4 的 value movement/observability 成本收敛，不新增 workload 专用路径。profile quick `1.648x`，full quick VM/Lua `1.679x`，AOT/Lua `1.957x`，AOT/VM `1.166x`；profile copy counters 与上一轮同形，说明 metrics gate 传递没有改变 copy 语义。
- 第一百轮：packed hot-slot 中所有直接 generic value copy 都改为消费 known metrics gate，包括 global fallback/define、map/object access cache hit、map get、build list/map、list push 和 map set 等路径；`copy_value_for_register()` 不再出现在 `packed/hot_exec.rs`。该轮属于阶段 1/4 的 hot-slot value movement/observability 成本收敛，不新增 workload 专用路径。profile quick `1.766x`，full quick VM/Lua `1.792x`，AOT/Lua `2.012x`，AOT/VM `1.122x`；single-sample wall time 本轮回退，按噪声记录，profile copy counters 与上一轮同形，说明 metrics gate 传递没有改变 copy 语义。
- 第一百零一轮：packed/opcode named-call 和 shared positional slow-call 的 call-arg copy 改为消费 frame runtime view 传入的 `collect_metrics`，`load_named_pairs()`、`run_call_named_*()`、`run_call_*()` 和 `run_positional_call_common()` 不再在 call-arg heap copy 时重新读取全局 metrics gate。该轮属于阶段 1/3/4 的 call frame/value movement observability 成本收敛，不新增 workload 专用路径。profile quick `1.791x`，full quick VM/Lua `1.719x`，AOT/Lua `1.925x`，AOT/VM `1.120x`；profile copy counters 与上一轮同形，说明 call-arg gate 传递没有改变 copy 语义。

最新验证（第一百零一轮）已通过：

- `cargo fmt --all -- --check`
- `cargo check -p lk-core`
- `cargo test -p lk-core runtime_sites -- --nocapture`
- `cargo test -p lk-core frame -- --nocapture`
- `cargo test -p lk-core`
- `cargo build --release -p lk-cli`
- `git diff --check`
- 单文件行数检查：当前仓库自身 `.rs` 文件不超过 1500 行；`references/` 外部参考树存在超过 1500 行文件。
- `PROFILE_WORKLOADS=1 RUN_AOT=0 RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh`
- `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh`
