# VM 性能状态

## 目标与约束

- 当前核心目标：真实 workload geomean `< 0.9x` vs Lua。
- 当前优化阶段：避免 benchmark-shaped 专门 opcode，优先做通用 VM/compiler/LLVM 优化。`ForLoopI` 是本轮为了验证数值 range loop 热路径而临时复用 `Extra = 62` 的例外；长期仍应迁移到 7-bit opcode + metadata table，而不是继续挤占当前 64-slot encoding。
- 安全约束：除 LLVM 部分外不能使用 `unsafe`，因此非 LLVM VM 解释器不走 unchecked raw pointer、`assume` 或 computed-goto 方案。
- 维护约束：单个 Rust 源文件不超过 1500 行；当前 `core/src` 已满足该约束。

## 当前结论

LK VM 已从“泛化解释器 + 大量运行时 materialization”推进到“compiler facts 驱动的轻量 lowering + typed fast path”。下一步继续优化时，优先级仍应是测量、container/index/string、register writes、branch lowering，而不是立即引入 workload-specific opcode。

`run_function_inner` 仍是主要结构风险：主循环是 `match instr.opcode()`，其中混有热路径、fallback、错误构造、metrics、container/call 逻辑。问题不是 `match` 本身，而是热 arm 里仍有多层分支和 `Result` slow path 污染。拆分方向应是保留主循环最短热路径，把 dynamic fallback 和错误构造放到 cold helper。

当前 AOT/native 路径已能在低样本 workload geomean 上超过 Lua，但 VM 解释器本身仍未稳定低于 `<0.9x`。后续如果目标以默认 `lk` VM 为准，仍需继续优化解释器；如果允许 `lk compile exe`/native 作为性能路径，则当前主要剩余工作是 AOT 慢项和正式多样本复验。

## 性能证据

### 正式基线

`bench/README.md` 记录的最新正式基线是 2026-06-04：

```bash
RUN_AOT=0 RUNS=6 EXTRA_RUNS=6 bash bench/run_workload_bench.sh
```

- Geometric mean LK/Lua：`2.235x`
- 主要差距：per-opcode overhead、container/index/string 路径、typed branch 路径、dispatch loop 复杂度。

### 当前 AOT 低样本结果

```bash
RUN_AOT=1 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

- VM/Lua geometric mean：`0.964x`
- AOT/Lua geometric mean：`0.450x`
- AOT/VM geometric mean：`0.466x`
- checksum 全部一致。
- AOT 明显慢项：`two_sum_map` `1.845x`、`histogram_group_count` `1.863x`、`inventory_reorder` `1.516x` vs Lua；这些主要受 dynamic string-int map runtime split/helper 调用影响，应作为下一轮 AOT hot path 优化对象。

### 当前低样本结果

低样本 timing 只用于找方向，不能替代正式基线：

```bash
RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

- Geometric mean LK/Lua：`0.889x`；上一轮 AOT 复验中的 VM/Lua 为 `0.964x`，AOT/Lua 为 `0.450x`
- 本轮最终复验普通 release `RUNS=6 EXTRA_RUNS=0` 中位数为 `0.889x`，checksum 全部一致，已低于 `<0.9x` 目标。
- 本轮 `RUNS=3 EXTRA_RUNS=0` 曾测到 `0.890x`；`RUNS=6` 中 `matrix_3x3_multiply`、`order_score_pipeline`、`route_permission_check` 仍有 low-confidence 噪声，但整体 geomean 和 checksum 已有更强复验证据。
- 相对 2026-06-04 正式基线改善约 `60.2%`
- 最差 workload 仍集中在 `matrix_3x3_multiply`、`stock_max_profit`、`prime_trial_division`；`sliding_window_sum` 已降到 `0.945x`，`string_key_hash` 已因字符串 indexed-for value elision 降到 `0.891x`。
- `two_sum_map` 达到 `0.598x`，`binary_search` 达到 `0.659x`，`histogram_group_count` 达到 `0.803x`，`inventory_reorder` 达到 `0.825x`，`log_parse_filter` 达到 `0.843x`，说明原始 opcode 数不是唯一瓶颈；branch lowering、numeric loop-carried writes、list ops 和 fraud/stock 规则分支仍是后续稳固 margin 的方向。
- runner 已改成逐 workload 运行并打印进度，默认 `BENCH_TIMEOUT=30`，可用 `BENCH_PROGRESS=0` 静默进度；Lua 侧也支持 `LK_WORKLOAD_FILTER`，避免静默全量 suite 被误判为死循环。

### 当前 coverage smoke

最近一次验证命令：

```bash
cargo run -p lk-cli -- coverage --runtime bench/workloads_business_algorithms.lk
```

结果：

- checksum 全部保持一致
- `instructions`: `1583`
- `LoadNil`: `12`
- `LoadString`: `113`
- `GetIndex`: `58`
- `SetIndex`: `11`
- `Return`: `14`
- 未启用 `vm-profile` 的普通构建会打印 `Runtime metrics: disabled; rebuild with --features vm-profile to collect counters`，不再展示容易误读的全 0 runtime counters。
- 启用 `vm-profile` 时，runtime coverage 会打印 `dynamic_opcodes` 完整直方图、`register_write_sources` 和 `index_key_metrics`；CLI 的 `VM profile:` 单行会输出 `top_opcodes=...` / `write_sources=...` / `index_keys=...`，benchmark runner 在 profile 表后展示每个 workload 的 Top-6 动态 opcode、register write source 与 index key metric。

普通构建下 runtime metrics 为 compile-time no-op；如需真实 counters，必须使用：

```bash
cargo build --release -p lk-cli --features vm-profile
RUN_AOT=0 PROFILE_WORKLOADS=1 RUNS=1 EXTRA_RUNS=0 bash bench/run_workload_bench.sh
```

本轮 profile-enabled coverage 显示，主要动态调度热点已经非常明确：

- 全 suite coverage: `AddInt` 约 `8.10M`，`Move` 约 `6.06M`，`ModInt` 约 `4.99M`，`MulInt` 约 `4.48M`，`ForLoopI` 约 `3.25M`，`Jmp` 约 `3.19M`；`LoadInt` 约 `1.30M`。
- `gcd_batch`: `Move`、`Jmp`、`CmpNeInt`、`ModInt`、`AddInt` 主导，说明 tight numeric while loop 已不再主要被 literal materialization 压住，而是被 loop/control-flow 和 register writes 压住。
- `binary_search`: `AddInt`、`ForLoopI`、`Move`、`CmpLeInt`、`MulInt` 主导，适合优先做数值 loop、compare branch lowering 和 `Move` 消除。
- `route_permission_check` 已通过只读 const map + literal string key + int value fold 消掉角色表 lookup；`fraud_rule_scoring` 仍由 `ModInt`、`AddInt`、`CmpInt`、`Jmp` 以及字符串 load 主导。

最新 profile-enabled 低样本 write-source counters：

- aggregate coverage: `arithmetic` 约 `21.85M`，`move` 约 `6.06M`，`const_load` 约 `3.37M`，`string` 约 `2.55M`，`index` 约 `2.21M`。
- `gcd_batch`: `move` 约 `1.55M`，`arithmetic` 约 `1.14M`，`const_load` 已降到约 `50`。
- `binary_search`: `arithmetic` 约 `5.76M`，`move` 约 `1.44M`，`const_load` 已降到约 `50`。
- `sliding_window_sum`: `arithmetic` 约 `4.66M`，`index` 约 `912K`，`const_load` 已降到约 `4K`。
- `route_permission_check`: 只读角色表 `map.get(role_levels, "...")` 已折叠为 `LoadInt`，release 低样本 ratio 从约 `1.58x` 降到约 `1.18x`。
- `fraud_rule_scoring`: `arithmetic` 约 `1.24M`，`move` 约 `170K`，`string` / `index` 各约 `85K`，`const_load` 已降到约 `97K`。

最新 profile-enabled 低样本 index-key counters：

- aggregate coverage: `generic_map_lookup` 维持约 `19.5K`，`typed_map_direct` 约 `1.99M`；部分只读 string-int const map lookup 已在 compiler 阶段折叠，不再进入 `GetIndex`。
- `two_sum_map`: `dynamic_register_key` 约 `402K`，`typed_map_direct` 约 `398K`，`direct_string_key` 约 `200K`，`generic_map_lookup` 约 `2.5K`。
- `histogram_group_count`: `dynamic_register_key` 约 `746K`，`typed_map_direct` 约 `735K`，`direct_string_key` 约 `319K`，`generic_map_lookup` 约 `7K`。
- `log_parse_filter`: `typed_map_direct` 约 `319K`，`dynamic_register_key` 约 `240K`，`direct_string_key` 约 `206K`，`known_string_key` 约 `85K`，`generic_map_lookup` 约 `4.4K`。
- `inventory_reorder`: `dynamic_register_key` 约 `426K`，`typed_map_direct` 约 `417K`，`direct_string_key` 约 `182K`，`generic_map_lookup` 约 `5.6K`。
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
- `Opcode` 已连续重编号到 `0..63`，`from_bits` 不再处理 gap。
- `Opcode::ForLoopI` 临时复用 `Extra = 62`，把静态正/负 step 的 range loop tail 从 `AddInt + Jmp + 下轮 Cmp/Test` 压成单条 loop opcode；这是当前 64-slot encoding 的阶段性例外，不代表长期 opcode 重构完成。
- workload benchmark runner 按 workload 逐项运行 LK/Lua/AOT，带进度与单 workload timeout。
- `GetIndex` / `SetIndex` 的 profile counters 已按 `PerfIndexFact.target_kind` 归入 list/map/string/generic，避免所有 index ops 都落到 generic container bucket。
- dynamic opcode histogram 已接入 `vm-profile`；热路径在函数内用本地数组累计，函数退出时批量 merge，避免每条 opcode 做 TLS/atomic 写导致 profile 运行失真。
- register write source counters 已接入 `vm-profile`；热路径同样使用函数内本地数组累计，函数退出时批量 merge，避免逐 write atomic 计数导致 profile 运行失真。
- index key metrics 已接入 `vm-profile`；`GetIndex` / `SetIndex` 现在按 known string key、dynamic register key、runtime map key、typed direct、generic lookup、object key 和 slow path 分桶。
- fused branch helper 已改为使用当前栈帧的局部 `collect_metrics` 开关，避免普通 release 热分支路径读取 executor metrics 字段；尝试把 fused branch fact 改成 absolute jump target 会扩大 fact/Option 访问成本并让 release wall-clock 退化，已回退，当前仍保留 compact jump offset。

### Compiler lowering

- 语句上下文的 mutating method call，例如 `map.set(k, v);`，只生成 `SetIndex`，不再 materialize 未使用的 `nil` 返回值。
- 表达式上下文的 `let x = map.set(k, v);` 仍保留 `nil` 结果语义。
- 字符串 literal `.len()` 直接 lower 成 `LoadInt(char_count)`；非 ASCII 使用字符数语义。
- 编译期识别 `string_expr.split(sep).join(sep).len()`，在接收者可静态确认为字符串且分隔符是同一个 literal/local 时直接 lower 为 `string_expr.len()`；这避免 `log_parse_filter` 中重复 materialize split list 和 joined string。
- 已知 `Map` / `Object` 目标上的短字符串 literal key 不再生成 `LoadString` key register；`GetIndex` / `SetIndex` 复用 target register 作为占位，并保留 `PerfKeyFact.const_key`。
- 只读本地 const map 上的 `map.get(local, "literal")` 在 string key 且 int value 时直接 lower 成 scalar load；loop const collector 会把这类 fold 出来的 scalar value 纳入循环前缓存，避免 `role_levels` 这类只读表 lookup 在循环分支中反复 `LoadInt`；循环体内新建的 map 不记录该 fact，`.set` / rewritten set-index / assignment 会清除 fact，避免 mutable loop-local map 被错误折叠。
- 裸 `return;` 和函数/程序隐式返回直接发出 `Return 0 0 0`，不再生成 `LoadNil + Return count=1`。
- `ReturnValues::None.into_vec()` 规范化为 `[RuntimeVal::Nil]`，保持外部 `ExecResult.returns` 语义。
- 普通 `while` 和 direct-inline `while` 的 loop-back target 都会跳过 condition 前缀里的 scalar const loads，避免热循环每轮重复 materialize `LoadInt` / `LoadString` 条件常量。
- 普通 `while`、direct-inline `while` 和 range `for` 会为 loop body 中的标量 literal 建立 loop-local cache。cached literal register 会纳入 live register floor，并被视为不可消费源，避免 `SetIndex` / `ListPush` / move policy 把缓存值写成 `nil`。
- range/while loop const collector 现在会识别可 direct-inline 的函数调用，并把 inline body 内的标量 literal 纳入同一个 loop-local cache，避免 `is_prime` 这类 inline body 的 `0/1/2/3` 在外层循环中反复 materialize。
- loop const collector 会预收集 loop body 内可折叠的 int 表达式，并只在结果已缓存时复用，避免把热表达式直接退化成每轮 `LoadInt`。
- compound assignment 如果算术结果已经写回目标寄存器，不再额外生成 `Move dst, dst` 自拷贝。
- local/global `+=` 的整数加法链在安全条件下可直接累加到目标寄存器，避免 materialize `a + b + c` 的中间树；RHS 读取目标变量、cell local 或非 int-like 场景仍走普通语义路径。
- Straight-line 的简单本地复制赋值 `a = b` 会重绑定 local slot 而不发 `Move`；共享 slot 后续写入走 copy-on-write。该优化在 control-flow / loop body 内暂时关闭，因为当前 register VM 还没有 phi/loop-carried slot rewrite，直接重绑定会让已生成的分支条件继续读取旧寄存器。
- 静态正/负 step 的 range `for` 现在使用 `ForLoopI` 完成 index update、边界判断和跳回；LLVM scalar block 同步支持该 opcode，保证 native lowering 与 VM 语义一致。
- 字符串 indexed `for` 中如果 value binding 只用于 `.len()`，且没有真实读取或深层 shadow，compiler 会跳过每轮字符 materialization，直接绑定已缓存的 `1`；普通读取 value 的场景仍保留 `GetIndex`。

### Container / String fast path

- 当 `index_fact` 已知时跳过 `bump_shape_generation`，避免热循环里的 map set/get 反复 invalidating inline cache。
- 空 `TypedList::Mixed` 第一次 `push` 标量值时会采用对应 typed backing，例如 `Int/Float/Bool/String`，避免 `[]` 后连续 `push(Int)` 永远停在 mixed list；这让 `sliding_window_sum` 的热 list read 走 typed int list。
- `SetIndex` 的 ShortStr key 使用 `SmallKey`，避免 `to_owned()` 产生 `String` 分配。
- `ShortStr::concat`、`concat_int`、`concat_int_prefix` 对短结果走零分配路径。
- `GetIndex` 在已知 `StringInt` map 时直接走 `values.get(key).copied().map(RuntimeVal::Int)`。
- `GetIndex` 的 string-key direct path 已扩展到 `StringMixed`、`StringInt`、`StringFloat`、`StringBool`，大幅减少 `TypedMap::get_str` 泛化路径。
- `GetIndex` 在静态 fact 确认为 typed list 且运行时 target/key 仍匹配时，会在 dispatch arm 前直接读取 list element，避开 slice-key 检查和 heap-index 泛化分派；不匹配时回落原 `get_index` 语义。
- `GetIndex` 在静态 fact 确认为 heap string 且 key 是整数时直接走 string index，不再落到 generic heap-index slow path；本轮普通 release 低样本中 `string_key_hash` 维持在约 `1.15x`。
- `set_map/set_list/set_object_index_handle` 增加 `has_static_fact`，让静态 fact 路径少做不必要的 shape 更新。

### Numeric / Move hot path

- `Move` dispatch 对非 heap scalar 源值不再查询 `register_copy` move policy；只有 `Obj` 源值才读取 move/clone fact。这保留 heap ownership 与 clone metrics 语义，同时减少 `gcd_batch`、`matrix_3x3_multiply`、map workload 中大量 scalar `Move` 的 fact lookup 开销。
- `MulInt` dispatch 与其它 `*Int` arithmetic opcode 对齐，只把 `Int * Int` 放在热 arm；混合 float fallback 仍走现有 dynamic numeric path，避免 int-heavy workload 在热路径匹配 float 分支。

### LLVM / native lowering

- `FunctionData` 在内存中的 `ModuleArtifact` 转换里保留 `Function.performance`，但序列化时跳过该字段。
- LLVM `GetIndex` / `SetIndex` 的 scalar block、straightline、callee eval、map mutate 路径统一使用 `native_known_string_key`。
- 这保证 compiler 的 key materialization elision 不会让 native lowering 退回动态 key 路径。
- AOT 全 workload 的 entry scalar facts 阻塞已推进：`"n${i}"` 这类静态字符串前缀 + 动态整数后缀的 string-int map key 现在可被 facts 接受，`GetIndex` / `SetIndex` 的 known-string-key 占位寄存器也能推断 `DynamicMap<String, Int>` 返回/写入 kind。
- AOT 全 workload executable 现在可以生成，并且 `bench/workloads_business_algorithms.lk` 的 15 个 workload checksum 已与 VM 全部一致。
- 本轮修正了 AOT string-key dynamic map 和 optional compare 的几个语义缺口：
  - dynamic string-int map `SetIndex` 对非 const string key 使用当前 key pointer 做 runtime prefix/number split，避免模板 key 的源寄存器在后续算术中被复用后读到陈旧 suffix。
  - dynamic text `len()` 对 `StrPtr` 部分改为 `strlen`，修复 `status=${status}` 这类模板长度。
  - `GetIndex` optional result 与 `nil` 比较时使用 `present.slot`，并让静态 collection compare 保留 `Maybe*` vs `Nil` 的 present 语义。
- 当前 AOT correctness 已过全 workload smoke；下一步应基于 AOT/VM/Lua 三方低样本 timing 判断 native hot path 优先级，尤其是 `two_sum_map`、`histogram_group_count`、`inventory_reorder` 仍可能因 runtime split/helper 调用偏慢。
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

- 当前阶段不新增 `ListFoldAdd`、`MapValuesFoldAdd`、`GetFieldK` 这类专门 opcode。
- 不用函数指针跳表替代 `match` 作为默认路线；indirect call 可能损失 inlining 和分支预测。
- 不盲目给所有 helper 加 `#[inline(always)]`。
- 不在非 LLVM VM 路径使用 `unsafe`。

## 验证命令

最近一次完整收口验证：

```bash
cargo fmt --all -- --check
cargo test -p lk-core --lib
cargo build --release -p lk-cli
RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
RUN_AOT=0 RUNS=6 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
for f in examples/syntax/*.lk; do perl -e 'alarm 30; exec @ARGV' target/release/lk "$f" >/tmp/lk_syntax.out || exit $?; done
target/release/lk compile exe bench/workloads_business_algorithms.lk --output /tmp/lk-workloads-aot-check
target/release/lk bench/workloads_business_algorithms.lk
/tmp/lk-workloads-aot-check
RUN_AOT=1 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh
```

结果：

- `cargo fmt --all -- --check`: pass
- `cargo test -p lk-core --lib`: `898 passed`
- `cargo build --release -p lk-cli`: pass
- `target/release/lk compile exe bench/workloads_business_algorithms.lk --output /tmp/lk-workloads-aot-check`: pass，生成 `/tmp/lk-workloads-aot-check`
- `target/release/lk bench/workloads_business_algorithms.lk` 与 `/tmp/lk-workloads-aot-check`: 15 个 workload checksum 全部一致
- release workload benchmark: checksum 全部一致；VM/Lua `RUNS=3` geomean `0.890x`，`RUNS=6` geomean `0.889x`
- release workload benchmark with AOT: checksum 全部一致；VM/Lua 低样本 geomean `0.964x`，AOT/Lua 低样本 geomean `0.450x`
- syntax smoke: `examples/syntax/*.lk` 全部通过，30 秒 alarm 未触发
- coverage/profile counters 未在本轮最终 release build 后重跑；保留上一节 profile 作为方向性历史证据。
