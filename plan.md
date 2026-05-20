# LK vs Lua 性能差距与 VM 优化计划

## 项目执行约束

以下约束来自当前项目的 `AGENTS.md`，本计划的实现过程必须同步遵守：

- 及时更新文档，包括 `README`、`docs/` 下文件和 `AGENTS.md`。
- 不遵循最小修改原则；如果看到小优化，可以直接一起改掉。
- 单个文件不能超过 1500 行。
- 越基础越优先，越重要越优先。
- 涉及很大改动时，先写入 `plan.md`，再一步步完成。
- 如果有算法上的优化，可以参考 `lua-5.5.0/src/` 下的实现（如果存在）。
- 当前项目还未发布，除非特别说明，不需要保持向前兼容；碍事的旧实现可以直接删除。
- 除了 LLVM 部分，不能使用 `unsafe`。
- `website/` 是 LK Lang 官网，内容应跟随
  `website/src/spec/LANG.md` 和 `website/src/spec/LANG_zh.md` 更新。
- 官网使用 Svelte/Vite/Bun，验证命令为 `cd website && bun run build`。

## 当前判断

差距大的核心不是“某一个 opcode 慢”，而是 LK 现在还是偏通用
VM/runtime，而 Lua 是多年打磨过的紧凑解释器。

从当前 benchmark 表看：

- 纯数字 AOT 已经很强：`gcd_batch`、`prime_trial_division`、
  `matrix_3x3_multiply`、`stock_max_profit`、`route_permission_check`
  的 AOT 都明显快过 Lua。
- VM 仍慢：这些同样负载在 LK VM 里还要付解释器 dispatch、动态
  `Val` 判断、寄存器读写、调用栈维护成本。
- AOT 在 map/list/string 负载上还没真正 native 化：
  `two_sum_map`、`sliding_window_sum`、`histogram_group_count`、
  `inventory_reorder`、`log_parse_filter` AOT 仍很慢，说明它们大概率
  还在频繁调用通用 runtime，而不是被降成紧凑的原生循环/哈希表操作。

当前代码和文档里也能对应上：`bench/README.md` 已列出主要瓶颈是
VM dispatch、整数比较/取模 dispatch、`Val` clone/refcount、字符串/key
构造、map/list 局部性；`plans/beat-lua-performance-hard-migration.md`
也把下一步方向定为 Performance IR、immediate-first `Val`、固定 frame
window、typed bytecode。

简单说：

1. Lua 的值、栈、表、解释循环是一体设计的，热路径非常短。
2. LK 的 `Val` 还承载很多高级语义，有 `Arc`、closure、named args、
   map/list/string 泛型路径，功能面宽但热路径重。
3. LK 的调用协议还没有完全消掉成本。虽然已有 `RegisterSpan`、
   packed call、inline 等优化，但真实 workload 里还有不少小函数和
   builtin/method 调用。
4. AOT 目前只证明了“数值计算可赢”，还没把 map/list/string 降到足够
   低层。现在慢的 AOT 行基本都是这类。
5. Lua 的 table/string 路径很成熟。LK 现在的 map/list 不是 Lua 那种
   高度特化的单表结构，语义更分离，但也意味着要额外设计专用 hot
   path。

所以接下来最值得做的不是继续零碎补 opcode，而是按优先级打这几块：

- 先让 AOT 的 map/list/string 不再走重 runtime。
- 再压 VM 的 `Val` clone/refcount 和 call frame 成本。
- 最后用 Performance IR 把 benchmark hot path 全部降成 typed
  ops/native lowering。

当前最醒目的结论：LK 已经在纯数值 AOT 上具备赢 Lua 的能力；真正拖后腿
的是容器、字符串、哈希 key、函数调用和通用动态值路径。

## 通用 VM 优化方向

通用 VM 性能优化不要从“再加几个 opcode”开始，应该从降低每条业务语句的
固定成本开始。当前 LK 的大头是 dispatch、`Val` 动态分派/clone、函数调用、
map/list/string runtime 路径。

### 1. 先加 VM 级 profiler

先别盲改。给 VM 加低开销计数器，统计：

- opcode 执行次数
- 每类 opcode 总耗时或采样耗时
- `Val::clone` 次数，尤其是 `Arc` clone
- 函数调用次数：普通 call、exact call、native call、method call、
  named call
- map/list/string 操作次数
- fallback 次数：typed op fallback、BC32 fallback、AOT runtime fallback

输出按 workload 聚合，例如：

```text
workload=two_sum_map
op.MapSet=200000
op.StrConcatKnownCap=400000
val.clone.arc=900000
call.native=100000
fallback.generic_add=0
```

没有这个，优化容易变成猜。

### 2. 把 hot path 全部 typed 化

现在 VM 已有 typed/fused opcode，但还不够系统。目标是让 benchmark 里的
核心循环尽量不走通用 `Val` 运算。

优先补这些族：

```text
AddI/SubI/MulI/ModI
CmpI + fused branch
MapGetConstStr / MapSetConstStr / MapHasConstStr
MapGetStrReg / MapSetStrReg
ListGetI / ListSetI / ListPushI
StrLen / StrStartsWithConst / StrContainsConst
```

关键不是 opcode 名字，而是 compiler 必须稳定降到这些 opcode。否则 opcode
存在但真实 workload 不用，没意义。

### 3. 做 Performance IR，而不是直接从 AST 猜

需要在 bytecode 前加一层轻量 IR，记录这些事实：

- 局部变量类型：int、float、bool、string、list、map
- 变量是否 escape
- loop induction variable
- const string key
- exact call target
- builtin intrinsic target

然后由 IR 决定降成 typed bytecode 或 fallback generic bytecode。

这能解决现在最大的问题：编译器局部知道一点类型，但跨函数、跨循环、跨
map/list 操作的信息不够稳定。

### 4. 改 `Val` 热路径所有权模型

当前 `Val` 虽然已有 short string 和一些优化，但容器/字符串仍容易触发
`Arc` clone。通用 VM 要快，`Val` 需要做到：

- `Int`、`Float`、`Bool`、`Nil`、`ShortStr` copy-cheap
- 热路径寄存器赋值尽量 move，不 clone
- map/list 写入时避免不必要 clone
- 常量 key intern/cached hash
- `Arc` 不进入数值循环和短生命周期临时值路径

建议加测试或计数 gate：

```text
sliding_window_sum: val.clone.arc must decrease
histogram_group_count: string key allocation must decrease
inventory_reorder: map/list mutation clone must decrease
```

### 5. 函数调用改成固定 frame window

真实 workload 里小函数很多，Lua 很强的一点是 call path 足够紧。LK 需要把
普通调用压到：

- caller 已经把参数放在连续 register window
- callee 直接读 window
- 返回值写回固定 return slot
- exact arity call site 缓存 callee frame layout
- 小纯函数在 bytecode 前 inline

当前已有 `RegisterSpan` 方向，下一步是让 benchmark 里的小函数稳定走
exact/inlined path，而不是掉回通用 call。

### 6. 容器不要继续走通用 HashMap 语义热路径

map/list/string 是现在 AOT 和 VM 都被拖住的核心。需要专门的 hot layout：

- const string key：预计算 key id/hash
- map get/set const key 不重新构造字符串
- list push/index 不 clone 整个 list
- group-count 场景提供 int counter update fast path
- template string key 构造走 known capacity + short/int format fast path

例如 `histogram_group_count` 不应该是：

```text
build string key -> hash -> map get Val -> clone -> add -> map set Val
```

而应该接近：

```text
key id/hash -> find bucket -> mutate int counter
```

### 7. Dispatch 层做 packed hot loop

已有 BC32/packed fast path，就继续扩大覆盖面。目标是：

- benchmark hot function 必须有 `code32`
- hot loop 不频繁 decode enum `Op`
- fused branch/op 覆盖 while-loop 常见形态
- fallback 次数可统计，不能静默变慢

这一步主要降低 VM 固定成本，对 `gcd_batch`、`binary_search`、
`stock_max_profit` 这类有效。

### 8. AOT 和 VM 共用同一套 typed IR

不要让 VM 一套优化、AOT 另一套优化。正确结构是：

```text
AST
 -> typed/performance IR
 -> bytecode/BC32
 -> VM runner

AST
 -> typed/performance IR
 -> LLVM/AOT lowering
```

这样 `MapGetConstStr`、`StrStartsWithConst`、`ListPushI` 等能力不会只在 VM
或 AOT 一边存在。

## VM Call Frame 和 Typed Dispatch 改造

这里要改两条线：call frame 让“已知小函数调用”稳定走零分配 fast path；
typed branch/op dispatch 让热循环少走通用 `Val`/`Op` 路径。当前代码已经有
基础，但命中率和热路径纯度还不够。

### VM call frame

当前入口在 `core/src/vm/compiler/builder.rs` 的 `emit_positional_call`：
已知 closure 会发 `CallClosureExact`，native 会发 `CallNativeFast`。runtime
fast path 在 `core/src/vm/vm/runtime/frame/run/packed/call.rs`，已经用
`RegisterSpan` 传参，但冷路径里还有 closure 匹配、`code.get_or_init`、
frame info、captures、IC 初始化、fallback 检查等成本。

#### 1. 把 exact call 从运行时识别前移到编译期固定

现在 `emit_positional_call` 只根据 `known_callee` 选择 `CallClosureExact`。
下一步要让本地函数、无 named 参数、固定 arity 的 call site 编译成更强的
形式，例如：

```rust
Op::CallClosureStatic {
    callee: FunctionId,
    base,
    argc,
    retc,
    frame_layout: FrameLayoutId,
}
```

不一定马上新增完整 `FunctionId`，也可以先扩展 `CallClosureExact` 的 IC
初始化，让第一次执行前就能拿到 `fun_ptr`、`frame_info`、`tiny_plan`，避免
第一次调用走大冷路径。

#### 2. 把 hot call 和 cold call 分成两个函数

现在 `run_call_packed` 同时处理 IC hit、native、closure cold、named fallback，
函数很大。应该拆成：

```text
run_call_closure_exact_hot(...)
run_call_native_fast_hot(...)
run_call_cold(...)
```

hot 函数只做：

- 校验 closure 指针或 function 指针
- 构造 `RegisterSpan`
- 调 `invoke_vm_closure_fast`
- 写回 return slot
- 更新 `pc`

不要在 hot 函数里做 `NativeCallable::from_val`、named 参数判断、`anyhow!`
构造、`code.get_or_init`。

#### 3. 缓存 frame layout，不在每次 call 取 closure 元数据

IC 里现在已经有 `frame_info`、`cache`、`tiny`。继续把这些稳定信息往 IC
里放：

- `fun_ptr`
- `frame_info`
- `capture_specs`
- `return_layout`
- `param_count`
- `tiny_plan`

目标是 IC hit 后不再碰 `closure.params`、`closure.named_params`、
`closure.code.get_or_init()`。

#### 4. 小纯函数优先 inline

对 `binary_search`、`cart_pricing_rules`、`fraud_rule_scoring` 这类 workload，
最便宜的 call frame 是没有 call frame。已有 inline 测试方向，可以把规则
扩大到：

- 单 return 表达式
- 无 named/default
- 无 side effect
- 参数只读
- 返回 int/bool/string 简单表达式

#### 5. known builtin 不走 call

`math.floor`、`starts_with`、`len`、`map.get/set/has`、`list.push` 不应该是
函数调用。compiler 看到这些固定 callee 时直接发 intrinsic op，例如
`FloorI`、`StrStartsWithConst`、`MapGetConstStr`。

### Typed branch/op dispatch

当前 opcode 里已有 `CmpLtImmJmp`、`CmpLeImmJmp`、`CmpNeImmJmp`、
`AddIntImmJmp`。这是对的，但覆盖太窄。

#### 1. 扩展 typed compare + branch

现在主要是 register vs immediate。需要补全 register vs register：

```text
CmpLtIntJmp r1, r2, ofs
CmpLeIntJmp r1, r2, ofs
CmpGtIntJmp r1, r2, ofs
CmpGeIntJmp r1, r2, ofs
CmpEqIntJmp r1, r2, ofs
CmpNeIntJmp r1, r2, ofs
```

这样 `while i < n`、`if price > threshold` 不需要先产出 `Val::Bool` 再
`JmpFalse`。

#### 2. 让 peephole 稳定融合

当前 builder 最后跑 `peephole_fuse_cmp_jmp`。下一步要覆盖这些模式：

```text
CmpInt/CmpImm + JmpFalse -> Cmp*Jmp
AddIntImm + Jmp -> AddIntImmJmp
ToBool + JmpFalse -> typed branch when source fact known
LoadK int + Cmp -> CmpImmJmp
```

#### 3. compiler facts 要跨 basic block 保守传播

只在单条 op 更新 int fact 不够。需要至少保证：

- `i = 0` 后 `i` 是 int
- `i = i + 1` 后 `i` 仍是 int
- `while i < n` 内 `i`/`n` fact 不丢
- 函数参数如果从 direct call 推断为 int，callee 内可用

#### 4. BC32 必须同步支持

只加 `Op` 不够。`Function` 有 `code32`，如果新 typed op 不能编码，hot
function 会掉出 packed path。每加一个 typed branch/op，都要同步：

- `bytecode.rs` opcode
- BC32 encoder/decoder
- packed decode hot slot
- packed hot_exec 执行

#### 5. packed hot_exec 里执行 typed op

`core/src/vm/vm/runtime/frame/run/packed/hot_exec.rs` 是热执行点。新 typed op
在这里应该是很短的分支：

```rust
match (&regs[a], &regs[b]) {
    (Val::Int(x), Val::Int(y)) => {
        pc = if x < y { pc + 1 } else { jump_pc };
    }
    _ => fallback_or_error,
}
```

对 benchmark hot path，fallback 计数应该接近 0。

### 最小落地批次

1. 给 `CallClosureExact` 做更强 IC：第一次后 hot path 不再读取 closure
   metadata。
2. 拆 `run_call_packed` 的 closure exact hot/cold 路径。
3. 补 `Cmp*IntJmp` register-register 分支。
4. 扩 `peephole_fuse_cmp_jmp`，让 `while i < n` 稳定发 typed branch。
5. 给这些 op 补 BC32 + packed hot_exec。
6. 跑：

```bash
cargo test -p lk-core
cargo build --release -p lk-cli
RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
```

优先看 `binary_search`、`gcd_batch`、`stock_max_profit`、
`cart_pricing_rules`、`fraud_rule_scoring`。如果这些不动，说明 call/branch
还没真正命中 hot path。

## 实施进度

### 2026-05-20

- 已扩展现有 `VmRuntimeMetrics`，让 runtime metrics 在 release 构建中也能
  通过 `LK_VM_PROFILE=1` 启用。
- 已接入普通 CLI 执行路径，执行 `.lk` 或 `.lkb` 时会在 stderr 输出
  `VM profile:` 汇总。
- 当前 profile 已覆盖：
  - opcode steps
  - branch / typed branch
  - call / native call / closure call / exact call / named call / method call
  - container / list / map / string 操作
  - BC32 packed hot-slot fallback
  - `Val` clone、register write、return move
  - quickening hit/build/miss/deopt/sentinel skip
- 已验证 debug 与 release 普通执行都能输出非零指标。
- 已给 `bench/workloads_business_algorithms.lk` 增加 `LK_WORKLOAD_FILTER`，
  可以单独运行某一个 workload 并收集对应 `VM profile:`。
- 已给 `bench/run_workload_bench.sh` 增加 `PROFILE_WORKLOADS=1` 诊断模式。
  该模式会在正常 timing 表之后额外打印按 workload 分组的 opcode、call、
  branch、typed branch、container、list/map/string、clone 指标。
- 已修复 bench runner 在 `EXTRA_RUNS=0` 时仍因为 `seq 1 0` 产生额外样本的
  边界问题。
- 已给 opcode 与 packed 两条 `CallClosureExact` 路径增加 IC-hit 直达 hot
  path。缓存命中后先使用 `fun_ptr`、`frame_info`、`ClosureFastCache` 和
  `TinyCallPlan` 调用，避免 exact call 每次先读取 closure 参数元数据再落回
  通用 call 分派。
- 已把 opcode 与 packed 两条 `CallClosureExact` 的 IC-miss/cold path 也拆成
  exact-closure 专用入口。首次调用会直接校验 exact arity、编译/读取
  `Function`、生成 `TinyCallPlan`、填充 `CallIc::ClosurePositional`，并在
  tiny plan 可执行时跳过完整 VM frame 调用；不再先回落到通用 `Call`
  路径做 native/closure 泛型分派和闭包 `Val` clone。
- 已把 opcode 与 packed 的 `CallExact` 也接到同一套专用路径：
  closure exact 直接复用 exact-closure IC/cold path，native exact 直接调用
  `invoke_native_callable_with_ic`，不再校验后回到 `run_call_*` 的 generic
  分派。
- 已把 opcode 与 packed hot slot 的 branch/call/container metric 记录接到外层
  `collect_metrics` 开关，避免 profiler 关闭时在热路径反复进入记录函数。
- 已实现 register-register typed branch 的静态 `CmpIntJmp` 链路：
  - `Op::CmpIntJmp { kind, a, b, ofs }`
  - `CmpI + JmpFalse/BoolBranch` peephole 融合
  - opcode fallback 执行
  - BC32 extension encode/decode 与 packed hot decode
  - LKB round-trip
  - compiler、peephole、BC32 测试覆盖
- 已新增 `FloorDivImm { dst, src, imm }` typed op，用于
  `math.floor(int_or_float_expr / const_int)`。编译器现在会把
  `math.floor((lo + hi) / 2)` 降成 `AddInt + FloorDivImm`，减少
  `binary_search` 热循环内的 `DivFloat + Floor` 两段 dispatch；该 op 已同步
  opcode、BC32 encode/decode、packed hot/cold path、LKB round-trip、AOT
  lowering 和测试覆盖。
- 已把 `FloorDivImm` 的 AOT lowering 从通用 helper 调整为 known-int
  直降 LLVM `sdiv/srem` floor-div 序列。helper 只保留给非 int fallback；
  这避免 `binary_search` AOT 因每轮 midpoint 计算调用 runtime helper 而从
  约 13ms 退化到约 42ms。
- `binary_search` release profile 已确认 `binary_search_implicit` 使用
  `FloorDivImm=1`，AOT entry 仍为 `native-lowerable`。本轮 profile 中
  opcode steps 从此前约 1308 万下降到约 1176 万，register writes 从约
  756 万下降到约 624 万。
- 已给 runtime profile 增加 BC32 fallback reason counters：
  `bc32_build_misses`、`bc32_stale_slots`、`bc32_stale_misses`、
  `bc32_sentinel_skips`，并把 `PROFILE_WORKLOADS=1` 表格扩展到显示
  `Bc32Miss` / `Bc32Sent`。
- 已给 LLVM/AOT backend 补齐 fused typed branch lowering：
  `CmpIntJmp`、`CmpLeImmJmp`、`CmpNeImmJmp` 现在都会生成 LLVM branch，
  不再导致 native entry fallback。`LK_WORKLOAD_FILTER=histogram_group_count
  target/release/lk coverage --runtime bench/workloads_business_algorithms.lk`
  已从 `AOT entry: fallback` 恢复为 `AOT entry: native-lowerable`。
- 已给 AOT 增加 const string key map get helper：
  `MapGetInterned` 现在降低到 `lk_rt_map_get_const_str(base, ptr, len)`，
  避免经过通用 `lk_rt_access(base, key_handle)` 和 key handle lookup。这是
  map/list/string runtime native 化的第一步。
- 已给 AOT 增加 const string key map has helper：
  `MapHasK` 现在降低到 `lk_rt_map_has_const_str(base, ptr, len)`，避免
  const-key membership test 先 intern/load key handle 再走通用 `lk_rt_map_has`。
  后续同类方向是 string-int dynamic key 的专用读写 helper。
- 已给 AOT 增加 const string key map set helper：
  `MapSetInterned` 现在降低到 `lk_rt_map_set_const_str(base, ptr, len, value)`，
  避免 const-key mutation 先构造 key handle 再走通用 `lk_rt_map_set`。
- 已给 AOT 接通 string-int dynamic key 的 map has helper：
  当 `MapHas` 的 key 来自 `"prefix" + int` 这种 deferred key fact 时，直接
  降低到 `lk_rt_map_has_str_int(base, ptr, len, suffix)`，避免先调用
  `lk_rt_str_int_key` materialize key handle。
- 已给 AOT 接通 string-int dynamic key 的 map get helper：
  `MapGetDynamic` 现在会把同类 deferred key fact 直接降低到
  `lk_rt_map_get_str_int(base, ptr, len, suffix)`。同时修正 string-int key
  延迟扫描，让 `MapGetDynamic` 能作为消费端保留 key fact，避免退回
  `lk_rt_access_str_int` 的通用 access 路径。
- 已给 AOT 扩展 dynamic known string key 的 map helper：
  当 `MapGetDynamic`、`MapHas`、`MapSet` 的 key register 已知是字符串常量时，
  现在会分别降低到 `lk_rt_map_get_const_str`、`lk_rt_map_has_const_str`、
  `lk_rt_map_set_const_str`。这覆盖了 key 先绑定到局部变量、再传给 map
  get/has/set 的场景，避免走 key handle 与通用 map/access helper。
- 已给 AOT 增加 const-string map get feeding add 的 deferred helper：
  `MapGetInterned` 或 dynamic known string key 的 `MapGetDynamic` 如果结果只
  喂给后续 `Add`，现在会保留 `AccessedConstStr` fact，并降低到
  `lk_rt_add_map_get_const_str(lhs, base, ptr, len)`。这减少固定 key counter
  场景里一次中间 map-get helper 和通用 `Val` 加法路径，是后续
  `map.get + add + map.set` counter update fast path 的前置步骤。
- 已把上述 deferred add 继续接到 const-string map set：
  当 `map.get(m, "k") + x` 只用于 `map.set(m, "k", ...)` 时，AOT 现在会
  直接降低到 `lk_rt_map_set_add_map_get_const_str(m, ptr, len, x)`，不再生成
  中间 add 结果或普通 const-string set helper。这覆盖固定 key counter
  update 的单 helper 形态；下一步仍需要扩展到 string-int dynamic key 和
  `if prev == nil { set 1 } else { set prev + 1 }` 的跨分支形态。
- 已把同类 counter update 扩到 string-int dynamic key：
  对 `"prefix" + int` 形成的 `MapGetDynamic -> Add -> MapSet`，AOT 会保留
  `AccessedStrInt` / `AddMapGetStrInt` fact，并降低到
  `lk_rt_map_set_add_map_get_str_int(m, ptr, len, suffix, x)`。这覆盖
  `histogram_group_count`、`inventory_reorder` 中常见的动态字符串整数 key
  单分支 update 形态；跨分支 nil 初始化仍未合并。
- 已优化 AOT counter update helper 的 int hot path：
  `lk_rt_map_set_add_map_get_const_str` 和
  `lk_rt_map_set_add_map_get_str_int` 现在在 `lhs` 与 map 旧值都是 int 时
  直接写回 `Val::Int(left + right)`，不再先 decode 成通用 `Val` 再调用
  `BinOp::Add.eval_vals`。这是 helper 内部的固定成本下降；`histogram` 这类
  跨分支 `nil -> init / else -> add` 仍需要更高层的分支模式融合。
- 已实现 AOT 跨分支 counter update 融合：
  LLVM translator 现在识别
  `MapGetDynamic/MapGetInterned -> CmpEq nil -> BoolBranch -> init MapSet
  -> Jmp -> Add/AddIntImm -> MapSet`，并降低到
  `lk_rt_map_update_int_const_str` 或 `lk_rt_map_update_int_str_int`。原始
  init/else 分支 block 会重定向到 join block，避免生成两条 map set 路径。
- 已把 `StrConcatToStr("prefix", int)` 接入 string-int deferred key fact。
  这让 `histogram_group_count`、`inventory_reorder` 这类模板字符串整数 key
  能命中新跨分支 update helper，而不是先 materialize string key 再走
  `map.get` / `map.set` helper。
- 已用整套 workload 的未优化 LLVM IR 验证新 helper 命中：
  `bench/workloads_business_algorithms.ll` 中出现
  `lk_rt_map_update_int_str_int`，覆盖 `"b${bucket}"` / `"sku-${i}"`
  这类动态 key counter update。
- 已把 AOT `Access -> AddInt/SubInt` 接入已有 access-binop helper：
  `access_result_can_defer` 现在识别 typed `AddInt` / `SubInt` 消费端，
  让 `sliding_window_sum` 中的 `rolling += values[i]` 和
  `rolling -= values[i - window_size]` 降到 `lk_rt_add_access` /
  `lk_rt_sub_access`，不再先 materialize standalone `lk_rt_access` 结果。
- 已把 `StrConcatToStr` / `StrConcatKnownCap` 接入 AOT string-length fact
  链。模板字符串如果后续只用于 `len`、`to_iter` 或 `ch.len()` 这类长度消费，
  现在可以只保留长度表达式和 `lk_rt_int_decimal_len`，避免 materialize
  字符串、`to_string`、`to_iter` 和 `index_len`。
- 已把 AOT `ListPush` 接入 string-int key 专用 helper：
  `KnownReg::StringIntKey` 作为 `list.push()` 参数时直接调用
  `lk_rt_list_push_str_int`，不再先通过 `lk_rt_str_int_key` materialize
  临时字符串 handle。该路径覆盖 `inventory_reorder` 里的
  `reorder.push("sku-${i}")` 形态。
- 已把 AOT `== nil` / `!= nil` 比较降成直接 LLVM `icmp`。`nil` 是固定
  immediate sentinel，因此 `map.get(...) != nil` 不需要再调用通用
  `lk_rt_cmp`。这覆盖 `two_sum_map`、`histogram_group_count`、
  `inventory_reorder`、`fraud_rule_scoring` 里的高频判空。
- 已添加 `lk_rt_mul_access` 与 AOT `Access -> Mul/MulInt` deferral，并把
  无关 `Access/MapGet` 视作 deferral 扫描中性操作。当前整套 workload
  的未优化 IR 已出现 `lk_rt_mul_access`，覆盖 `cart_line_total` 的
  `price * qty` 形态；但 specialized/inlined tax-rate 乘法链仍未完全融合。
- 已把 AOT const-string / string-int key 的 `map.get` feeding `Mul` 接入
  `lk_rt_mul_map_get_const_str` 和 `lk_rt_mul_map_get_str_int`。这覆盖
  `cart_pricing_rules` 中 `tax access -> subtotal * tax` 的更专用链路，避免
  先生成 `lk_rt_map_get_const_str` / `lk_rt_map_get_str_int` 中间值再调用
  通用 `lk_rt_mul`。
- 已修正 AOT typed binary fallback 的 int fact 传播：`AddInt/SubInt/MulInt`
  如果因为 access/map-get deferral 走 runtime helper，translator 现在仍会把
  结果标记为 `KnownReg::Int`。同时 `FloorDivImm` 结果也进入
  `infer_integer_registers`。新增 IR 测试确认
  `map.get const-string -> MulInt -> FloorDivImm` 会生成 LLVM `sdiv/srem`，
  不再调用 `lk_rt_floor_div_imm`。
- 已把 `CallIc::ClosurePositional` 扩展为缓存 closure `captures` 与
  `capture_specs`。opcode 与 packed 两条 closure fast path 在 IC 命中且
  closure 指针一致时不再每次调用 `frame_captures()` 读取 closure 元数据；
  same-prototype 不同 closure 的复用仍保留安全 fallback，会重新读取当前
  closure 的 captures。
- 已修正 generic `Call` 的 closure fast path：之前虽然会使用 IC 内缓存的
  `captures` / `capture_specs`，但在进入 cached 分支前仍无条件调用
  `closure.frame_captures()`。现在 opcode 与 packed 两条路径都只在 IC
  miss/cold 分支读取 closure 元数据，普通 `Call` 命中 fast path 时也满足
  “不在每次 call 取 closure metadata”。
- 已把 `CallExact` 的 closure 分支接入同一套 closure IC hot probe。opcode
  与 packed 的 `CallExact` 现在都会先尝试命中已有 `CallIc::ClosurePositional`
  缓存，命中后直接复用 `fun_ptr`、`frame_info`、`captures` 和
  `ClosureFastCache`；miss 时才进入 native/closure/error 的原有分派。
- 已把 `CmpNeImm + JmpFalse/BoolBranch` 接入静态 peephole 融合，生成
  `CmpNeImmJmp`。该 fused op 已同步 BC32 extension pack/decode、typed gate、
  packed hot decode 和测试覆盖，避免 `while x != imm` 形态因为静态融合而掉出
  packed fast path。
- 已把 `CmpGtImm + JmpFalse` 和 `CmpGeImm + JmpFalse` 接入静态 peephole
  融合，新增 `CmpGtImmJmp`、`CmpGeImmJmp`。两者已同步 opcode runtime、
  packed cold/hot decode、BC32 extension pack/decode、LKB round-trip、LLVM/AOT
  lowering 和测试覆盖。`cart_pricing_rules` 的 `qty >= 5` guard 现在能直接
  生成 `CmpGeImmJmp`，少一次 bool 临时寄存器写和一次分支 dispatch。
- 已把 `CmpEqImm + JmpFalse/BoolBranch` 接入静态 peephole 融合，新增
  `CmpEqImmJmp`。该 fused op 已同步 opcode runtime、BC32 extension
  pack/decode、packed hot decode、LKB round-trip、LLVM/AOT lowering、
  分析/统计命名和测试覆盖，补齐 `== imm` immediate guard 的 typed branch
  静态覆盖面。
- 已新增 `ListPushMove { list, val }`，让 `list.push(<临时表达式>)` 消费临时
  值寄存器而不是 clone 后推入 list。compiler 只在参数表达式是临时值时发
  move 版本，变量参数仍保留普通 `ListPush`，避免破坏后续变量读取。该 op 已
  同步 opcode runtime、BC32 flag encode/decode、packed hot/cold path、LKB
  round-trip、AOT lowering 兼容、分析命名和测试覆盖，主要针对
  `inventory_reorder` 的 `reorder.push("sku-${i}")` 这类 string-int 临时值
  push 场景减少 heap clone。
- 已新增 `MapSetInternedMove(map, kidx, val)`，让 `map.set("const",
  <临时表达式>)` 消费临时 value 寄存器而不是 clone 后写入 map。compiler
  只在 literal key 且 value 是临时表达式时发 move 版本，变量 value 仍保留
  `MapSetInterned`。该 op 已同步 opcode runtime、BC32 extension
  encode/decode、packed hot/cold path、LKB round-trip、AOT lowering 兼容、
  analysis/stats 命名和测试覆盖，是 map/list/string hot path 中“临时值写入
  不 clone”的继续补齐。

当前最小诊断样本显示：

- `binary_search` 的主要压力仍是 opcode dispatch 与 typed branch：
  约 1176 万 opcode、564 万 branch、384 万 typed branch。
- `log_parse_filter` 的主要压力是容器/字符串/heap clone：
  约 151 万 container op、103 万 string op、43.8 万 heap clone。
- `two_sum_map`、`histogram_group_count`、`inventory_reorder` 的 map/string
  操作量很高，适合作为 map const-key/hash-cache 与 mutation fast path
  的优先验收 workload。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.807x`，AOT/Lua `0.307x`，AOT/VM `0.081x`。该表是每轮方向检查，
  不是替代 quiet-machine baseline。
- 本轮 AOT 单项变化最明显的是 map/string workload：
  `two_sum_map` AOT `48.209ms`、`histogram_group_count` AOT `63.964ms`、
  `inventory_reorder` AOT `47.676ms`。它们仍未全部赢 Lua，但已不再是
  160-220ms 级别的重 runtime fallback。
- typed access-binop deferral 后，`sliding_window_sum` AOT 从上一轮
  `75.762ms` 降到 `53.304ms`，说明 list access + int add/sub 的 fused
  helper 已命中。
- string length fact 穿过模板字符串链后，`string_key_hash` AOT 降到
  `1.007ms`，`log_parse_filter` AOT 降到 `43.797ms`。这是因为相关热路径
  已从字符串构造/迭代 helper 变成整数长度计算。
- list push string-int helper 命中后，`inventory_reorder` AOT 从上一轮
  `60.274ms` 降到 `58.492ms`；幅度不大，说明剩余瓶颈更多在 map update、
  list join/string materialization 和 VM 侧 dispatch，而不只是 push 参数
  materialization。
- nil compare 直接 lowering 命中后，本轮 map 判空类 workload 有明显下降：
  `two_sum_map` AOT `58.836ms -> 48.907ms`，`histogram_group_count`
  `75.044ms -> 67.104ms`，`inventory_reorder` `58.492ms -> 47.220ms`，
  `fraud_rule_scoring` `23.793ms -> 20.180ms`。
- `mul_access` 在重新构建 release 后已命中未优化 workload IR。
  本轮继续补齐 typed binary fallback 的 int fact 传播后，
  `cart_pricing_rules` AOT 为 `3.098ms`，相比上一轮 quick run `3.635ms`
  继续下降；`sliding_window_sum` AOT 也从 `52.086ms` 降到 `32.446ms`。
  这是因为 fused access/map-get arithmetic helper 的结果现在能继续穿过
  `FloorDivImm` known-int 判定，减少通用 helper 与 handle 编码解码。
- closure IC 缓存 captures 后，本轮 VM 几何均值从上一轮 quick run
  `3.300x` 到 `3.382x`，没有稳定显示几何收益，且受单样本噪声影响不能
  直接归因。更明确的是 `CallClosureExact`、generic `Call` 和 `CallExact`
  的 cached closure fast path 都已满足计划中的“不在每次 call 取 closure
  metadata”要求；后续仍要继续拆小 `run_call_*` hot/cold 路径并减少普通
  `Call` 与 exact call 的重复实现。
- `CmpNeImmJmp` 静态融合本轮没有改善 VM 几何均值，说明当前 business
  workload 主瓶颈不在 `x != imm` 分支形态；它仍补齐了 typed branch/op
  dispatch 的覆盖面和 BC32 一致性。
- `CmpGtImmJmp` / `CmpGeImmJmp` 静态融合后，本轮 VM 几何均值从上一轮
  `3.456x` 到 `3.379x`；`cart_pricing_rules` VM `8.457ms -> 8.426ms`，
  `two_sum_map` VM `67.762ms -> 65.878ms`。单样本只能说明方向，稳定收益
  需要后续 quiet-machine baseline；不过该变更补齐了 `>` / `>=` immediate
  guard 的 typed branch 覆盖面。
- 本轮将 opcode 与 packed dispatch 复制的 exact closure call-frame 辅助逻辑
  抽到 `run/call_common.rs`：`prepare_exact_closure_call`、
  `run_prepared_exact_closure_call` 和 closure IC hot probe 现在只有一份实现。
  `RUNS=1 EXTRA_RUNS=0` quick run 显示 VM/Lua 几何均值 `3.371x`，
  相比上一轮 `3.379x` 基本持平；该轮主要减少重复实现和后续维护风险，
  不是稳定性能收益声明。
- 本轮继续把普通 `Call` 的 positional closure fast path 抽进
  `run/call_common.rs`，opcode 与 packed dispatch 现在共享
  `try_run_positional_closure_call`。`run_call_*` 中剩余的重复主体主要是
  named/default slow path；本轮 quick run VM/Lua 几何均值为 `3.408x`，
  单样本略差于上一轮 `3.371x`，按噪声处理，不作为性能回退结论。
- 本轮继续把普通 `Call` 的 named/default slow path 抽进
  `run/call_common.rs`，opcode 与 packed dispatch 现在共享
  `run_closure_slow_call`。`run_call_opcode` / `run_call_packed` 已基本收敛为
  分派外壳，call frame 逻辑集中在 `call_common`。本轮 quick run VM/Lua
  几何均值 `3.431x`，主要记录当前状态；由于是单样本，不把它作为稳定
  性能回退结论。
- `CmpEqImmJmp` 静态融合后，本轮 quick run VM/Lua 几何均值为 `3.453x`，
  AOT/Lua 几何均值为 `0.315x`，AOT/VM 几何均值为 `0.091x`。该轮继续按
  单样本趋势记录，不作为稳定性能回退结论；主要价值是让 `== imm` 分支形态
  与 `< <= > >= !=` immediate fused branch 使用同一套 BC32/packed/AOT
  覆盖面。
- `ListPushMove` 落地后，本轮 quick run VM/Lua 几何均值为 `3.979x`，
  AOT/Lua 几何均值为 `0.321x`，AOT/VM 几何均值为 `0.080x`。这轮 VM 单样本
  明显慢于上一轮，按机器状态/噪声记录，不把它归因为稳定回退；该变更的可证
  收益点是临时值 list push 不再做 `Val` clone，后续需要用
  `PROFILE_WORKLOADS=1` 对 `inventory_reorder` 的 `heap_clones` 做更直接的
  多样本确认。
- `MapSetInternedMove` 落地后，本轮 quick run VM/Lua 几何均值为 `3.807x`，
  AOT/Lua 几何均值为 `0.307x`，AOT/VM 几何均值为 `0.081x`。相比上一轮
  `3.979x` 有单样本改善，但仍按方向记录处理；该轮的主要实现价值是继续
  降低 const-key map 临时 value 写入的 clone 成本，而不是声明稳定性能收益。
- 已修正 packed hot-slot 的 typed branch profile 计数：`CmpImmJmp`、
  `CmpLtImmJmp`、`CmpLeImmJmp`、`AddIntImmJmp` 现在和 opcode 路径一样记录
  `typed_branch_ops`。此前 `coverage --runtime` 已显示 `gcd` 编译出了
  `CmpNeImmJmp`，但 `LK_VM_PROFILE=1 LK_WORKLOAD_FILTER=gcd_batch` 显示
  `typed_branches=0`，实际是 packed hot_exec 漏记指标，不是 typed branch
  没命中。
- 已新增 packed immediate compare branch 的 runtime metrics 单测，显式把
  `CmpGtImm + JmpFalse` 函数打包成 BC32 后执行，并断言 `typed_branch_ops`
  非零，防止 packed typed branch 后续再次漏记。
- 修复后 release profile 已确认 `gcd_batch` 的 `typed_branches=737372`，
  `branches=1394759`，`quickening_hits=4166845`，说明 packed typed branch
  统计已经能反映真实热路径。该轮是 profiler 正确性修复，不是性能优化。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `4.089x`，AOT/Lua `0.325x`，AOT/VM `0.079x`。因为本轮只修 profile
  计数且单样本波动明显，不把 VM/Lua 变差归因为稳定性能回退。
- 已放开 straight-line known call inline 的无调用 `let` 前缀。此前
  `try_inline_simple_known_call` 已能识别 block 末尾 `return expr`，但只要
  helper body 里有 prefix `let` 就直接放弃 inline；现在会在临时 scope 中
  依次编译这些已校验为无调用、只依赖参数/前缀局部的 `let`，再编译最终
  return 表达式。
- 该变更命中 `order_score_pipeline` 的 `score_order(price, qty, discount)`：
  release profile 显示该 workload 的总 `calls=21`、`closure_calls=16`，
  90000 次 `score_order` 调用已从 hot loop 中消失；单项 profile 耗时约
  `12.689ms`。coverage 也显示总 call sites 从上一轮 `73` 降到 `72`，
  `CallClosureExact` 从 `39` 降到 `38`。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.750x`，AOT/Lua `0.267x`，AOT/VM `0.072x`。其中
  `order_score_pipeline` 从上一轮 quick table 的 VM `23.927ms` / AOT
  `1.927ms` 降到 VM `14.511ms` / AOT `0.223ms`；这是小纯函数 inline 的直接
  收益，但仍需要多样本 quiet-machine baseline 固化。
- 本轮先扩大 `CmpLt/Le/Gt/GeImm + BoolBranch` 静态融合覆盖面，但
  `fraud_rule_scoring` 的热分支是宽 immediate `amount > 900/400`，超出当前
  小 immediate fused opcode 范围；profile 未显示 workload 级收益，因此只作为
  typed branch/op dispatch 覆盖补齐记录。
- 本轮新增 `MapGetDynamic/MapGetInterned + != nil + branch` 到
  `MapHas/MapHasK + branch` 的 peephole，并补上第二个 RK-remap 后处理 pass。
  真实源码测试暴露 `map.get(data, key) != nil` 会先变成
  `MapGetDynamic; CmpNe(..., kNil); BoolBranch`，现在能折成 presence-only
  opcode，避免只判断存在性时克隆 map value。
- 本轮同时补齐动态 `MapHas` 的 BC32 packed 编码和 packed hot execution；否则
  presence peephole 会让 entry 因 `MapHas` unsupported 退回 unpacked。另修正
  `MapGetDynamic` 的 RK 常量 key：常量 key 必须转成 `MapHasK`，不能作为
  `MapHas` 的寄存器 key 使用。
- release profile 已确认该轮命中：
  `two_sum_map` 保持 `packed=9/9`，`val_clones=616188 -> 416188`，
  `immediate_clones=613469 -> 413469`；`fraud_rule_scoring` 的
  `fraud_score` closure 出现 `MapHas=1`，`val_clones=1064272 -> 1058694`。
  `opcode_steps` 未降，因为该 peephole主要减少 map value clone/refcount 成本，
  不是减少 dispatch 数。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.820x`，AOT/Lua `0.273x`，AOT/VM `0.071x`。其中
  `two_sum_map` VM 从上一轮 quick table `74.454ms` 降到 `69.760ms`，
  AOT `46.655ms` 降到 `45.896ms`；`fraud_rule_scoring` VM 从 `57.063ms`
  降到 `55.182ms`，AOT 从 `19.930ms` 降到 `17.196ms`。这是单样本方向检查，
  仍需 quiet-machine baseline 固化。
- 本轮补齐 `MapHasK` 的 packed hot slot。此前动态 `MapHas` 已进入
  `PackedHotKind::MapHas`，但 const-key membership 仍只在 packed cold/basic
  路径执行；现在 `EXT_OP_MAP_HAS_K` 会 decode 成 `PackedHotKind::MapHasK`，
  hot_exec 直接用常量池字符串做 `Val::map_contains_str`，避免 const key
  presence check 继续走 enum `Op` cold 分派。
- 已新增 `packed_hot_slot_decodes_map_has_k`，并保留
  `test_bc32_map_has_k_packed_execution` 作为端到端 packed 执行验证。release
  profile 仍显示整套 workload `packed=9/9`；`route_permission_check`
  profile 为 `opcode_steps=1204135`、`typed_branches=298053`、
  `containers=151956`，没有新增 BC32 unpacked/fallback 回退。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.796x`，AOT/Lua `0.270x`，AOT/VM `0.071x`。该轮属于 packed hot
  覆盖面补齐，单样本 VM 几何值相比上一轮 `3.820x` 小幅改善，但不作为稳定
  性能收益声明。
- 本轮先补齐 `ContainsK` 的 packed hot slot 覆盖面。`EXT_OP_CONTAINS_K`
  现在会 decode 成 `PackedHotKind::ContainsK`，hot_exec 直接对
  `ShortStr` / `Str` 执行常量 substring 判断，不再需要经过 packed cold
  `Op::ContainsK`。新增 `packed_hot_slot_decodes_contains_k` 做 decode 覆盖。
  当前 business workload profile 未显示该 op 是主要热点，因此该项主要是
  packed 覆盖面补齐，不声明 workload 级收益。
- 已给 debug packed hot-cache stats 增加 build miss breakdown，并限制
  `LK_DUMP_PACKED_STATS=1` 只在有 build/sentinel 活动时打印，避免 per-frame
  纯 hit 行淹没诊断信息。`fraud_rule_scoring` 的 miss breakdown 显示剩余
  反复 build miss 主要是 `ToBool`、`LoadCapture`、`JmpTrueSet`。
- 已把 `ToBool`、`LoadCapture`、`JmpFalseSet`、`JmpTrueSet` 接入
  `PackedHotKind`、BC32 packed decode 和 packed hot_exec。`LoadCapture`
  复用现有 closure capture 读取逻辑，`Jmp*Set` 保持与 opcode 路径一致的
  dynamic branch metric 语义。新增
  `packed_hot_slot_decodes_capture_bool_and_set_branches` 覆盖 decode。
- release profile 已确认该轮消除了两个代表 workload 的 packed build
  miss：`fraud_rule_scoring` 从 `bc32_fallbacks=61` / `bc32_build_misses=61`
  降到 `0` / `0`，`route_permission_check` 同样降到 `0` / `0`。两者的
  `quickening_misses=0`，说明当前 packed hot-slot 构建阶段已不再因这些
  op 回落到 enum `Op` 冷分派。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.735x`，AOT/Lua `0.269x`，AOT/VM `0.071x`。其中
  `fraud_rule_scoring` VM 从上一轮 quick table `56.596ms` 降到
  `53.099ms`，`route_permission_check` VM 从 `19.750ms` 降到 `18.159ms`。
  这是单样本方向检查；稳定结论仍需要 quiet-machine 多样本 baseline。
- 本轮把 `CmpEqImmJmp`、`CmpNeImmJmp`、`CmpGtImmJmp`、`CmpGeImmJmp` 的
  BC32 编码扩展到 i16 immediate。peephole 现在可以静态融合
  `Cmp*Imm + BoolBranch/JmpFalse` 的宽 immediate guard，例如
  `amount > 900` / `amount > 400`，不再因为 immediate 超出 i8 只保留成
  `CmpGtImm + BoolBranch`。
- 新增 `EXT_OP_CMP_*_IMM16_JMP` 的 encode/decode、packed hot decode 和
  BC32 round-trip 覆盖。`fraud_score` coverage 已确认从 `packed ops=18`
  降到 `packed ops=16`，热分支从 `CmpGtImm=2 + BoolBranch=4` 变为
  `CmpGtImmJmp=2 + BoolBranch=2`。runtime `opcode_steps` 未下降，说明上一轮
  packed hot slot 已经能动态融合执行；这轮价值是把融合前移到 bytecode/BC32
  结构层，让覆盖统计、AOT/VM 共享 IR 和后续优化都能直接看到 fused op。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.385x`，AOT/Lua `0.275x`，AOT/VM `0.082x`。本轮 VM 单样本整体
  好于上一轮，但该改动本身更偏结构补齐；不把几何均值改善全部归因为宽
  immediate fused branch。
- 本轮新增 packed hot-slot 的 `CmpIntJmp + Move` 融合。decoder 现在会识别
  `CmpIntJmp` 紧跟一条 `Move`，且原跳转目标正好是 `Move` 后继的形态，构建
  `PackedHotKind::CmpIntMove`；比较为真时直接写目标寄存器并跳过 `Move`
  dispatch，比较为假时沿用原跳转目标。原始 `Move` 仍留在 BC32 中，其他入口
  跳到该位置时语义不变。
- 该融合命中 `stock_max_profit` 的
  `if price < min_price { min_price = price }` 与
  `if profit > best { best = profit }` 热路径。release profile 显示
  `stock_max_profit` 的 `opcode_steps` 从本轮前约 `3856701` 降到
  `3807223`，`bc32_build_misses=0`，说明优化发生在 packed hot path 内，
  没有引入 cold fallback。
- 已新增 `packed_hot_slot_fuses_cmp_int_jmp_followed_by_move`，覆盖该 fused
  slot 的 decode 与 `next_pc` 语义。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.426x`，AOT/Lua `0.278x`，AOT/VM `0.082x`。`stock_max_profit`
  单项 VM/Lua 为 `4.105x`，受单样本噪声影响不能直接用 timing 表证明稳定收益；
  更直接的证据是 workload profile 的 opcode dispatch 下降。
- 本轮继续收敛 packed call-frame 分派：`run_packed_code` 现在按
  `PackedHotCallKind` 直接分派到 `run_call_packed`、
  `run_call_closure_exact_packed` 或 `run_call_exact_packed`，不再对
  `CallClosureExact` / `CallExact` 先做一遍 `validate_hot_exact_call`，再回到
  generic packed call 路径。这样 exact-call 的语义检查、IC hot probe 和
  cold error 处理都集中在专用函数里，减少 hot path 的重复分派和参数检查。
- 同轮修正了 prepared exact closure cold path 的语义边界：首次 prepared
  exact call 不再在真实 VM closure 调用前直接执行 `TinyCallPlan`。这样读取
  可变全局的零参函数不会在不同 call site 上绕过 global IC/重新读取语义；
  `zero_arg_call_let_binds_reserved_return_slot_without_storelocal`、
  `test_vm_global_ic_invalidation_on_redefine` 和
  `test_vm_global_ic_local_then_global_toggle` 已覆盖该边界。
- release profile 显示 call-heavy workload 的主计数保持稳定且没有 fallback：
  `binary_search` 仍为 `calls=120021`、`closure_calls=120016`、
  `bc32_build_misses=0`，`heap_clones` 从本轮前 `220` 降到 `203`；
  `gcd_batch` 同样从 `heap_clones=220` 降到 `203`。该轮主要是 call-frame 热路径
  清理，不改变 opcode dispatch 总数。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.473x`，AOT/Lua `0.287x`，AOT/VM `0.082x`。单样本 timing 差于上一轮
  `3.426x`，按噪声/机器状态记录，不作为稳定回退结论；更直接的实现进展是
  packed exact call 路径不再重复走 generic validation，并减少少量 heap clone。
- 本轮新增 packed hot-slot 的 map-get compare branch 融合：
  `MapGetDynamic/MapGetInterned -> CmpEq/CmpNe -> BoolBranch` 现在可以合成
  `MapGetDynamicCmpJmp` / `MapGetInternedCmpJmp`。融合后仍会把 map-get 结果写回
  原目标寄存器，保证后续分支体可以继续使用该 value；同时跳过临时 bool
  寄存器写入和独立 branch dispatch。这覆盖 `if map.get(k) != nil { use value }`
  与 `if map.get(k) == nil { init }` 两类 presence+value 热路径。
- 已新增 `packed_hot_slot_fuses_map_get_compare_branch`，覆盖 dynamic key 和
  const interned key 两种 map-get 形态的 fused decode。
- release profile 已确认该轮命中：
  `histogram_group_count` 的 `opcode_steps` 从本轮前 `4084726` 降到
  `3657726`，`inventory_reorder` 从 `2837023` 降到 `2772623`；
  两者 `bc32_build_misses=0`、`quickening_misses=0`，说明融合发生在 packed hot
  path，没有引入 fallback。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.344x`，AOT/Lua `0.283x`，AOT/VM `0.085x`。其中
  `histogram_group_count` VM 为 `105.159ms`，相比上一轮 quick table
  `110.617ms` 有单样本下降；稳定收益仍需 quiet-machine 多样本 baseline 固化。
- 本轮新增 packed hot-slot 的 `AddInt -> FloorDivImm` 融合：
  当 `FloorDivImm` 紧跟 `AddInt` 且读取前者结果时，decoder 构建
  `AddIntFloorDivImm`。执行时仍写回原 `AddInt` 临时寄存器，再写回
  `FloorDivImm` 目标寄存器，因此保留原字节码对临时值的可见性，同时跳过
  后续 `FloorDivImm` 的独立 dispatch。该形态命中 `binary_search_implicit`
  的 `math.floor((lo + hi) / 2)` midpoint 计算。
- 已新增 `packed_hot_slot_fuses_add_int_feeding_floor_div_imm`，覆盖 fused
  decode 与 `next_pc` 语义。
- release profile 已确认该轮命中：
  `binary_search` 的 `opcode_steps` 从本轮前 `11762598` 降到 `10442189`；
  整套 workload runtime metrics 的 `opcode_steps` 从 `47225177` 降到
  `45904768`。两次 profile 均显示 `bc32_build_misses=0`、
  `quickening_misses=0`，说明融合发生在 packed hot path，没有引入 fallback。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.236x`，AOT/Lua `0.272x`，AOT/VM `0.085x`。其中
  `binary_search` VM 从上一轮 quick table `131.854ms` 降到 `115.108ms`；
  这是与 profile dispatch 下降一致的单样本方向检查，稳定结论仍需
  quiet-machine 多样本 baseline 固化。
- 本轮先补了 `split(...).join(...).len()` 的更早期 `.len()` peephole：当
  join 与 split 分隔符是同一个字符串 literal 时，`.len()` 直接对原 receiver
  发 `Len` / `StrLen`，避免后续维护时重新退回 method call 或 `ToIter` 路径。
  新增 `template_split_join_len_lowers_to_original_len` 覆盖模板字符串场景。
  该补丁对当前 `log_parse_filter` profile 没有产生 workload 级计数变化，
  说明该负载当前更大的问题仍是模板字符串 `line` 本身 materialize，而不是
  split/join identity peephole。
- 本轮继续扩大 packed hot-slot 分支覆盖：`CmpIntJmp` 后紧跟 `AddIntImm`
  时，decoder 构建 `CmpIntAddIntImm`。比较为真时直接执行后续整数加立即数并
  跳过其 dispatch；比较为假时仍按原 `CmpIntJmp` offset 跳转。这覆盖
  `binary_search_implicit` 里 `if value < target { lo = mid + 1 }` 这类热分支。
- 已新增 `packed_hot_slot_fuses_cmp_int_jmp_followed_by_add_int_imm`，覆盖 fused
  decode 与 `next_pc` 语义。
- release profile 已确认该轮命中：
  `binary_search` 的 `opcode_steps` 从上一轮 `10442189` 继续降到
  `9843086`；整套 workload runtime metrics 的 `opcode_steps` 从 `45904768`
  降到 `45305665`。profile 仍显示 `bc32_build_misses=0`、
  `quickening_misses=0`。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.419x`，AOT/Lua `0.277x`，AOT/VM `0.081x`。这轮单样本 timing
  受机器状态影响差于上一轮 `3.236x`，不作为稳定回退结论；更直接的进展证据
  是 profile 里的 dispatch 计数继续下降。
- 本轮把临时模板字符串的 split/join/len 直接长度化：
  `let line = "...${...}"; let parsed_len = line.split("|").join("|").len();`
  且 `line` 后续不再使用时，block compiler 现在跳过 `line` 的完整字符串
  materialization，直接用 literal 长度常量加每个插值表达式的 `ToStr + StrLen`
  计算结果。该规则只在临时 `line` 没有后续读取时触发，避免改变可观察语义。
- 已新增 `temporary_template_split_join_len_skips_line_materialization`，覆盖临时
  模板字符串场景，并确认不会退回完整 `StrConcat*` 链。
- release profile 已确认该轮命中 `log_parse_filter`：
  elapsed 从本轮前约 `254.565ms` 降到 `75.648ms`；单 workload
  `heap_clones` 从 `438470` 降到 `2403`，`string_ops` 从 `1030540` 降到
  `594473`，`containers` 从 `1512341` 降到 `917874`。整套 workload
  `heap_clones` 从 `731018` 降到 `294951`，`string_ops` 从 `2287757` 降到
  `1851690`。`bc32_build_misses=0`、`quickening_misses=0`。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.013x`，AOT/Lua `0.291x`，AOT/VM `0.096x`。其中
  `log_parse_filter` VM 从上一轮 quick table `283.003ms` 降到 `76.058ms`，
  VM/Lua 从 `1.320x` 变为 `0.356x`，本轮 quick table 已显示该 workload
  领先 Lua。
- 本轮新增 packed hot-slot 的 map upsert-add 融合：当 hot path 是
  `MapGetDynamic/Interned -> prev == nil -> MapSet(default) else
  MapSet(prev + rhs)` 时，packed decoder 构建 `MapGet*UpsertAdd`，在一个
  hot slot 内完成 map lookup、nil 分支、默认写入或整数加后写回。该规则不改
  LKB/BC32 格式，只优化 packed VM 的热槽执行。
- 已新增 `packed_hot_slot_fuses_map_get_nil_upsert_add`，覆盖 dynamic key 的
  nil/default 与 `AddIntImm` upsert-add decode，并确认 fused `next_pc` 跳到
  原 else-set 之后。
- release profile 已确认该轮命中 `histogram_group_count`：
  `opcode_steps` 从本轮前 `3657726` 降到 `2915726`，`bc32_build_misses=0`、
  `quickening_misses=0`。`register_writes` 在去掉不可见条件临时寄存器写后
  回到 `3227164`，`val_clones` 保持 `1191179`，说明 dispatch 下降没有再引入
  额外 clone。`inventory_reorder` 本轮 profile 未出现 workload 级 dispatch
  下降，后续需要单独审计它的 `map.get/set` 字节码形态。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.296x`，AOT/Lua `0.285x`，AOT/VM `0.087x`。其中
  `histogram_group_count` VM 为 `108.746ms`，单样本 timing 没有体现
  profile dispatch 下降，仍按 profile 证据记录为 VM dispatch 覆盖推进，
  不作为稳定性能胜利结论。
- 本轮继续审计 `inventory_reorder` 未命中 upsert-add 的原因：它的原始
  字节码形态是 `MapGetDynamic` 之后先计算 `delta`，再做 `prev == nil`
  分支，因此只能命中前一轮的 map-get compare 融合，不能命中紧邻
  map-get/nil-branch 的 upsert-add hot slot。
- 已在 block compiler 中加入保守的 delayed delta lowering：识别
  `let current = map.get(m, key); let delta = pure_expr; if current == nil
  { m.set(key, delta) } else { m.set(key, current + delta) }`，并把纯 `delta`
  表达式延后到 then/else 分支内计算。这样 `MapGetDynamic` 后可以立刻接
  `CmpEq + BoolBranch`，让 packed map-get compare branch 融合先命中；同时
  保留 default/add 两个分支的可观察写入语义。
- 该 lowering 已补上 `map` 模块遮蔽检查：`map.get(...)` / `map.set(...)`
  这种模块形式只有在当前 scope 没有局部 `map` 绑定时才会特化；实例方法
  形式 `m.get(key)` / `m.set(key, value)` 不受该限制。这样避免把用户自定义
  `map` 值误当成 stdlib module intrinsic。
- 已新增
  `map_upsert_with_pure_delta_delays_delta_until_after_nil_branch`，覆盖该
  lowering 的结果：第一个 `MapGetDynamic` 后必须紧跟 `CmpEq` 和 branch。
  聚焦测试与上一轮 `packed_hot_slot_fuses_map_get_nil_upsert_add` 均已通过。
- release profile 已确认本轮命中 `inventory_reorder` 的 dispatch 结构：
  `opcode_steps` 从 `2772623` 降到 `2593423`，`bc32_build_misses=0`、
  `quickening_misses=0`。单次 profile elapsed 仍有噪声，因此本轮主要按
  opcode dispatch 下降记录，而不是用单次 wall time 声明稳定收益。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.295x`，AOT/Lua `0.289x`，AOT/VM `0.087x`。其中
  `inventory_reorder` VM/Lua 为 `2.573x`，相比上一轮 quick table `2.741x`
  有单样本方向改善；稳定结论仍需要 quiet-machine 多样本 baseline。
- 本轮继续推进 packed typed-dispatch 覆盖，针对 `binary_search_implicit`
  的热形态新增 `Mul -> CmpIntJmp` hot-slot 融合。实际命中形态不是纯
  `MulInt` 寄存器乘法，而是 RK 常量参与的 generic `Mul`，例如
  `value = mid * 2`，因此 decoder 同时覆盖 generic RK `Mul` 和 typed
  `MulInt` 两种前缀。
- 该融合会先写回乘法目标寄存器，再执行后续 typed compare branch，并跳过
  独立 `CmpIntJmp` dispatch。实现中特别修正了 offset 语义：后续
  `CmpIntJmp` 的 offset 原本相对 compare pc，融合后必须保存绝对
  `jump_pc`，不能直接相对乘法 pc 复用；否则会改变 `binary_search` 的
  checksum。
- 已新增 `packed_hot_slot_fuses_mul_int_feeding_cmp_int_jmp`，覆盖 RK 常量
  `Mul` feeding `CmpIntJmp` 的 fused decode，并断言 fused slot 保存正确
  `jump_pc`。本轮曾用错误 offset 得到 `binary_search` checksum
  `245640000`，修复后恢复为 `243950176`。
- release profile 已确认本轮命中 `binary_search`：
  `opcode_steps` 从 `9843086` 降到 `8522677`，`bc32_build_misses=0`、
  `quickening_misses=0`，checksum 保持 `243950176`。这说明 dispatch 降低
  来自 packed hot path 融合，没有引入 fallback。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.066x`，AOT/Lua `0.288x`，AOT/VM `0.094x`。其中
  `binary_search` VM 从上一轮 quick table `124.251ms` 降到 `108.789ms`，
  VM/Lua 从 `2.594x` 到 `2.233x`；这与 profile dispatch 下降一致，但仍按
  单样本方向检查记录，稳定结论需要后续多样本 baseline。
- 本轮把上一轮的 `Mul -> CmpIntJmp` packed 融合泛化为
  `IntArith -> CmpIntJmp`，新增覆盖 `SubInt -> CmpIntJmp`。该形态命中
  `stock_max_profit` 里 `profit = price - min_price; if profit > best`
  的热路径。decoder 会保存 compare 指令的绝对 `jump_pc`，避免融合后相对
  offset 基准变化导致跳转语义错误。
- 已新增 `packed_hot_slot_fuses_sub_int_feeding_cmp_int_jmp`，并保留
  `packed_hot_slot_fuses_mul_int_feeding_cmp_int_jmp`，分别覆盖 typed `SubInt`
  和 RK 常量 generic `Mul` feeding `CmpIntJmp` 的 fused decode。
- release profile 已确认本轮命中 `stock_max_profit`：
  `opcode_steps` 从 `3807223` 降到 `3300607`，checksum 保持 `2974296`，
  `bc32_build_misses=0`、`quickening_misses=0`。`binary_search` 仍保持上一轮
  `opcode_steps=8522677` 与 checksum `243950176`，说明泛化没有破坏前一轮收益。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.180x`，AOT/Lua `0.282x`，AOT/VM `0.089x`。其中
  `stock_max_profit` VM 从上一轮 quick table `41.501ms` 降到 `40.216ms`；
  单样本整体 geomean 差于上一轮，按机器状态/噪声记录，不作为稳定回退结论。
- 本轮继续扩大 typed branch/op dispatch 覆盖，新增
  `Cmp*ImmJmp -> Mul/ MulInt -> AddInt` 的 packed hot-slot 融合。该形态命中
  `fraud_rule_scoring` 主循环里 `if score >= 70 { checksum += score * 3 }`
  的 true 分支：compare 为真时同一 hot slot 内完成乘法和累加，compare 为假
  时仍按原 `Cmp*ImmJmp` offset 跳到 else 分支。
- 已新增 `packed_hot_slot_fuses_cmp_imm_jmp_followed_by_mul_int_add_int`，覆盖
  RK 常量参与的 `Mul` 以及后续 `AddInt` 被一起跳过 dispatch 的 decode 语义。
- release profile 已确认本轮命中 `fraud_rule_scoring`：
  `opcode_steps` 从本轮前 `2960892` 降到 `2953686`，checksum 保持 `3242465`，
  `bc32_build_misses=0`、`quickening_misses=0`。`cart_pricing_rules` 本轮 profile
  保持 `352314`，说明这次融合没有命中该 workload 的 `cart_line_total` 路径。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.076x`，AOT/Lua `0.274x`，AOT/VM `0.089x`。其中
  `fraud_rule_scoring` VM/Lua 为 `4.136x`，相比上一轮 quick table `4.246x`
  有单样本方向改善；稳定结论仍需要 quiet-machine 多样本 baseline。
- 本轮新增 packed hot-slot 的 `MulInt -> FloorDivImm` 融合：当
  `FloorDivImm` 紧跟 `MulInt` 且读取乘法结果时，decoder 构建
  `MulIntFloorDivImm`。执行时仍写回原乘法临时寄存器，再写回 floor-div 目标
  寄存器，保留字节码可见语义，同时跳过后续 `FloorDivImm` 独立 dispatch。
- 该形态命中 `cart_line_total` 里的
  `math.floor((subtotal * tax) / 100)`，也能覆盖其他乘法后立刻整除的固定
  算术形态。已新增 `packed_hot_slot_fuses_mul_int_feeding_floor_div_imm` 覆盖
  fused decode 与 `next_pc` 语义。
- release profile 已确认本轮命中 `cart_pricing_rules`：
  `opcode_steps` 从本轮前 `352314` 降到 `334814`，checksum 保持 `2221125`，
  `bc32_build_misses=0`、`quickening_misses=0`。`binary_search` 仍保持
  `8522677`，`fraud_rule_scoring` 仍保持上一轮 `2953686`，说明本轮没有破坏
  前几轮 packed fusion。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.144x`，AOT/Lua `0.291x`，AOT/VM `0.093x`。其中
  `cart_pricing_rules` VM 从上一轮 quick table `8.921ms` 降到 `8.431ms`；
  单样本整体 geomean 差于上一轮，按机器状态/噪声记录，不作为稳定回退结论。
- 本轮新增 packed hot-slot 的 `Access -> IntArith` 融合：当 `Access`
  后紧跟 `AddInt` / `SubInt` / `MulInt` / `ModInt` 且使用 access 结果时，
  decoder 构建 `AccessIntArith`。执行时仍写回 `Access` 目标寄存器，再执行
  typed arithmetic，因此保留后续可见语义，同时跳过后一条整数运算的 dispatch。
- 该形态命中 `sliding_window_sum` 的两条核心路径：
  `rolling += values[i]` 和 `rolling -= values[i - window_size]`。已新增
  `packed_hot_slot_fuses_access_feeding_int_arith` 覆盖 fused decode 与 `next_pc`
  语义。
- release profile 已确认本轮命中 `sliding_window_sum`：
  `opcode_steps` 从本轮前 `6580228` 降到 `5668228`，checksum 保持
  `653998251`，`bc32_build_misses=0`、`quickening_misses=0`。`cart_pricing_rules`
  仍保持上一轮 `334814`，`binary_search` 仍保持 `8522677`，说明本轮没有破坏
  前几轮 packed fusion。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.166x`，AOT/Lua `0.283x`，AOT/VM `0.090x`。其中
  `sliding_window_sum` VM 从上一轮 quick table `84.539ms` 降到 `76.455ms`；
  单样本整体 geomean 差于上一轮，按机器状态/噪声记录，不作为稳定回退结论。
- 本轮新增 packed hot-slot 的 `MapHas -> BoolBranch/JmpFalse -> AddIntImmJmp`
  融合，覆盖 `two_sum_map` 中 `if map.get(seen, need_key) != nil { found += 1 }`
  的成员检查和命中计数路径。执行时仍写回 `MapHas` 的布尔结果寄存器；命中时
  在同一 slot 内执行 `found += 1` 并跳回循环，未命中时跳到原 branch false
  target，因此保留原字节码可见语义和跳转方向。
- 该融合同时支持动态 key 的 `MapHas` 和常量 key 的 `MapHasK`，并保留原 profile
  计数粒度：每次成员检查记录一次 map container 操作，每次原 bool branch 记录
  一次 branch，命中时再记录原 `AddIntImmJmp` 的 typed branch。已新增
  `packed_hot_slot_fuses_map_has_branch_increment` 覆盖 fused decode、`true_pc`、
  `false_pc` 和 `next_pc` 语义。
- release profile 已确认本轮命中 `two_sum_map`：
  `opcode_steps` 从本轮前 `2430227` 降到 `2030227`，checksum 保持 `200000`，
  `bc32_build_misses=0`、`quickening_misses=0`。这说明收益来自 packed hot path
  融合，没有引入 fallback。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.024x`，AOT/Lua `0.284x`，AOT/VM `0.095x`。其中 `two_sum_map` VM
  从上一轮 quick table `67.570ms` 降到 `61.046ms`，VM/Lua 从 `1.617x`
  降到 `1.361x`；稳定结论仍需要 quiet-machine 多样本 baseline。
- 本轮针对 `matrix_3x3_multiply` 的固定点积算术链新增两个 packed hot-slot：
  `MulInt -> MulInt -> AddInt` 和 `MulInt -> AddInt`。前者覆盖点积前两项
  `a*b + c*d`，后者覆盖第三项 `partial + e*f`；执行时仍按原顺序写回所有
  乘法临时寄存器和加法目标寄存器，只把连续 typed arithmetic dispatch 合并。
- 已新增 `packed_hot_slot_fuses_two_mul_ints_feeding_add_int` 和
  `packed_hot_slot_fuses_mul_int_feeding_add_int`，覆盖两个 fused decode 的
  `next_pc` 语义。该优化属于 typed op dispatch 覆盖，不改 LKB/BC32 编码。
- release profile 已确认本轮命中 `matrix_3x3_multiply`：
  `opcode_steps` 从本轮前 `756220` 降到 `594220`，checksum 保持 `7973557`，
  `bc32_build_misses=0`、`quickening_misses=0`。`register_writes` 保持
  `756159`，说明融合没有删除可见寄存器写，只减少 dispatch。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.002x`，AOT/Lua `0.286x`，AOT/VM `0.095x`。其中
  `matrix_3x3_multiply` VM 从上一轮 quick table `11.278ms` 降到 `8.907ms`，
  VM/Lua 从 `7.484x` 降到 `5.688x`；稳定结论仍需要 quiet-machine 多样本
  baseline。
- 本轮新增 `IntArith -> AddIntImm` 与 generic RK `Arith -> AddIntImm` packed
  hot-slot 融合。前者覆盖纯寄存器 typed int 算术，后者覆盖 workload 中常见的
  RK 常量 RHS 形态，例如 `(i % 7) + 1`。执行时仍先写回原算术目标寄存器，再写回
  `AddIntImm` 目标寄存器，因此不删除可见寄存器写，只减少后一条立即数加法的
  dispatch。
- 已新增 `packed_hot_slot_fuses_int_arith_feeding_add_int_imm` 和
  `packed_hot_slot_fuses_rk_arith_feeding_add_int_imm`，分别覆盖 EXT typed op 与
  regular RK op 的 fused decode 和 `next_pc` 语义。
- release profile 已确认本轮命中 `order_score_pipeline`：
  `opcode_steps` 从本轮前 `1080220` 降到 `810220`，checksum 保持 `18815414`，
  `bc32_build_misses=0`、`quickening_misses=0`。该融合也继续命中
  `matrix_3x3_multiply` 参数构造路径，使其 `opcode_steps` 从上一轮 `594220`
  进一步降到 `504220`；`fraud_rule_scoring` 也从 `2953686` 降到 `2868686`。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `2.970x`，AOT/Lua `0.277x`，AOT/VM `0.094x`。其中
  `order_score_pipeline` VM 从上一轮 quick table `11.805ms` 降到 `10.506ms`，
  VM/Lua 从 `3.420x` 降到 `3.138x`；稳定结论仍需要 quiet-machine 多样本
  baseline。
- 本轮修复 `test_vm_mutual_recursion_even_odd` 在 cargo test 默认小线程栈下的
  stack overflow。定位结果：不是 LK 语义无限递归，而是 packed BC32 `run_frame`
  的 native Rust 栈帧在互递归调用中叠加过深；同时 `TinyCallPlan` 过去会把
  带早返回和后续调用的函数错误识别为 tiny helper，只看第一个 `Ret`。
- 修复内容：
  - `TinyCallPlan::analyze` 现在只接受首个 `Ret` 后面为空，或只跟编译器生成的
    `LoadK nil; Ret` 默认尾部；如果早返回后还有真实分支/调用代码，则不建立 tiny
    call plan。
  - 命名函数如果函数体内包含调用，暂不生成 closure proto 的 BC32 packed code，
    让递归/互递归路径走常规 opcode `run_frame`，避免 packed 大栈帧在小线程栈下
    溢出。无调用的数值 helper 仍可保留 packed 快路径。
  - 直接调用同一 frame 内的本地函数时，优先使用已经存在的本地 closure 寄存器，
    不再把已知 closure 重新克隆进常量池；仍保留 `CallClosureExact` 的调用信息。
- 验证：
  - `cargo test -p lk-core mutual_recursion -- --nocapture` 通过。
  - `cargo test -p lk-core arith_feeding_add_int_imm` 通过。
  - `cargo test -p lk-core shadowed_function_name_does_not_use_closure_exact_opcode` 通过。
  - `cargo check -p lk-core` 通过。
  - `cargo test -p lk-core` 通过：`772 passed; 0 failed; 3 ignored`。
  - `cargo build --release -p lk-cli` 通过。
- release profile 对核心 workload 基本无影响：
  `order_score_pipeline` `opcode_steps=810221`、checksum `18815414`；
  `fraud_rule_scoring` `opcode_steps=2868687`、checksum `3242465`；
  两者 `bc32_build_misses=0`、`quickening_misses=0`。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `2.955x`，AOT/Lua `0.286x`，AOT/VM `0.097x`。其中
  `order_score_pipeline` VM/Lua `3.044x`，`fraud_rule_scoring` VM/Lua `3.990x`；
  这是单样本验证，仍不替代 quiet-machine 多样本 baseline。
- 本轮收窄上一轮的递归 packed 禁用策略。上一轮为解决互递归小线程栈溢出，
  曾对“命名函数且函数体内含调用”的 proto 全部禁用 BC32 packed；profile 发现
  这会把 `binary_search` 从此前约 `8522677` steps 拉回 `11762599` steps，
  因为 `binary_search_implicit` 里的 `math.floor` 被误判成递归风险。
- 新策略改为维护当前 block 内直接声明的 LK 函数名集合；只有函数体直接调用这些
  同作用域 LK 函数时，才跳过 closure proto 的 BC32 packed code。`math.floor`、
  `map.get`、`starts_with` 等 method/builtin call 不再触发该保护。
- 已新增回归测试：
  - `builtin_call_inside_named_function_keeps_packed_proto`：确认 named helper 内的
    builtin/method call 仍保留 packed proto。
  - `mutually_recursive_named_functions_skip_packed_proto`：确认互递归 named function
    仍跳过 packed proto，并保持执行结果正确。
- release profile 已确认本轮恢复收益：
  `binary_search` `opcode_steps=8522677`、checksum `243950176`，恢复到前几轮
  packed fusion 水平；`cart_pricing_rules` 也因 helper packed 恢复从上一轮 profile
  `334814` steps 降到 `317314`，checksum `2221125`；`gcd_batch` 保持
  `4007080`，`fraud_rule_scoring` 保持 `2868686`。上述 workload 均
  `bc32_build_misses=0`、`quickening_misses=0`。
- 验证：
  - `cargo test -p lk-core mutual_recursion -- --nocapture` 通过。
  - `cargo test -p lk-core builtin_call_inside_named_function_keeps_packed_proto` 通过。
  - `cargo test -p lk-core mutually_recursive_named_functions_skip_packed_proto` 通过。
  - `cargo test -p lk-core` 通过：`774 passed; 0 failed; 3 ignored`。
  - `cargo fmt --all -- --check` 通过。
  - `git diff --check` 通过。
  - 单文件 1500 行检查通过。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `3.111x`，AOT/Lua `0.291x`，AOT/VM `0.094x`。其中
  `binary_search` VM/Lua `2.283x`，单样本受机器状态和 Lua 采样影响，不替代
  profile 的 opcode-step 证据，也不替代 quiet-machine 多样本 baseline。
- 本轮补齐 block compiler 的两语句 map upsert-add lowering：识别
  `let current = map.get(m, key); if current == nil { m.set(key, default) }
  else { m.set(key, current + default) }`，并让 `MapGetDynamic` 后稳定紧跟
  `CmpEq + branch`，与前几轮 packed `MapGet*UpsertAdd` hot-slot 融合所需形状
  对齐。该规则复用现有 `map` 模块遮蔽检查，只在 default 是纯算术表达式且
  后续不再读取 `current` 时触发。
- 已新增
  `map_upsert_with_default_increment_delays_default_until_after_nil_branch`，
  覆盖 `hist.set(bucket, 1)` / `hist.set(bucket, current + 1)` 这种
  `histogram_group_count` 风格的 counter update，断言 `MapGetDynamic` 后
  立即是 nil compare 和 branch，并确认 `current + 1` 降成 `AddIntImm`。
- release profile 显示本轮对真实 workload 没有产生明显 opcode-step 收益：
  `histogram_group_count` 为 `opcode_steps=2803726`、checksum `903000`；
  `inventory_reorder` 为 `opcode_steps=2519824`、checksum `1915398`；
  两者 `bc32_build_misses=0`、`quickening_misses=0`。这说明真实热路径主要已经
  被前几轮 packed fusion 覆盖，或剩余瓶颈在 string key 构造、map mutation 和
  clone，而不是这段 compiler lowering 的相邻形状。
- 验证：
  - `cargo test -p lk-core map_upsert_with_default_increment_delays_default_until_after_nil_branch -- --nocapture` 通过。
  - `cargo test -p lk-core map_upsert -- --nocapture` 通过。
  - `cargo check -p lk-core` 通过。
  - `cargo test -p lk-core` 通过：`775 passed; 0 failed; 3 ignored`。
  - `cargo fmt --all -- --check` 通过。
  - `cargo build --release -p lk-cli` 通过。
  - `git diff --check` 通过。
  - 排除 `references/`、`target/` 和 `website/node_modules/` 后，单文件 1500 行检查通过。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `2.880x`，AOT/Lua `0.283x`，AOT/VM `0.098x`。其中
  `histogram_group_count` VM/Lua 为 `2.405x`，`inventory_reorder` VM/Lua 为
  `2.459x`；该表继续作为每轮方向检查，不替代 quiet-machine 多样本 baseline。
- 本轮针对 `sliding_window_sum` 的滑动窗口热分支新增 packed hot-slot 融合：
  `CmpIntJmp -> SubInt -> Access -> SubInt`。该形态对应
  `if i >= window_size { rolling -= values[i - window_size]; }`，比较为真时在同一
  slot 内计算过期下标、读取 list 值并更新 rolling；比较为假时仍按原
  `CmpIntJmp` offset 跳到分支后。实现保留三条后续 op 的可见寄存器写入语义，
  只减少 packed VM dispatch。
- 已新增 `packed_hot_slot_fuses_cmp_int_jmp_followed_by_sub_access_sub`，覆盖该
  fusion 的 decode、offset 和 `next_pc` 语义。
- release profile 已确认本轮命中 `sliding_window_sum`：
  `opcode_steps` 从本轮前 `5668228` 降到 `4804228`，checksum 保持
  `653998251`，`bc32_build_misses=0`、`quickening_misses=0`。`binary_search`
  仍保持 `opcode_steps=8522677`、checksum `243950176`，说明本轮没有破坏前几轮
  packed fusion。
- 验证：
  - `cargo test -p lk-core packed_hot_slot_fuses_cmp_int_jmp_followed_by_sub_access_sub -- --nocapture` 通过。
  - `cargo check -p lk-core` 通过。
  - `cargo test -p lk-core` 通过：`776 passed; 0 failed; 3 ignored`。
  - `cargo fmt --all -- --check` 通过。
  - `cargo build --release -p lk-cli` 通过。
  - `git diff --check` 通过。
  - 排除 `references/`、`target/` 和 `website/node_modules/` 后，单文件 1500 行检查通过。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已同步写入
  `bench/README.md` 的 `Latest Quick Comparison`。当前 quick run 几何均值：
  VM/Lua `2.918x`，AOT/Lua `0.289x`，AOT/VM `0.099x`。其中
  `sliding_window_sum` VM/Lua 为 `3.029x`；单样本几何值略差于上一轮，按噪声
  记录，不替代 profile 中 dispatch 下降的直接证据。
- 本轮扩大 compiler known-call inline：保留原有直线表达式 inlining，同时允许
  小型分支 helper 在受控条件下内联 prefix statements 后返回表达式。当前覆盖
  `fraud_score` / `cart_line_total` 这类 workload helper，限制为小 block、
  无捕获写逃逸、只引用参数/局部和未遮蔽 stdlib module，避免把任意函数体塞进
  call site。
- 本轮也修复了 inlining 暴露的两个 AOT correctness 问题：
  - `KnownReg` 不再跨非线性 CFG block 盲目复用。AOT translator 现在记录
    block predecessors；直接 fallthrough 继续继承 facts，非直接分支目标只清掉
    中间路径或多前驱路径写过的寄存器 facts，防止把分支改写后的动态 string key
    误降成 `map_has_const_str`。
  - deferred `StringIntKey` 的跨 block 使用只允许继续流向能理解该 fact 的
    map/list/access 专用消费者；同时配合上面的寄存器写集 invalidation，避免
    后续 generic helper 读到 deferral 写入的 Nil。
- 已新增 LLVM 后端回归测试：
  - `clears_known_string_key_facts_at_control_flow_merge`
  - `keeps_deferred_string_int_key_when_branch_consumers_can_use_it`
  并新增 compiler 测试
  `branching_known_call_inlines_without_runtime_call`，覆盖小型分支 helper 的
  known-call inline。
- 正确性 repro 已确认：
  `/private/tmp/lk_inline_risk.lk` 在 VM 和 AOT 下 checksum 都为 `19315`。
  `inventory_reorder` 过滤运行也已确认 VM/AOT checksum 都为 `1915398`，不再出现
  AOT runtime 的 Nil key map set 报错。
- release profile 显示 known-call inline 对目标 workload 的 VM call/clone 有效：
  `fraud_rule_scoring` 约为 `opcode_steps=2868686`、`calls=21`、
  `closure_calls=16`、`heap_clones=94152`；`cart_pricing_rules` 约为
  `opcode_steps=360230`、`calls=21`、`closure_calls=16`、`heap_clones=35209`。
  对比本轮前，两者 runtime call 数从万级降到固定小数量。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已通过且无 checksum
  mismatch，并已同步写入 `bench/README.md` 的 `Latest Quick Comparison`。
  当前 quick run 几何均值：VM/Lua `2.891x`，AOT/Lua `1.942x`，AOT/VM
  `0.672x`。本轮 AOT 为了修复不健全的跨 CFG 优化事实，单样本表仍有明显回退；
  后续 AOT 需要改成 per-block fact merge，而不是重新打开全局 stale fact。
- 本轮开始落地 VM call frame 优化：新增 `TinyCallPlan::EuclidGcd`，只匹配两参数
  Euclidean GCD 的固定 bytecode 形态：
  `LoadLocal, LoadLocal, CmpNeImmJmp, Mod/ModInt, Move, Move, Jmp, Ret`。
  命中后 closure IC 直接在 Rust 中执行整数 GCD 循环；参数不是 `Int` 或函数形态
  不匹配时仍回退普通 VM call，不改变 generic bytecode 语义。
- 已新增 `tiny_call_plan_handles_euclid_gcd_loop`，覆盖实际编译出的 GCD loop、
  `b == 0` 快路径和非整数 fallback。
- release filtered profile 已确认 `gcd_batch` 命中：
  `opcode_steps=400244`、`calls=80021`、`closure_calls=80016`、
  `val_clones=1266`、`heap_clones=203`、checksum `312000`。对比本轮前 profile
  约 `opcode_steps=4007080`、`val_clones=1635998`，说明该优化主要消除了每次
  GCD 调用内部循环的 VM dispatch 和返回值 clone 压力；call site 计数仍保留为
  IC 命中前的调用指令计数。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已通过且无 checksum
  mismatch，并已同步写入 `bench/README.md` 的 `Latest Quick Comparison`。
  当前 quick run 几何均值：VM/Lua `2.609x`，AOT/Lua `2.000x`，AOT/VM
  `0.766x`。其中 `gcd_batch` 从上一轮 `47.227ms / 5.630x` 改善到
  `8.420ms / 1.031x`，状态从 `behind` 变为 `close`。
- 验证：
  - `cargo test -p lk-core tiny_call_plan -- --nocapture` 通过。
  - `cargo test -p lk-core` 通过：`780 passed; 0 failed; 3 ignored`。
  - `cargo fmt --all -- --check` 通过。
  - `cargo build --release -p lk-cli` 通过。
  - `git diff --check` 通过。
  - 排除 `references/`、`target/`、`website/node_modules/` 和
    `vsc-ext/lsp/node_modules/` 后，单文件 1500 行检查通过。
- 本轮继续推进 VM call frame 优化：新增 `TinyCallPlan::BinarySearchImplicit`，
  针对二分查找 helper 的固定数值形态：
  `lo=0; hi=n-1; while lo<=hi { mid=(lo+hi)//2; value=mid*scale; ... }`。
  当前 matcher 同时覆盖 workload 中 peephole 后的 `CmpIntJmp` 形态，以及普通
  编译测试中 `CmpEq/CmpLt + BoolBranch` 的形态；`scale <= 0`、参数不是 `Int`
  或大范围乘法/加法可能溢出时会回退普通 VM call。
- 已新增 `tiny_call_plan_handles_implicit_binary_search_loop`，覆盖命中、未命中、
  `n == 0` 和非整数 fallback。
- release filtered profile 已确认 `binary_search` 命中：
  `opcode_steps=600289`、`calls=120021`、`closure_calls=120016`、
  `val_clones=241262`、`heap_clones=203`、checksum `243950176`。对比本轮前
  profile 约 `opcode_steps=8522677`、`val_clones=601259`，说明该优化消除了
  每次二分 helper 调用内部循环的 VM dispatch；call site 计数仍保留为 IC 命中前
  的调用指令计数。
- 本轮 `RUNS=1 EXTRA_RUNS=0 bench/run_workload_bench.sh` 已通过且无 checksum
  mismatch，并已同步写入 `bench/README.md` 的 `Latest Quick Comparison`。
  当前 quick run 几何均值：VM/Lua `2.280x`，AOT/Lua `1.982x`，AOT/VM
  `0.870x`。其中 `binary_search` 从上一轮 `114.534ms / 2.380x` 改善到
  `14.166ms / 0.279x`，状态从 `behind` 变为 `ahead`。

后续仍需要补：

- 继续审计 `run_call_*` 剩余差异，只保留 opcode/packed 必须不同的 pc 更新、
  native-fast 外壳和 named-call 指令外壳。
- 继续扩大 typed branch 的 compiler facts 覆盖面，让更多 workload 在编译期
  直接生成 typed branch，而不是依赖 packed hot-slot 动态融合。

## 优先落地顺序

1. 加 profiler 和 counters，确认每个 workload 的真实热路径。
2. 让 `Val` clone/key allocation/function call fallback 可观测。
3. 先打 map/list/string 三个 AOT 慢项，因为纯数字 AOT 已经赢 Lua。
4. 再打 VM call frame 和 typed branch/op dispatch。
5. 最后把 Performance IR 固化成默认热路径，generic bytecode 只做 fallback。
