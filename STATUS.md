# VM 性能状态

## 目标与约束

- 当前核心目标：真实 workload geomean `< 0.5x` vs Lua；`lk file.lk` 直接执行默认使用 bytecode VM。native/AOT 允许作为显式架构级性能路径：`LK_NATIVE_RUN=1 lk file.lk` 可选启用 cached native executable，`lk compile exe file.lk` 显式生成 native executable。
- 当前优化阶段：避免 benchmark-shaped 专门 opcode，优先做通用 VM/compiler/LLVM 优化。7-bit opcode + metadata format 基础迁移已完成，当前 opcode 编号已按语义分区重排序；后续新增 opcode 应只做 Lua-style operand-shape specialization。`ForLoopI` 是此前为了验证数值 range loop 热路径而临时复用 `Extra = 62` 的历史例外，当前已随重排序移出该历史槽位，不应继续作为复用保留槽的先例。
- 安全约束：除 LLVM 部分外不能使用 `unsafe`，因此非 LLVM VM 解释器不走 unchecked raw pointer、`assume` 或 computed-goto 方案。
- 维护约束：单个 Rust 源文件长期不应超过 1500 行；当前为了先完成 opcode 优化验证，`core/src/vm/compiler/tests.rs` 和 `core/src/vm/compiler.rs` 暂时超过该限制，后续完成性能阶段后需要拆分。

## 当前结论

最新结论：`BrEqZeroInt` / `BrNeZeroInt`、`BrEqIntI4` / `BrNeIntI4` 和 `BrModEqZeroIntI4` / `BrModNeZeroIntI4` 已作为通用 branch operand-shape 落地，只在 facts-confirmed Int condition false edge 发射，不是 workload-specific fused opcode。zero branch 覆盖 `x == 0` / `x != 0`，i4 branch 覆盖 `x == K` / `x != K` 且 `K` 在 `0..15` 的 small-int equality/inequality，mod-zero i4 branch 覆盖 `(x % K) == 0` / `(x % K) != 0` 且 `K` 在 `1..15` 的 divisibility guard。`AddMulInt A B C` 和 `Add2Int A B C` 已作为通用 compound accumulator shape 落地；`AddListInt A B C` / `SubListInt A B C` 已作为 facts-confirmed `List<Int>[IntKey]` accumulator shape 落地；`MidInt A B C` 已作为 facts-confirmed integer midpoint shape 落地。`GetIndexStrI` / `SetIndexStrI` 现在覆盖 `Map` 目标上的 string-prefix + int-suffix template key，例如 `"n${i}"` 和 `"id${event_id}"`，避免为直接 `.set(...)` key 先 materialize `ConcatString`。2-part/3+ part template string assignment 也支持 direct-to-destination lowering，避免 `ConcatString/ConcatN temp; Move dst temp`。ordering compare-test 的 `TestLtInt` / `TestLeInt` / `TestGtInt` / `TestGeInt` 现在有专门 VM dispatch arm，避免 facts-confirmed Int/Int 条件每次走 generic compare-test 二级 opcode match。最新默认 VM 正式样本为 `0.783x`；此前 template assignment direct-to-destination 后为 `0.779x`，ordering compare-test hot arm cleanup 后为 `0.785x`，`MidInt` 后为 `0.792x`，`AddListInt` / `SubListInt` 后为 `0.797x`，`Add2Int` 后为 `0.796x`，`ConcatN` register-window 直写两轮为 `0.798x` / `0.793x`，checksum 全部一致，runner 输出 `AOT: disabled`。static coverage 现在显示全 workload `instructions=1902`、`Jmp=166`、`LoadInt=313`、`AddInt=99`、`SubInt=43`、`AddIntI=58`、`AddMulInt:10`、`Add2Int:2`、`AddListInt:1`、`SubListInt:1`、`MidInt:2`、`SetIndexStrI:3`、`BrEqZeroInt:1`、`BrModNeZeroIntI4:21`、`BrNeZeroInt:9`、`BrNeIntI4:10`、`GetList:4`、`GetIndex:62`、`Move=300`。profile 显示 `sliding_window_sum` opcode steps 从约 `7.06M` 降到约 `6.15M`，`two_sum_map` opcode steps 从约 `2.04M` 降到约 `1.84M`，`event_join_by_id` 从约 `3.78M` 降到约 `3.55M`；`two_sum_map` 的 string write source 从约 `400K` 降到约 `200K`。该收益是局部结构性收益，不是 `<0.5x` 主路径。当前默认 VM 仍未达到 `<0.5x`；下一步应优先做更系统的 branch-chain / hot-loop lowering、loop-carried register write 消除或解释器结构拆分。

LK VM 已从“泛化解释器 + 大量运行时 materialization”推进到“compiler facts 驱动的轻量 lowering + typed fast path”。解释器 VM 在当前 20 项 workload suite 上尚未达到 `<0.5x`；显式 `lk compile exe` native/AOT 曾在 supported shapes 上达到该阈值，但当前 full-suite AOT 仍有 dynamic map `GetIndex` lowering 缺口，不能当作默认执行达标状态。当前 opcode 方向已经完成基础 encoding 迁移：`Opcode` 空间从 64 slot 扩到 128 slot，`InstrFormat` 改由 `OpcodeInfo` metadata 决定，`ABC.C` 恢复 8 bit；本轮进一步按语义分区重排 opcode 编号，并把 module artifact version 从 `1` bump 到 `2`，这是 bytecode encoding 维护重构，不单独宣称 wall-clock 性能收益。`AddIntI`、`MulIntI` 和 `ModIntI` 已作为通用 integer immediate operand-shape opcode 落地；`AddMulInt` / `Add2Int` 已作为通用 compound integer accumulator opcode 落地；`AddListInt` / `SubListInt` 已作为 facts-confirmed typed int-list accumulator opcode 落地；`MidInt` 已作为 facts-confirmed integer midpoint opcode 落地；`MinInt` / `MaxInt` 已作为通用 min/max update opcode 落地；`GetIndexStrI` / `SetIndexStrI` 已作为 string-prefix + int-suffix map key opcode 落地；`ConcatN`、2-part template direct-to-destination、`Return0` / `Return1`、`Move2`、`TestEqIntI2`、`BrEqZeroInt` / `BrNeZeroInt`、`BrEqIntI4` / `BrNeIntI4`、`BrModEqZeroIntI4` / `BrModNeZeroIntI4` 已作为通用 operand-shape 或 control-flow shape 落地。最新正式样本 geomean 为 `0.783x`。继续实现并验证过 register-vs-register `Br*Int8` 短分支候选：dynamic profile 覆盖 `binary_search`、`sliding_window_sum`、`prime_trial_division` 等通用 compare branch，默认 VM 正式样本小幅到 `0.887x`，但收益不足且会暴露当前 native dynamic map `GetIndex` lowering 缺口；因此已回退。它说明 branch-chain、loop literal hoisting 和解释器热路径方向有效，但还不是 `<0.5x` 主路径。此前验证过的负结论仍成立：`DivIntI`、`LoadIntI`、typed int sidecar/register cache、`ConcatKInt`、dynamic compare-test 放宽、absolute `Jmp` target cache、`ForLoopI` 四向拆分、single-term `AddMulInt`/`AddModIntI` 扩大发射等都已因退化、timeout 或收益不足回退。下一步如果继续做控制流，应优先做更系统的 branch-chain / hot-loop lowering 或解释器结构拆分，而不是简单替换 `Test + Jmp` trampoline。

`run_function_inner` 仍是主要结构风险：主循环是 `match instr.opcode()`，其中混有热路径、fallback、错误构造、metrics、container/call 逻辑。问题不是 `match` 本身，而是热 arm 里仍有多层分支和 `Result` slow path 污染。拆分方向应是保留主循环最短热路径，把 dynamic fallback 和错误构造放到 cold helper。`GetFieldK` / `SetFieldK` 已把一部分 const string key 的 map/object 读写从泛化 `GetIndex` / `SetIndex` 中拆出来，`ConcatN` 已减少多段 template 的 binary concat/materialization，但默认 VM geomean 仍未达标。

历史 AOT/native 路径曾在 20 项 workload suite 的 `RUNS=3 EXTRA_RUNS=5` 复验中达到 `<0.5x` vs Lua；但当前 full-suite `RUN_AOT=1 RUNS=1 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=120 bash bench/run_workload_bench.sh` 会跳过 AOT compile，原因是 native lowering 仍不支持 loop 后 dynamic map `GetIndex` shape。直接执行 `lk file.lk` 默认仍是 bytecode VM，不默认 AOT/native。cached native executable 仅在显式设置 `LK_NATIVE_RUN=1` 时作为 opt-in 路径尝试，native lowering 或构建失败时回退 VM。纯解释器 VM 本身仍未达到 `<0.5x`，后续如果目标严格以默认执行路径为准，仍需继续优化解释器；native/AOT 的主要剩余工作是修复 dynamic map `GetIndex` lowering，并降低 dynamic string-map runtime helper、typed map mutation 和 template path 等慢项。

## 性能证据

### 正式基线

`bench/README.md` 记录的最新正式基线是 2026-06-04：

```bash
RUN_AOT=0 RUNS=6 EXTRA_RUNS=6 bash bench/run_workload_bench.sh
```

- Geometric mean LK/Lua：`2.235x`
- 主要差距：per-opcode overhead、container/index/string 路径、typed branch 路径、dispatch loop 复杂度。

### 历史 AOT 低样本结果

```bash
RUN_AOT=1 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

- suite：20 项 workload，包含 customer/event/config/template/state-machine 场景。
- VM/Lua geometric mean：`1.063x`
- AOT/Lua geometric mean：`0.351x`
- AOT/VM geometric mean：`0.331x`
- checksum 全部一致。
- AOT 明显慢项：`event_join_by_id` `2.207x`、`two_sum_map` `2.030x`、`histogram_group_count` `1.857x`、`inventory_reorder` `1.546x`、`template_render_mix` `1.065x` vs Lua；这些主要受 dynamic string key/map runtime split/helper 调用和 template string construction 影响，应作为下一轮 AOT hot path 优化对象。
- `state_machine_transitions` 曾用静态 map + dynamic template transition key，导致 native facts 无法分类；当前已改为显式 transition branches，保留状态机场景但避免 mixed optional map lookup 阻断整包 AOT。

### 历史 opt-in native 低样本结果

`LK_NATIVE_RUN=1 lk file.lk` 在 LLVM feature 可用、未设置 `LK_FORCE_VM=1` / `LK_VM_ONLY=1` / `LK_VM_PROFILE=1` 时会尝试 cached native executable。cache key 包含源文件内容、当前 `lk` executable 路径/mtime 和 CLI package version；同一 source 首次运行可能触发 compile，后续 filtered workload 复用同一个 native executable。native lowering 或 clang 构建失败时回退现有 VM 解释器。未设置 `LK_NATIVE_RUN=1` 时，直接执行默认走 VM；workload runner 的 LK 样本也会默认传入 `LK_FORCE_VM=1`，避免外部 shell 环境误把默认 VM benchmark 切到 cached native。

```bash
RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

- opt-in cached native LK/Lua geometric mean：历史样本 `0.353x`（冷 cache dir + prewarm 后）
- full runner 历史复验：opt-in cached native LK/Lua `0.349x`，AOT/Lua `0.350x`，AOT/LK `1.001x`
- checksum 全部一致。
- 该结果是显式 native 性能路径，不是默认 `lk file.lk` 的 VM 结果。

### 当前低样本结果

低样本 timing 只用于找方向，不能替代正式基线：

```bash
RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

- Geometric mean LK/Lua：当前低样本多次复测约 `1.139x` 到 `1.220x`，checksum 全部一致。低样本波动明显，不能把单次 `1.139x` 当成稳定基线。
- 本轮结果包含当前 20 项 workload suite、7-bit opcode encoding 基础迁移，以及 `AddIntI` / `MulIntI` / `ModIntI` 通用 operand-shape opcode。它们已接入 VM、compiler、LLVM lowering 和 profile counters。
- profile 证实 `AddIntI` 动态出现：`gcd_batch` `160000`、`order_score_pipeline` `270000`、`config_defaults_merge` `180000`、`fraud_rule_scoring` `151717`。但该 opcode 只替代部分 small-int add/sub，release wall-clock 低样本未改善。
- `MulIntI` / `ModIntI` 覆盖了更热的 literal RHS numeric shape：profile 显示 `MulIntI` 出现在 `binary_search:1440409`、`stock_max_profit:1080000`、`gcd_batch:160000`，`ModIntI` 出现在 `log_parse_filter:782684`、`inventory_reorder:478001`、`config_defaults_merge:435000`、`route_permission_check:360002`。普通 release 低样本 geomean `1.139x`，checksum 全部一致；但 `gcd_batch` / `stock_max_profit` 噪声较高，需要正式多样本复验后再当作基线。
- `BrTrue` / `BrFalse` opcode 已接入 IR、VM dispatch、control-flow facts、LLVM scalar/straightline/subfunction lowering 和 fused bool branch fact，但 compiler 默认不发该 opcode；启用单条 branch 形状的低样本 VM/Lua geomean 约 `1.219x`，未优于当前默认路径。
- condition-context short-circuit lowering 已用于普通 `if` / `while` / conditional expression，并扩展到 direct-inline `if` / `while`；`&&` / `||` 条件不再强制 materialize 中间 bool。
- `BrNil` / `BrNotNil` 已作为通用 nilness branch opcode 落地，覆盖 `x == nil` / `x != nil` 条件分支。profile 显示 `config_defaults_merge` 出现 `BrNotNil:360000`，release 低样本 geomean 从前一轮约 `1.197x` 降到 `1.131x`；`config_defaults_merge` 从约 `1.802x` 降到 `1.679x`。
- Lua-style compare-test opcode 已接入 VM/compiler/LLVM/control-flow facts，并默认只对 facts-confirmed `Int/Int` condition-context 比较启用。全量 unknown/dynamic compare-test lowering 已验证会退化，低样本 geomean 约 `1.234x`；typed-only lowering 后低样本 geomean 约 `1.217x`，checksum 全部一致。profile 显示主要覆盖 `gcd_batch TestNeInt:737372`、`state_machine_transitions TestEqInt:1114278`、`config_defaults_merge TestEqInt:540000`。当前 VM dispatch 已给 equality 和 ordering typed compare-test 都提供专门 hot arm，其中 `TestLtInt` / `TestLeInt` / `TestGtInt` / `TestGeInt` 不再落到 generic compare-test arm；最新默认 VM 正式样本为 `0.783x`。该结果说明 compare-test 应作为 typed operand-shape opcode 保留，不应对动态比较默认启用。
- `GetFieldK` / `SetFieldK` 已作为 Map/Object + 短字符串 literal key 的通用 operand-shape opcode落地。profile coverage 显示 `GetFieldK:405733`、`SetFieldK:144990`，同时 `GetIndex` 降到 `2674709`、`SetIndex` 降到 `1043867`。该优化覆盖 `map.get`、字段访问、rewritten set-index 和 `.set(...)` statement/expression，不改变 mutable map/object 的 runtime lookup/write 语义；wall-clock 低样本仍有明显波动。
- `GetIndexStrI` / `SetIndexStrI` 已作为 Map + string-prefix/int-suffix template key 的通用 operand-shape opcode 落地。compiler 只在 direct template key 形状和 suffix facts 确认 `Int` 时发射；`SetIndexStrI` 当前 static coverage 为 `3`。profile-enabled 单样本显示 `two_sum_map` opcode steps 从约 `2.04M` 降到约 `1.84M`，`event_join_by_id` 从约 `3.78M` 降到约 `3.55M`，正式默认 VM geomean 为 `0.783x`，checksum 全部一致。
- `GetList` 已实现 VM/compiler/LLVM 支持并以窄条件默认开启：只在 compiler facts 确认目标是 `List`、元素 kind 是 `Int`、key register facts 是 `Int` 时发射。`AddListInt` / `SubListInt` 进一步覆盖 compound assignment 里的 facts-confirmed `List<Int>[IntKey]` accumulator shape；static coverage 为 `AddListInt:1`、`SubListInt:1`、`GetList:4`、`GetIndex:62`。profile 显示 `sliding_window_sum AddListInt:480000`，opcode steps 从约 `7.06M` 降到约 `6.15M`；默认 VM 正式样本为 `0.797x`，checksum 全部一致。此前泛化 `GetList` 曾覆盖约 `1.22M` dynamic ops 但低样本退化，因此当前只保留 facts-confirmed typed int-list 形状。
- `MidInt` 已实现 VM/compiler/LLVM 支持并以窄条件默认开启：只在 `math.floor((lhs + rhs) / 2)` 且 `lhs` / `rhs` 都可由 facts 证明为 Int-like 时发射。static coverage 为 `MidInt:2`，把全 workload `instructions` 从 `1920` 降到 `1918`，`AddInt` 从 `101` 降到 `99`，`DivInt` 从 `18` 降到 `16`；该轮默认 VM 正式样本为 `0.792x`，checksum 全部一致。后续 ordering compare-test hot arm cleanup 推进到 `0.785x`，template assignment direct-to-destination 后最新默认 VM 为 `0.783x`。该形状覆盖二分查找等通用 midpoint expression，不针对 workload 名称。
- known string key 读取空 `TypedMap::Mixed` 时现在直接返回 `Nil`，避免 sparse map/default 场景为 miss 构造 `RuntimeMapKey` 并走 generic lookup。profile 显示 `config_defaults_merge` 中 `typed_map_direct` 为 `396428`、`generic_map_lookup` 为 `69429`；同环境 A/B 里关闭该 fast path 会把 `config_defaults_merge` 从 `1.204x` 退到 `1.383x`。该优化不新增 opcode，也不改变非空 `Mixed` map 的 lookup 语义。
- `AddIntI` / `MulIntI` 已扩展到 commuted small-int literal shape：`literal + x` / `literal * x` 在 RHS facts 确认 `Int` 且 static flavor 为 `Int` 时发射 immediate opcode。这是通用 operand-shape lowering，不针对某个 workload。
- 当前正式样本明显慢项仍包括 `sliding_window_sum`、`state_machine_transitions`、`prime_trial_division`、`gcd_batch` 和 `config_defaults_merge`；其中 `prime_trial_division`、`state_machine_transitions`、`stock_max_profit` 和 `binary_search` 噪声偏高。empty mixed-map fast path 改善了 `config_defaults_merge`，但默认 VM geomean 仍未接近 `<0.5x`。
- 下一步 VM opcode 优化不应针对单个 workload；控制流方向应只继续 typed compare-test / compare-branch，不要对 unknown/dynamic 比较启用；容器方向的新分桶显示 hot dynamic key 主要是 short string，`GetI/SetI` 暂不作为下一步默认优化。返回路径的 `Return0/Return1` 已落地但收益很小。仅把 `Test + Jmp` 改成 `BrTrue/BrFalse`、把所有 `Jmp` 改成 absolute target cache，或增加 `DivIntI` 暂不保留为默认优化。
- runner 已改成逐 workload 运行并打印进度，默认 `BENCH_TIMEOUT=30`，可用 `BENCH_PROGRESS=0` 静默进度；Lua 侧也支持 `LK_WORKLOAD_FILTER`，避免静默全量 suite 被误判为死循环。
- 最新默认 VM validation 使用 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh`。opcode renumbering 后最新 VM/Lua geomean 为 `0.783x`；template assignment direct-to-destination 后为 `0.779x`；ordering compare-test hot arm cleanup 后为 `0.785x`；新增 `MidInt` 后为 `0.792x`；保留 `AddListInt` / `SubListInt` 后为 `0.797x`；此前 `Add2Int` 后为 `0.796x`，保留 `ConcatN` register-window 直写后两轮结果为 `0.798x` / `0.793x`，新增 profile 分桶后的普通 release 复测为 `0.800x`，checksum 全部一致，runner 输出 `AOT: disabled`。`BrEqZeroInt` 把 `gcd_batch` 的 zero false edge 收成单条 branch；`BrNeZeroInt` 覆盖非 modulo 的 zero false edge；`BrModNeZeroIntI4` 覆盖 facts-confirmed `(x % K) == 0` false edge；`AddMulInt` 覆盖 10 处 facts-confirmed compound-add multiply term，`Add2Int` 覆盖 2 处 facts-confirmed compound-add plain pair term；`AddListInt` / `SubListInt` 覆盖 2 处 facts-confirmed typed int-list accumulator term；`MidInt` 覆盖 2 处 facts-confirmed integer midpoint expression；`SetIndexStrI` 覆盖 3 处 facts-confirmed string-prefix/int-suffix map key write。static coverage 显示全 workload `instructions=1902`、`Jmp=166`、`LoadInt=313`、`AddInt=99`、`SubInt=43`、`AddIntI=58`、`GetList:4`、`GetIndex:62`、`AddMulInt:10`、`Add2Int:2`、`AddListInt:1`、`SubListInt:1`、`MidInt:2`、`SetIndexStrI:3`、`TestEqIntI=6`、`TestEqIntI2=6`、`BrEqZeroInt:1`、`BrModNeZeroIntI4:21`、`BrNeZeroInt:9`、`BrNeIntI4:10`、`Move=300`。这些结果说明通用 arithmetic/control-flow/index/string-window fast path、loop literal hoisting 和 compiler-level branch-chain 重排仍能降低调度成本，但默认 VM 还没有达到 `<0.5x`；后续需要更系统的 branch-chain/hot-loop lowering。
- 本轮继续验证并回退了一个通用候选：给 direct branch / inline zero branch 增加“短 forward taken target 是 `Move`/`Move+Jmp`”的 dispatch peephole 后，低样本 geomean 退到 `0.832x`，`state_machine_transitions` 退到 `1.426x`，已回退。此前泛化 `GetList` 也因 wall-clock 退化保持关闭；本轮保留的是更窄的 facts-confirmed `List<Int> + Int key` lowering。结论是当前不能靠局部 branch target peephole 达到 `<0.5x`；下一步应做 compiler-level branch-chain/register-write 重排或更系统的 hot-loop lowering。
- 继续验证并回退了 arithmetic/list tail 后接 `ForLoopI` 的 dispatch peephole：`AddInt` / `AddIntI` / `AddMulInt` / `ListPush` 成功写入后，如果下一条正好是 `ForLoopI`，尝试在当前 handler 内直接执行 loop update/bounds check。该候选覆盖通用 loop-tail 形状且 checksum clean，但默认 VM 正式样本退到 `0.856x`，`state_machine_transitions` 退到 `1.329x`，已回退。结论是当前不应继续在解释器里把相邻 loop tail 拼进前一条 opcode；loop 方向需要 compiler-level phi/register-write 重排或更完整的 hot-loop lowering。
- 继续验证并回退了 `Move2` / `AddIntI` 后接 `Jmp` 的 next-jump dispatch peephole。profile 方向上它确实把 `gcd_batch` opcode steps 从约 `3.43M` 降到约 `2.77M`，把 `binary_search` 从约 `11.88M` 降到约 `10.68M`，但普通 release 默认正式样本退到 `0.823x`，checksum 全部一致，已回退。结论是减少 `Jmp` dispatch 不能只靠局部 tail jump 拼接。
- 继续验证并回退了 `ForLoopI` static `step_value` fact：VM 热路径少读 step register、少匹配一个 `RuntimeVal::Int`，但默认正式样本退到 `0.821x`，checksum 全部一致，已回退。结论是当前 `ForLoopI` 方向不应做单点 operand-read 微调，应做更高层的 loop-carried register rewrite 或 hot-loop lowering。
- 继续验证并回退了 `GetIndex` map fact micro fast-path：当 compiler fact 已确认 target 是 `Map` 时跳过“key 是短 List slice 描述”的罕见 probe。该改动保持 checksum clean，但默认正式样本曾从 `0.819x` 退到 `0.828x`，因此不保留。本轮还验证了更直接的 facts-confirmed Map dispatch helper，默认正式样本为 `0.803x`，仍差于保留区间，已回退。结论是当前 map/index 方向不能只靠调整 `GetIndex` arm 顺序推进，应优先做更高层的 map loop/value elision、动态 key lowering 或更系统的 hot-loop lowering。
- 继续验证并回退了 builtin method expression 的 direct-to-destination lowering：`.len()`、`.starts_with()`、`.split()`、`.join()`、`.set()` 直接写目标 local 后，static coverage 从 `instructions=1981` / `Move=341` 降到 `instructions=1978` / `Move=338`，但默认正式样本为 `0.825x`，低于回退后复验的 `0.814x`，checksum 全部一致。结论是这类低覆盖 method-result materialization 消除不是当前主路径，不应保留。
- `BrEqInt8` / `BrNeInt8` / `BrLtInt8` / `BrLeInt8` / `BrGtInt8` / `BrGeInt8` 作为 register-vs-register short branch 候选验证后回退。打开后 static coverage 为 `instructions=1974`、`Jmp=141`，dynamic profile 显示 `binary_search BrGtInt8:1320409` / `BrNeInt8:1320409`、`sliding_window_sum BrLtInt8:960000`；默认 VM 正式样本 geomean `0.887x`，checksum 全部一致。该收益太小，而且 `RUN_AOT=1 RUNS=1 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=120 bash bench/run_workload_bench.sh` 暴露 native lowering 仍会在 loop 后 dynamic map `GetIndex` shape 上跳过 AOT，因此该 candidate 不作为默认优化保留。

### 当前 coverage smoke

最近一次验证命令：

```bash
cargo run -p lk-cli -- coverage --runtime bench/workloads_business_algorithms.lk
```

结果：

- checksum 全部保持一致
- `instructions`: `1902`
- `LoadInt`: `313`
- `AddInt`: `99`
- `DivInt`: `16`
- `AddIntI`: `58`
- `MidInt`: `2`
- `BrModNeZeroIntI4`: `21`
- `BrNeZeroInt`: `9`
- `BrNeIntI4`: `10`
- `LoadNil`: `14`
- `LoadString`: `153`
- `GetFieldK`: `9`
- `GetIndex`: `62`
- `GetList`: `4`
- `AddListInt`: `1`
- `SubListInt`: `1`
- `SetFieldK`: `8`
- `SetIndexStrI`: `3`
- `SetIndex`: `9`
- `Return`: `14`
- `Move`: `300`
- 未启用 `vm-profile` 的普通构建会打印 `Runtime metrics: disabled; rebuild with --features vm-profile to collect counters`，不再展示容易误读的全 0 runtime counters。
- 启用 `vm-profile` 时，runtime coverage 会打印 `dynamic_opcodes` 完整直方图、`register_write_sources` 和 `index_key_metrics`；CLI 的 `VM profile:` 单行会输出 `top_opcodes=...` / `write_sources=...` / `index_keys=...`，benchmark runner 在 profile 表后展示每个 workload 的 Top-6 动态 opcode、register write source 与 index key metric。

普通构建下 runtime metrics 为 compile-time no-op；如需真实 counters，必须使用：

```bash
cargo build --release -p lk-cli --features vm-profile
RUN_AOT=0 PROFILE_WORKLOADS=1 RUNS=1 EXTRA_RUNS=0 bash bench/run_workload_bench.sh
```

本轮 profile-enabled coverage 显示，主要动态调度热点已经非常明确：

- 全 suite coverage: `AddInt` 约 `8.36M`，`Move` 约 `8.62M`，`ForLoopI` 约 `3.93M`，`Jmp` 约 `3.50M`；默认启用路径中 `MulIntI` 覆盖约 `2.9M+`，`ModIntI` 覆盖约 `3.6M+`，`GetFieldK` 约 `406K`，`SetFieldK` 约 `145K`。泛化 `GetList` 候选曾覆盖约 `1.22M` 但因 wall-clock 退化关闭；当前默认只保留静态 `GetList:6` 的 `List<Int> + Int key` 窄 lowering。
- `gcd_batch`: inline `Move2` 覆盖后，热赋值链已从两条 `Move` 收成单条 `Move2`；剩余主导项是 `TestNeIntI`、`Jmp`、`ModInt`、`Move2`、`AddIntI`、`MulIntI`，说明 tight numeric while loop 已不再主要被 literal materialization 压住，而是被 loop/control-flow、dynamic modulo 和 loop-carried register writes 压住。
- `binary_search`: `Jmp`、`Move`、`AddInt`、`MulIntI`、`AddIntI`、`DivInt` 主导，适合优先做数值 loop、compare branch lowering 和 `Move` 消除。
- `stock_max_profit`: `MulIntI`、`AddInt`、`ForLoopI`、`SubInt` 后面已经出现 `MaxInt:540000` 与 `MinInt:540000`，说明 min/max update opcode 覆盖了真实通用 scan 形状，但剩余成本仍来自 loop/arithmetic 调度。
- `route_permission_check` 已通过只读 const map + literal string key + int value fold 消掉角色表 lookup；`fraud_rule_scoring` 仍由 `ModIntI`、`Move`、`TestEqInt`、`AddInt`、`MulIntI` 以及字符串/index 路径主导。

最新 profile-enabled 低样本 write-source counters：

- aggregate coverage: `arithmetic` 约 `21.85M`，`move` 约 `6.06M`，`const_load` 约 `3.37M`，`string` 约 `2.55M`，`index` 约 `2.21M`。
- `gcd_batch`: `move` 约 `1.55M`，`arithmetic` 约 `1.14M`，`const_load` 已降到约 `50`。
- `binary_search`: `arithmetic` 约 `5.76M`，`move` 约 `1.44M`，`const_load` 已降到约 `50`。
- `sliding_window_sum`: `arithmetic` 约 `4.66M`，`index` 约 `912K`，`const_load` 已降到约 `4K`。
- `route_permission_check`: 只读角色表 `map.get(role_levels, "...")` 已折叠为 `LoadInt`，release 低样本 ratio 从约 `1.58x` 降到约 `1.18x`。
- `fraud_rule_scoring`: `arithmetic` 约 `1.24M`，`move` 约 `170K`，`string` / `index` 各约 `85K`，`const_load` 已降到约 `97K`。

最新 profile-enabled 低样本 index-key counters：

- aggregate coverage: `generic_map_lookup` 约 `84K`，`typed_map_direct` 约 `2.97M`；`config_defaults_merge` 的稀疏 default map miss 现在大量转成 direct nil，但仍有约 `69K` generic lookup。部分只读 string-int const map lookup 已在 compiler 阶段折叠，不再进入 `GetIndex`。新分桶显示 hot dynamic map key 几乎都是 `dynamic_short_string_key`，不是 integer key，因此当前不优先做 `GetI` / `SetI`。
- `two_sum_map`: `dynamic_register_key` / `dynamic_short_string_key` 约 `402K`，`typed_map_direct` 约 `398K`，`direct_string_key` 约 `200K`，`generic_map_lookup` 约 `2.5K`。
- `histogram_group_count`: `dynamic_register_key` / `dynamic_short_string_key` 约 `746K`，`typed_map_direct` 约 `739K`，`direct_string_key` 约 `315K`，`generic_map_lookup` 约 `3.5K`。
- `log_parse_filter`: `typed_map_direct` 约 `321K`，`dynamic_register_key` / `dynamic_short_string_key` 约 `240K`，`direct_string_key` 约 `204K`，`known_string_key` 约 `85K`，`generic_map_lookup` 约 `2.2K`。
- `inventory_reorder`: `dynamic_register_key` / `dynamic_short_string_key` 约 `426K`，`typed_map_direct` 约 `420K`，`direct_string_key` 约 `179K`，`generic_map_lookup` 约 `2.8K`。
- `config_defaults_merge`: `known_string_key` 约 `535K`，`direct_string_key` 约 `466K`，`typed_map_direct` 约 `396K`，`generic_map_lookup` 约 `69K`。
- `route_permission_check`: 角色等级表的 string literal key lookup 已被 const map fold 消除；剩余压力主要来自 branch/arithmetic。

### 当前 syntax smoke

最近一次验证命令：

```bash
for f in examples/syntax/*.lk; do perl -e 'alarm 30; exec @ARGV' target/debug/lk "$f"; done
```

结果：

- `examples/syntax/*.lk` 全部通过，包括 `unsupported.lk`。
- 修复了 named-args 示例中 `let i = start; i += step_val;` 对未注解参数派生局部变量的 compound assignment 类型约束缺口。
- `examples/syntax/*.lk` 已在 30 秒单文件 alarm 下复验通过，没有发现本轮 loop/opcode 改动造成的死循环。

## 已落地的通用优化

### Metrics 与 opcode 基础

- `vm-profile` feature gate 让普通构建下 runtime metrics 变成 compile-time no-op，消除默认 build 的 profile 开销。
- runtime profile 的 per-frame opcode/write/index buffers 现在只在 test 或 `vm-profile` 构建中存在；普通 release 使用零大小 no-op frame，避免未启用 metrics 时仍在函数入口清零 profile 数组。
- `Opcode` 已使用 7-bit encoding，并按语义分区连续重编号到 `0..105`；`from_bits` 不再处理 gap。
- `Opcode::ForLoopI` 曾临时复用 `Extra = 62`，把静态正/负 step 的 range loop tail 从 `AddInt + Jmp + 下轮 Cmp/Test` 压成单条 loop opcode；当前已随分区重排移出该历史槽位。module artifact version 已 bump 到 `2`，避免旧 artifact 的 raw instruction word 被新 opcode 编号误解。
- workload benchmark runner 按 workload 逐项运行 LK/Lua/AOT，带进度与单 workload timeout。
- `GetIndex` / `SetIndex` 的 profile counters 已按 `PerfIndexFact.target_kind` 归入 list/map/string/generic，避免所有 index ops 都落到 generic container bucket。
- dynamic opcode histogram 已接入 `vm-profile`；热路径在函数内用本地数组累计，函数退出时批量 merge，避免每条 opcode 做 TLS/atomic 写导致 profile 运行失真。
- register write source counters 已接入 `vm-profile`；热路径同样使用函数内本地数组累计，函数退出时批量 merge，避免逐 write atomic 计数导致 profile 运行失真。
- index key metrics 已接入 `vm-profile`；`GetIndex` / `SetIndex` 现在按 known string key、dynamic register key 及其 int/short-string/object/other 子类、runtime map key、typed direct、generic lookup、object key 和 slow path 分桶。
- fused branch helper 已改为使用当前栈帧的局部 `collect_metrics` 开关，避免普通 release 热分支路径读取 executor metrics 字段；尝试把 fused branch fact 改成 absolute jump target 会扩大 fact/Option 访问成本并让 release wall-clock 退化，已回退，当前仍保留 compact jump offset。

### Compiler lowering

- 语句上下文的 mutating method call，例如 `map.set(k, v);`，只生成 runtime set opcode；动态 key/list 写入保留 `SetIndex`，Map/Object + 短字符串 literal key 写入使用 `SetFieldK`，不再 materialize 未使用的 `nil` 返回值。
- 表达式上下文的 `let x = map.set(k, v);` 仍保留 `nil` 结果语义。
- 字符串 literal `.len()` 直接 lower 成 `LoadInt(char_count)`；非 ASCII 使用字符数语义。
- 3+ part template string 现在 lower 为 `ConcatN`，把连续 parts 寄存器一次拼接为结果字符串；2-part template 仍使用 `ConcatString`，单表达式 template 仍保留必要 `ToString`。template string 写入 local/目标寄存器时会 direct-to-destination，避免 `ConcatString/ConcatN temp; Move dst temp`。
- 编译期识别 `string_expr.split(sep).join(sep).len()`，在接收者可静态确认为字符串且分隔符是同一个 literal/local 时直接 lower 为 `string_expr.len()`；这避免 `log_parse_filter` 中重复 materialize split list 和 joined string。
- 已知 `Map` / `Object` 目标上的短字符串 literal key 不再生成 `LoadString` key register；读路径使用 `GetFieldK`，写路径使用 `SetFieldK`。动态 key 或非 Map/Object 目标仍保留 `GetIndex` / `SetIndex`。
- 只读本地 const map 上的 `map.get(local, "literal")` 在 string key 且 int value 时直接 lower 成 scalar load；loop const collector 会把这类 fold 出来的 scalar value 纳入循环前缓存，避免 `role_levels` 这类只读表 lookup 在循环分支中反复 `LoadInt`；循环体内新建的 map 不记录该 fact，`.set` / rewritten set-index / assignment 会清除 fact，避免 mutable loop-local map 被错误折叠。
- 裸 `return;` 和函数/程序隐式返回直接发出 `Return 0 0 0`，不再生成 `LoadNil + Return count=1`。
- 裸 `return;` 和函数/程序隐式返回现在发出 `Return0`；单值 `return value` 发出 `Return1`。旧 `Return A B` 仍保留给多返回值和手写/旧 artifact，VM 与 LLVM lowering 使用统一 return count 语义。
- `ReturnValues::None.into_vec()` 规范化为 `[RuntimeVal::Nil]`，保持外部 `ExecResult.returns` 语义。
- 普通 `while` 和 direct-inline `while` 的 loop-back target 都会跳过 condition 前缀里的 scalar const loads，避免热循环每轮重复 materialize `LoadInt` / `LoadString` 条件常量。
- 普通 `while`、direct-inline `while` 和 range `for` 会为 loop body 中的标量 literal 建立 loop-local cache；template string 的短 literal parts 也会进入该 cache，避免 `"prefix${i}"` 在每轮重新 `LoadString` prefix。cached literal register 会纳入 live register floor，并被视为不可消费源，避免 `SetIndex` / `ListPush` / move policy 把缓存值写成 `nil`。
- range/while loop const collector 现在会识别可 direct-inline 的函数调用，并把 inline body 内的标量 literal 纳入同一个 loop-local cache，避免 `is_prime` 这类 inline body 的 `0/1/2/3` 在外层循环中反复 materialize。
- loop const collector 会预收集 loop body 内可折叠的 int 表达式，并只在结果已缓存时复用，避免把热表达式直接退化成每轮 `LoadInt`。
- compound assignment 如果算术结果已经写回目标寄存器，不再额外生成 `Move dst, dst` 自拷贝。
- local/global `+=` 的整数加法链在安全条件下可直接累加到目标寄存器，避免 materialize `a + b + c` 的中间树；RHS 读取目标变量、cell local 或非 int-like 场景仍走普通语义路径。
- Straight-line 的简单本地复制赋值 `a = b` 会重绑定 local slot 而不发 `Move`；共享 slot 后续写入走 copy-on-write。该优化在 control-flow / loop body 内暂时关闭，因为当前 register VM 还没有 phi/loop-carried slot rewrite，直接重绑定会让已生成的分支条件继续读取旧寄存器。
- `let dst = math.floor(expr)` 在 `expr` 可由 literals / local facts / Int arithmetic 静态证明为 Int-like 时，会直接把 `expr` lower 到 `dst`，避免 `DivInt temp; Move dst temp` 这类临时结果搬运。该优化要求 `math` 是未被 local/function/native shadow 的外部全局。
- `let dst = map.get(map_value, key)` 在 `map` 是未被 shadow 的外部全局时，会把 `GetFieldK` / `GetIndex` 直接写入 `dst`，避免 `GetIndex temp; Move dst temp`。它不改变 map lookup 语义；const map fold、动态 key 和 mutable map invalidation 仍沿用原有 facts/运行时路径。
- 静态正/负 step 的 range `for` 现在使用 `ForLoopI` 完成 index update、边界判断和跳回；LLVM scalar block 同步支持该 opcode，保证 native lowering 与 VM 语义一致。
- 字符串 indexed `for` 中如果 value binding 只用于 `.len()`，且没有真实读取或深层 shadow，compiler 会跳过每轮字符 materialization，直接绑定已缓存的 `1`；普通读取 value 的场景仍保留 `GetIndex`。

### Container / String fast path

- 当 `index_fact` 已知时跳过 `bump_shape_generation`，避免热循环里的 map set/get 反复 invalidating inline cache。
- 空 `TypedList::Mixed` 第一次 `push` 标量值时会采用对应 typed backing，例如 `Int/Float/Bool/String`，避免 `[]` 后连续 `push(Int)` 永远停在 mixed list；这让 `sliding_window_sum` 的热 list read 走 typed int list。
- compiler 的 `ListPush` lowering 会把 pushed value kind 写回 list register fact；空 list 的第一次 push 采用 pushed kind，后续 push 做 kind join，但不保留静态 `known_len`，避免循环内 push 产生错误长度 fact。
- `SetIndex` 的 ShortStr key 使用 `SmallKey`，避免 `to_owned()` 产生 `String` 分配。
- `ShortStr::concat`、`concat_int`、`concat_int_prefix` 对短结果走零分配路径。
- `GetIndex` 在已知 `StringInt` map 时直接走 `values.get(key).copied().map(RuntimeVal::Int)`。
- `GetIndex` 的 string-key direct path 已扩展到 `StringMixed`、`StringInt`、`StringFloat`、`StringBool`，大幅减少 `TypedMap::get_str` 泛化路径。
- `GetIndex` 在静态 fact 确认为 typed list 且运行时 target/key 仍匹配时，会在 dispatch arm 前直接读取 list element，避开 slice-key 检查和 heap-index 泛化分派；如果 value kind 已确认为 `Int`，会先尝试 `TypedList::Int` backing 直读，避免完整 `TypedList` type match；不匹配时回落原 `get_index` 语义。
- `GetIndex` 在静态 fact 确认为 heap string 且 key 是整数时直接走 string index，不再落到 generic heap-index slow path；本轮普通 release 低样本中 `string_key_hash` 维持在约 `1.15x`。
- `set_map/set_list/set_object_index_handle` 增加 `has_static_fact`，让静态 fact 路径少做不必要的 shape 更新。

### Numeric / Move hot path

- `Move` dispatch 对非 heap scalar 源值不再查询 `register_copy` move policy；只有 `Obj` 源值才读取 move/clone fact。这保留 heap ownership 与 clone metrics 语义，同时减少 `gcd_batch`、`matrix_3x3_multiply`、map workload 中大量 scalar `Move` 的 fact lookup 开销。
- 连续 `Move` dispatch 的 next-op 判断改为 `self.pc >= code.len() || code[self.pc].opcode() != Opcode::Move`，避免热 `Move` 链每步构造 `Option`；在当前 opcode 基线下，默认 VM `RUNS=3 EXTRA_RUNS=5` geomean 从 `0.910x` 小幅降到 `0.905x`，checksum 全部一致。
- `MulInt` dispatch 与其它 `*Int` arithmetic opcode 对齐，只把 `Int * Int` 放在热 arm；混合 float fallback 仍走现有 dynamic numeric path，避免 int-heavy workload 在热路径匹配 float 分支。

### LLVM / native lowering

- `FunctionData` 在内存中的 `ModuleArtifact` 转换里保留 `Function.performance`，但序列化时跳过该字段。
- LLVM `GetIndex` / `SetIndex` 的 scalar block、straightline、callee eval、map mutate 路径统一使用 `native_known_string_key`；`GetFieldK` / `SetFieldK` 已同步接入 LLVM straightline、callee eval、scalar facts 和 scalar block lowering。
- 这保证 compiler 的 key materialization elision 不会让 native lowering 退回动态 key 路径。
- LLVM scalar block 已补齐 dynamic string-map `GetFieldK` lowering，const field key 可以复用 dynamic string map get helper；optional `Maybe*` 静态值经过 `BrNil` / `BrNotNil` 时不再被误判为确定非 nil，而是读取 `present.slot`。这修复了 AOT `config_defaults_merge` 等稀疏 map/default 场景的 checksum mismatch。
- AOT 全 workload 的 entry scalar facts 阻塞曾推进到：`"n${i}"` 这类静态字符串前缀 + 动态整数后缀的 string-int map key 可被 facts 接受，`GetIndex` / `SetIndex` 的 known-string-key 占位寄存器也能推断 `DynamicMap<String, Int>` 返回/写入 kind。
- 当前 full-suite AOT smoke 不能作为通过状态记录：`RUN_AOT=1 RUNS=1 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=120 bash bench/run_workload_bench.sh` 会跳过 AOT compile，阻塞点是 loop 后 dynamic map `GetIndex` shape 尚未被 native lowering 支持。
- 本轮修正了 AOT string-key dynamic map 和 optional compare 的几个语义缺口：
  - dynamic string-int map `SetIndex` 对非 const string key 使用当前 key pointer 做 runtime prefix/number split，避免模板 key 的源寄存器在后续算术中被复用后读到陈旧 suffix。
  - dynamic text `len()` 对 `StrPtr` 部分改为 `strlen`，修复 `status=${status}` 这类模板长度。
  - `GetIndex` optional result 与 `nil` 比较时使用 `present.slot`，并让静态 collection compare 保留 `Maybe*` vs `Nil` 的 present 语义。
- AOT correctness 的全 workload 通过只能作为历史记录；当前下一步应先补 dynamic map `GetIndex` native lowering，再基于 AOT/VM/Lua 三方低样本 timing 判断 native hot path 优先级，尤其是 `two_sum_map`、`histogram_group_count`、`inventory_reorder` 仍可能因 runtime split/helper 调用偏慢。
- `LK_NATIVE_BLOCK_TRACE=1` 仍可用于打印 scalar block lowering 的 pc/disassembly。

### Hot/cold 结构

- `ToString`、`ConcatString`、`StringStartsWith`、`StringSplit`、`ListJoin`、`Contains` 不默认标 `#[cold]`。
- `Test`、`Jmp`、`Len`、`ToIter`、普通 `Call` 不默认标 `#[cold]`。
- `relative_pc` / `relative_pc_from` 是正常跳转路径；只有 jump-before-start 错误构造保留 `#[cold] #[inline(never)]`。

## 当前热点

### Container / Index / String

最差 ratio 仍集中在 map/list/string/index 密集 workload：

- `two_sum_map`
- `histogram_group_count`
- `inventory_reorder`
- `route_permission_check`

最近 profile-enabled 低样本 counters：

- `sliding_window_sum`: `1,392,001` list ops，`1,932,001` typed branches。
- `histogram_group_count`: `742,000` map ops。
- `inventory_reorder`: `422,800` map ops，`9,200` list ops。
- `route_permission_check`: 角色等级表 lookup 已被 const map fold 消掉，低样本 ratio 已降到约 `1.18x`；剩余主要是 `388K` 级 typed branches 和 arithmetic。
- `fraud_rule_scoring`: `85,000` map ops，`815,772` typed branches。
- `string_key_hash`: `143,828` string ops。

下一步应确认：

- `known_string_key` / `PerfIndexFact` 在热循环中的实际命中率。
- `GetIndex` / `SetIndex` 是否仍频繁构造 `RuntimeMapKey`。
- typed map/list fast path 是否真正减少 register writes 和 heap lookup，而不只是减少 variant match。
- string key、`Arc<str>`、ShortStr 转换是否仍在循环内重复发生。

### Register Writes

profile-enabled 统计显示 register writes 主要来自 `arithmetic`、`const_load`、`move`、`index` 和 `string`：

- aggregate coverage: `arithmetic` 约 `21.85M`，`move` 约 `6.06M`，`const_load` 约 `3.37M`，`string` 约 `2.55M`，`index` 约 `2.21M`。
- `binary_search`: `arithmetic` 约 `5.76M`，`move` 约 `1.44M`，`const_load` 已降到约 `50`。
- `sliding_window_sum`: `arithmetic` 约 `4.66M`，`index` 约 `912K`，`const_load` 已降到约 `4K`。
- `histogram_group_count`: `arithmetic` 约 `1.80M`，`move` 约 `858K`，`string` / `index` 各约 `427K`，`const_load` 已显著下降。
- `fraud_rule_scoring`: `arithmetic` 约 `1.24M`，`move` 约 `170K`，`string` / `index` 各约 `85K`，`const_load` 已降到约 `97K`。

这说明下一步不应只盯 register write helper 本身；更高收益方向是降低 arithmetic 中间寄存器写入、继续消除 `Move`、做 compare branch lowering，并对 hot `GetIndex` 结果的立即消费做 lowering。

本轮已把已有 `while` condition scalar-const-load LICM 扩展到 direct-inline `while`，并为普通 `while` / direct-inline `while` / range `for` 增加 loop body scalar literal cache；随后增加了 direct-inline body literal cache、straight-line local rebind、compound assignment 自拷贝消除、same-separator split/join len 折叠、空 list push typed backing 采用、local/global compound add-chain lowering、字符串 indexed-for `.len()` value elision、known typed-list index dispatch fast path、scalar `Move` fact-lookup elision 和 `MulInt` hot arm 瘦身。`ForLoopI` 把动态 `Jmp` 从约 `6.43M` 降到约 `3.19M`，动态 `AddInt` 从约 `11.35M` 降到约 `8.10M`。连续 `Move` 批处理保留每条 `Move` 的 profile 计数，但减少相邻 `Move` 的外层 dispatch 循环开销；本轮最终 release `RUNS=6 EXTRA_RUNS=0` geomean 复验为 `0.889x`，AOT 同步复验里的 VM/Lua 为 `0.964x`。尝试把静态 range `step_value` 存进 `PerfForLoopFact`、避免 `ForLoopI` 热路径读取 step register，把 fused branch validated jump 改成无 `Result` PC 更新，以及在没有 loop-carried/assignment 版本控制的情况下记录 scalar const local，都没有带来可保留收益或会造成 while 条件被折成常量进而死循环，已回退。下一步仍需继续处理 arithmetic immediate、compare branch、loop-carried `Move` 来源，以及更系统的 phi/native hot-loop lowering。

本轮还验证过把 loop body scalar literal cache 扩展到 indexed `for`，该方向会破坏 LLVM/native lowering 对 list/map iteration 的静态语义，例如动态 map iteration 可被错误折成 `[[1, 10], [20, 20]]`。该尝试已回退；indexed `for` 需要先补齐更明确的 loop-local register ownership / native lowering 协议，再做 literal cache。

本轮还验证过把 `Test` / `Jmp` handler 强制 `#[inline(always)]`，以及把 fused branch fact 的两次查询合并为一次 slot 读取；两者 checksum 都正确，但 release 低样本 geomean 没有改善或出现退化，已回退。后续 control-flow 优化应优先做 opcode/IR 级 compare-branch lowering、`ForLoopI`/native hot-loop lowering，而不是继续微调现有 helper 形状。

### Branch / Compare

branch-heavy workload 仍有大量 typed branch：

- `binary_search`: `3,961,228`
- `sliding_window_sum`: `1,932,001`
- `stock_max_profit`: `1,626,001`
- `gcd_batch`: `817,373`

`try_fused_bool_branch` 已减少部分 bool 中间值，但当前仍要进入 compare arm、检查 fused fact、再更新 PC。下一步应优先做通用 control-flow lowering，避免 compare 结果寄存器写入。

## 优先级

### P0：测量链路

1. 已完成：`coverage --runtime` 在未启用 `vm-profile` 时明确提示 counters 不可用，避免全 0 被误读。
2. 已完成：增加 dynamic opcode histogram，并在 `coverage --runtime` / `VM profile:` / benchmark profile 表输出。
3. 已部分完成：`GetIndex` / `SetIndex` 按静态 target kind 归入 list/map/string/generic；下一步继续细分 `Len`、`ListPush` 和 key construction。
4. 已完成：register write source counters 已用函数内本地累计、退出批量 merge 接入。
   - 注意：register write source counters 不能在热路径逐次 atomic 计数；本轮验证过该方案会让 `PROFILE_WORKLOADS=1` 超时。
5. 已部分完成：index key metrics 已接入；下一步继续把 `generic_map_lookup` 降下来，并细分 RuntimeMapKey construction 与 typed direct miss/fallback。

### P1：继续优化 container/index/string

1. 对 hot `GetIndex` / `SetIndex` 增加更直接的 typed map/list helper。
2. 减少 `RuntimeMapKey` 构造、heap kind 重复判断和 `Result` 错误路径污染。
3. 对 string key 和 short-string path 做循环内复用或 interning。

### P2：消除中间 register writes

1. 消除立即消费的 compare 结果寄存器。
2. 消除 immediately-used index 结果中间值。
3. 减少 loop-local move 和 call-window 清理写入。

### P3：拆分 dispatch loop

1. 将小而确定的 typed fast path 拆成 tiny helper。
2. hot helper 使用 `#[inline]`，只有实测收益明确时才用 `#[inline(always)]`。
3. dynamic fallback、错误构造、GC slow path、跨语言调用使用 `#[cold] #[inline(never)]`。
4. 每拆一组都跑 workload benchmark 和 vm-profile counters。

### P4：Native / JIT

如果目标坚持 geomean `< 0.9x`，解释器微优化大概率不够。需要把 hot loop 编译成原生代码，优先复用现有 LLVM/native lowering 的 scalar block 能力，并保留 VM 作为 fallback 与 correctness oracle。

## 不建议

- 当前阶段不新增 `ListFoldAdd`、`MapValuesFoldAdd` 这类 workload-shaped 专门 opcode；`GetFieldK` / `SetFieldK` 已按 Lua-style operand-shape specialization 落地。
- 不用函数指针跳表替代 `match` 作为默认路线；indirect call 可能损失 inlining 和分支预测。
- 不盲目给所有 helper 加 `#[inline(always)]`。
- 不在非 LLVM VM 路径使用 `unsafe`。

## 验证命令

历史完整收口验证：

```bash
cargo fmt --all -- --check
cargo test -p lk-core --lib
cargo build --release -p lk-cli
RUN_AOT=1 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
for f in examples/syntax/*.lk; do perl -e 'alarm 30; exec @ARGV' target/release/lk "$f" >/tmp/lk_syntax.out || exit $?; done
target/release/lk compile exe bench/workloads_business_algorithms.lk --output /tmp/lk-workloads-aot-check
target/release/lk bench/workloads_business_algorithms.lk
/tmp/lk-workloads-aot-check
```

历史结果：

- `cargo fmt --all -- --check`: pass
- `cargo test -p lk-core --lib`: `904 passed`
- `cargo build --release -p lk-cli`: pass
- `target/release/lk compile exe bench/workloads_business_algorithms.lk --output /tmp/lk-workloads-aot-check`: pass，生成 `/tmp/lk-workloads-aot-check`
- `target/release/lk bench/workloads_business_algorithms.lk` 与 `/tmp/lk-workloads-aot-check`: 20 个 workload checksum 全部一致
- `RUN_AOT=1 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh`: pass，20 个 workload VM/Lua/AOT checksum 全部一致，VM/Lua geomean `1.064x`，AOT/Lua geomean `0.351x`
- release workload benchmark: checksum 全部一致；最新默认 VM `RUNS=3 EXTRA_RUNS=5` geomean 为 `0.783x`，此前 ordering compare-test hot arm cleanup 后为 `0.785x`，`MidInt` 后为 `0.792x`，`AddListInt` / `SubListInt` 后为 `0.797x`，`Add2Int` 后为 `0.796x`，profile 分桶后复测为 `0.800x`，保留 `ConcatN` register-window 直写两轮为 `0.798x` / `0.793x`；template string direct-to-destination 把 static `Move` 从 `313` 降到 `300`，ordering compare-test hot arm cleanup 降低 facts-confirmed `TestLtInt` / `TestLeInt` / `TestGtInt` dispatch 分支成本，`MidInt` 降低 facts-confirmed midpoint expression 的 `AddInt + DivInt` dispatch，`AddListInt` / `SubListInt` 降低 `sliding_window_sum` typed int-list accumulator dispatch，`Add2Int` 降低 `state_machine_transitions` 的 compound-add pair dispatch，empty mixed-map fast path A/B 改善 `config_defaults_merge`，此前 `RUNS=6` geomean `0.889x`
- release workload benchmark with AOT: checksum 全部一致；VM/Lua 低样本 geomean `0.964x`，AOT/Lua 低样本 geomean `0.450x`
- `cd website && bun run build`: pass；官网已把 `/performance` 保留为用户向性能路径概览，新增 `/technical` 承载 opcode、profile counter 和 lowering 细节。
- syntax smoke: `examples/syntax/*.lk` 全部通过，30 秒 alarm 未触发
- coverage/profile counters 未在本轮最终 release build 后重跑；保留上一节 profile 作为方向性历史证据。
