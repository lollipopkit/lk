# Opcode 设计结论

## 结论

当前 `Opcode` 不是长期最优设计，但当前优化阶段应避免 benchmark-shaped 专门 opcode。短期继续用 compiler facts、lowering 消除、typed fast path、hot/cold 拆分，以及 Lua-style operand-shape opcode 推进。

当前 encoding 基础迁移已完成：32-bit instruction 仍保留，但 opcode 从 6 bit 扩到 7 bit，`InstrFormat` 不再写入 instruction bits，而是由 `OpcodeInfo` metadata 决定；`ABC` 的 `C` operand 恢复为 8 bit。`Bx` 访问接口当前仍保持 `u16`，因为常量池、globals、captures 和 LLVM/AOT 索引路径仍按 `u16` 组织；完整 17-bit `Bx` 使用需要后续单独迁移索引类型。

`Opcode::ForLoopI` 曾是一个历史临时例外：它复用了原 `Extra = 62` 槽位，用于静态正/负 step 的整数 range loop。当前 opcode 已按语义分区重排序，`ForLoopI` 不再保留在该历史槽位；该历史只说明它当时用于验证 Lua-style numeric loop opcode 对真实 workload 的收益，不是继续追加 opcode 的先例。后续新增 opcode 应基于当前 7-bit encoding，并且只做通用 operand-shape specialization。

参考 Lua 后，推荐的长期方向是：

- 保留 register VM。
- 保留 32-bit fixed instruction。
- 删除当前指令内的 `InstrFormat` bits。
- 用 opcode metadata table 决定 instruction format、写寄存器行为、test 行为和 top 语义。
- 保留 `ExtraArg` 扩展槽。
- 只引入 Lua-style operand-shape specialization，不引入 workload-specific fused opcode。

## 当前状态

- opcode encoding 已有 7 bit，`0..127` 可用；当前已定义 opcode 到 `105`。
- opcode 编号已按语义分区重排序：基础 move/load/return、整数 arithmetic/immediate/accumulator、float、compare/test、branch/loop、call/global/cell、container/index/string、error/control。artifact version 已 bump 到 `2`，避免旧 raw instruction word 被新编号误解。
- `InstrFormat` 已由 `OpcodeInfo` metadata 决定，不再占 instruction bits。
- `ABC` 的 `C` operand 已恢复 8 bit。
- `Bx`/`sBx` 当前 API 仍是 `u16`/`i16` 兼容面；要完整使用 17-bit payload，需要同步扩大 const/global/capture/function 索引类型。
- `Extra` / `Wide` 仍需要重新整理为长期 `ExtraArg` 语义。
- `GetIndex` / `SetIndex` 过于泛化，很多已知 facts 需要运行时反复查询。

因此，后续可以新增 opcode，但必须满足两点：一是基于当前 7-bit encoding，不复用保留槽；二是只加入 counters 证明过的通用 operand-shape opcode。当前已加入 `AddIntI` / `MulIntI` / `ModIntI`、`AddMulInt` / `Add2Int`、typed int-list accumulator 的 `AddListInt` / `SubListInt`、integer midpoint 的 `MidInt`、`MinInt` / `MaxInt`、`BrNil` / `BrNotNil`、typed compare-test、zero/small-int/mod-zero direct branch、`GetFieldK` / `SetFieldK`、string-prefix + int-suffix map key 的 `GetIndexStrI` / `SetIndexStrI`、3+ part template string 使用的通用 `ConcatN`、常见返回路径 `Return0` / `Return1`，以及相邻本地赋值链 `Move2`；后续优先候选是更系统的 branch-chain / hot-loop lowering。`GetI` / `SetI` 仍是长期 Lua-style 形状，但最新 dynamic key 分桶显示当前 hot dynamic map key 几乎都是 short string 而不是 integer key，所以暂不作为下一步默认优化。

## Lua 可借鉴点

Lua 的关键设计不是业务专用 opcode，而是 operand shape 进入指令形状：

- opcode 数量足够，format 不占 instruction bits。
- `OP_EXTRAARG` 用于大常量和扩展参数。
- `OP_ADDI`、`OP_ADDK` 表达 immediate/constant operand。
- `OP_GETI`、`OP_SETI`、`OP_GETFIELD`、`OP_SETFIELD` 表达整数 key 和 const string key。
- `OP_RETURN0`、`OP_RETURN1` 避免常见返回路径走泛化 return。
- `OP_FORPREP`、`OP_FORLOOP` 把数值 for loop 热路径压缩到专门 loop opcode。

LK 应借鉴这些通用 operand-shape opcode，而不是做 `ListFoldAdd`、`MapValuesFoldAdd` 这类 benchmark-shaped opcode。

## 已落地 encoding

目标 instruction layout：

```text
Op(7) | A(8) | k(1) | B(8) | C(8)
```

推荐 format：

- `ABC`: `A(8), k(1), B(8), C(8)`
- `ABx`: `A(8), Bx(17)`
- `AsBx`: `A(8), sBx(17)`
- `Ax`: `Ax(25)`
- `sJ`: `sJ(25)`

当前配套 metadata：

```text
OpcodeInfo {
  format,
}
```

format 由 `OpcodeInfo` 决定，不再写入 instruction bits。当前 metadata 先只承载 `format`；`writes_a`、`is_test`、`uses_top`、`sets_top` 可在新增 operand-shape opcode 前继续补齐。这样 opcode 空间已变成 128，`C` 回到 8 bit，并保留 `k` bit 作为未来常量/翻转/测试极性等轻量 flag 的位置。

## 推荐 opcode 形状

### Move / Load

- `Move`
- `Move2`
- `LoadNil`
- `LoadBool`
- `LoadIntI`（候选，当前不保留默认 lowering）
- `LoadK`
- `LoadKX`
- `ExtraArg`

`Move2 A B C` 已落地，语义等价于顺序执行 `A = B; B = C`，不是 swap。compiler 只在相邻本地赋值链 `x = y; y = z`、相关 local 非 cell local、且不涉及 const-map local 时发射；普通 block 和 direct-call inline block 都会尝试该 lowering。它是通用 register-copy shape，覆盖 Euclid rotation、状态推进等常见局部变量滚动更新，不针对某个 workload 名称。profile 证明 direct-call inline 接入前 `gcd_batch` 热路径没有执行 `Move2`；接入后 `gcd_batch` 动态 `Move` 从约 `817K` 降到约 `160K`，动态 `Move2` 约 `657K`。连续 `Move` dispatch 也已改成 tight next-op bounds check，避免每步走 `code.get(...).map(...)`。此前默认 VM 样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 的 geomean 为 `0.897x`，checksum 全部一致，runner 输出 `AOT: disabled`；收益仍小，说明 `Move2`、direct-to-destination lowering、`MinInt` / `MaxInt` 和解释器 `Move` cleanup 都是正确的 register-write 覆盖修正，不是 `<0.5x` 主路径。

本轮验证过 `LoadIntI A sC` 直接写小整数 literal，但不保留默认 lowering：它会把 loop cached literal 的廉价 `Move` 替换成大量 `LoadIntI`，`state_machine_transitions` 和 `config_defaults_merge` 的 profile elapsed 退化。结论是当前小整数 literal 不应简单从 cache+move 改成 immediate load；如果后续要做 literal 方向，应和 loop-carried register rewrite 或 branch chain lowering 一起设计。

### Arithmetic

- `AddInt`
- `SubInt`
- `MulInt`
- `DivInt`
- `ModInt`
- `AddFloat`
- `SubFloat`
- `MulFloat`
- `DivFloat`
- `ModFloat`
- `AddIntI`
- `MulIntI`
- `ModIntI`
- `AddMulInt`
- `MidInt`
- `MinInt`
- `MaxInt`
- `AddListInt`
- `SubListInt`

已实现一个整数 immediate opcode：`AddIntI A B sC`。`x -= 3` 和 step `-1` 都编译成 `AddIntI` 的负 immediate。不要加 `SubIntI`，它浪费 opcode，也容易制造方向 bug。

当前 `AddIntI` 已接入 VM dispatch、compiler lowering、LLVM straightline/scalar lowering 和动态 opcode histogram。profile 显示它确实覆盖 `gcd_batch`、`order_score_pipeline`、`config_defaults_merge` 等 workload 的 small-int add/sub hot path；但 release 低样本 geomean 没有改善，因此它不是 `<0.5x` 的主路径。

本轮新增同一类通用 immediate arithmetic opcode：

- `MulIntI A B sC`: `A = B * sC`
- `ModIntI A B sC`: `A = B % sC`

compiler 只在 RHS 是 `i8` 范围内的 int literal，且 register facts 确认 LHS 为 `Int` 时默认发射；`AddIntI` / `MulIntI` 还覆盖 commuted small-int immediate shape：`literal + x` / `literal * x` 在 RHS facts 确认 `Int` 且 static flavor 为 `Int` 时同样发射 immediate opcode。`ModIntI` 不对 literal `0` 发射，也不做 commuted lowering；VM 和 LLVM lowering 仍保留 divisor-zero 防护。它们已接入 VM dispatch、compiler lowering、LLVM straightline/callee/scalar/subfunction lowering 和 tests。VM dispatch 必须使用当前调用帧的 `frame_base` 读写寄存器；本轮已补 direct-call callee 回归测试，避免 immediate arithmetic 在函数调用中误读 entry frame。profile 显示覆盖是真实通用数值 shape：`MulIntI` 出现在 `binary_search:1440409`、`stock_max_profit:1080000`、`gcd_batch:160000`，`ModIntI` 出现在 `log_parse_filter:782684`、`inventory_reorder:478001`、`config_defaults_merge:435000`、`route_permission_check:360002`。普通 release 低样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh` geomean 为 `1.139x`，checksum 全部一致；其中 `gcd_batch` 和 `stock_max_profit` 有较高噪声，仍需要正式多样本复验。

VM dispatch 还保留了一个不改变 bytecode 的 peephole：`ModInt` / `ModIntI` 写出 Int 后，如果下一条指令正好是读取同一目标寄存器的 `BrEqZeroInt` / `BrNeZeroInt`，解释器直接按余数应用分支并跳过下一次 dispatch。这不是新增 opcode，也不改变 LLVM lowering；它覆盖的是 modulo-zero guard / divisibility check 这类通用 control-flow shape。

同类 dispatch peephole 也覆盖 branch/test 的常见 fallthrough body：`BrNil` / `BrNotNil`、`BrEqZeroInt` / `BrNeZeroInt`、`BrEqIntI4` / `BrNeIntI4` fallthrough 后如果正好是 `Move + Jmp` 或单条 `Move`，`TestEqIntI2` true fallthrough 后如果正好是 `Move + Jmp` 或单条 `Move`，以及普通 compare-test 不跳转后如果正好落到 `Move + Jmp` 或单条 `Move`，VM 会直接执行该 `Move` 并在需要时使用后继 `Jmp` 更新 pc。`GetFieldK` 写出值后如果下一条正好是读取同一目标寄存器的 `BrNil` / `BrNotNil`，解释器会立即应用 nilness branch，并在 fallthrough 是 default `Move` 时直接执行该 `Move`；这覆盖的是通用 field default / nil-check shape。这些都不是新 opcode；只是减少 existing branch-chain bytecode 的解释器调度，不改变 bytecode 或 LLVM lowering。ordering compare-test 的 `TestLtInt` / `TestLeInt` / `TestGtInt` / `TestGeInt` 现在也有专门 VM dispatch arm，避免热 typed condition 走 generic compare-test 二级 opcode match。2-part/3+ part template string assignment 现在 direct-to-destination，避免 `ConcatString/ConcatN temp; Move dst temp`。最新默认 VM 正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 为 `0.783x`，此前 ordering compare-test hot arm cleanup 后为 `0.785x`，`MidInt` 后为 `0.792x`，`AddListInt` / `SubListInt` 后为 `0.797x`，`Add2Int` 后为 `0.796x`，profile 分桶后复测为 `0.800x`，保留 `ConcatN` register-window 直写的两轮为 `0.798x` / `0.793x`，checksum 全部一致，runner 输出 `AOT: disabled`；static coverage 为 `instructions=1902`、`LoadInt=313`、`AddInt=99`、`SubInt=43`、`AddIntI=58`、`Add2Int:2`、`AddListInt:1`、`SubListInt:1`、`MidInt:2`、`BrModNeZeroIntI4:21`、`BrNeZeroInt:9`、`GetIndex=62`、`GetList=4`、`Move=300`。

本轮还新增了不改变 opcode set 的 compiler-level branch-chain/register-write 重排：安全形状 `let/assign x = default; if cond { x = value } else if ...` 会被 lower 成 synthetic final else default，避免命中分支路径先写 default 再立刻覆盖。该优化只接受 default 是 literal/local、目标不是 cell local、condition 和 then value 不引用目标、then 分支均为单条同目标赋值、else 链为嵌套 if 或空的保守形状；它不是 workload-specific fused opcode，也不改变普通赋值语义。正式样本显示 `state_machine_transitions` 从 `1.228x` 改善到 `1.167x`，但 static `Jmp` 从 `157` 增到 `166`，所以它只是小幅全局收益。后续 branch-chain 方向应继续在 compiler 侧做直接写目标 register / final-default 重排，而不是继续堆叠 dispatch target peephole。

本轮新增通用 compound integer multiply-add opcode：

- `AddMulInt A B C`: `A = A + B * C`
- `Add2Int A B C`: `A = A + B + C`
- `AddListInt A B C`: `A = A + B[C]`
- `SubListInt A B C`: `A = A - B[C]`

compiler 只在 compound-add accumulator 可原地写、RHS 已被 facts-confirmed 为纯 Int additive expression、且该 additive expression 至少还有另一个 term 时默认发射 `AddMulInt` / `Add2Int`。`AddMulInt` 覆盖 multiply term，`Add2Int` 覆盖相邻两个普通 Int term；它们不是 workload-specific fused opcode，覆盖的是常见 `sum += left * right`、`score += weight * factor`、`checksum += state + event` 这类 shared accumulator operand shape。`AddListInt` / `SubListInt` 只在 compound assignment 中发射，要求 accumulator 是 Int、target 是非 cell local 的 facts-confirmed `List<Int>`、key 可证明为 Int-like；它覆盖 `total += values[i]` / `total -= values[j]` 这类 typed list-index accumulator operand shape。`AddMulInt` / `Add2Int` 已接入 VM dispatch、compiler lowering、LLVM straightline/callee/scalar/subfunction lowering 和 arithmetic regression tests；`AddListInt` / `SubListInt` 当前作为默认 VM typed-list accumulator 优化接入 VM/compiler/tests，显式 native/AOT 路径仍可在 unsupported shape 上保守回退。single-term expansion 曾验证过但回退，因为它扩大了发射范围而没有稳定收益；`AddModIntI` 也曾验证并回退。string-prefix/int-suffix map key lowering 后最新默认 VM 正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 的 VM/Lua geomean 为 `0.783x`，checksum 全部一致，runner 输出 `AOT: disabled`；static coverage 显示 `instructions=1902`、`AddMulInt:10`、`Add2Int:2`、`AddListInt:1`、`SubListInt:1`、`MidInt:2`、`SetIndexStrI:3`、`GetList:4`。profile 显示 `state_machine_transitions Add2Int:120000`，opcode steps 从约 `1.663M` 降到约 `1.543M`；`sliding_window_sum AddListInt:480000`，opcode steps 从约 `7.06M` 降到约 `6.15M`。收益仍是局部通用 accumulator/register materialization lowering，不是 `<0.5x` 主路径。

本轮新增通用 integer midpoint opcode：

- `MidInt A B C`: `A = (B + C) / 2`

compiler 只在 `math.floor((lhs + rhs) / 2)` 且 `lhs` / `rhs` 都可由 facts 证明为 Int-like 时默认发射。它覆盖二分、区间收缩和其它通用 midpoint operand shape，不针对某个 workload 名称；不满足 Int-like 或未被证明的 `math` 外部全局条件时仍走原有 call/arithmetic lowering。VM dispatch 使用当前语义下的 `wrapping_add` 后整数除以 `2`，与原 `AddInt + DivInt` 路径保持一致；LLVM straightline/callee/scalar/subfunction lowering 已同步接入。该轮默认 VM 正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 的 VM/Lua geomean 为 `0.792x`，后续 ordering compare-test hot arm cleanup 后为 `0.785x`，template assignment direct-to-destination 后最新默认 VM 为 `0.783x`；checksum 全部一致，runner 输出 `AOT: disabled`。static coverage 显示 `instructions=1918`、`MidInt:2`、`AddInt=99`、`DivInt=16`，相对前序 `instructions=1920`、`AddInt=101`、`DivInt=18` 少了两组 midpoint arithmetic。

本轮新增通用 min/max update opcode：

- `MinInt A B C`: `A = min(B, C)`
- `MaxInt A B C`: `A = max(B, C)`

compiler 只在没有 `else` 的单赋值 `if` 中默认发射，且要求目标 local 不是 cell local、目标和候选值 facts 均已确认 `Int`，形状为 `if candidate < current { current = candidate }` 或 `if candidate > current { current = candidate }` 及其等价反向比较。该 opcode 不融合业务逻辑，只把通用 min/max register update 从 “typed compare-test + branch + assignment” 收成一条 arithmetic-style write。VM dispatch 与 LLVM straightline/callee/scalar/subfunction lowering 均已接入，LLVM 使用 `icmp slt/sgt` + `select`。profile 显示 `stock_max_profit` 中 `MinInt:540000`、`MaxInt:540000`；该轮默认 VM 正式样本 geomean 为 `0.897x`，checksum 全部一致。该形状可保留，但整体收益仍很小。

`AddK` / `SubK` / `MulK` / `DivK` / `ModK` 可以等动态 opcode histogram 证明收益后再加。

### Branch / Compare

表达式需要 bool 时保留：

- `CmpInt`
- `CmpNeInt`
- `CmpLtInt`
- `CmpLeInt`
- `CmpGtInt`
- `CmpGeInt`

控制流分支不应 materialize bool。当前已有 `BrTrue A sBx` / `BrFalse A sBx` 的 IR、VM dispatch、control-flow facts 和 LLVM lowering 支持，但 compiler 默认不发它们：低样本 bench 证明把现有 `Test + Jmp` trampoline 直接替换成单条 branch 没有 wall-clock 收益，VM/Lua geomean 约 `1.219x`。

当前默认启用的是更具体的 nilness branch：

- `BrNil A sBx`
- `BrNotNil A sBx`

它们只覆盖 condition-context 下的 `x == nil` / `x != nil`，不改变普通表达式比较仍返回 bool 的语义。该 opcode 已接入 VM dispatch、compiler condition lowering、direct-inline `if` / `while` lowering、LLVM scalar lowering 和 control-flow facts。最新低样本 VM/Lua geomean 从前一轮约 `1.197x` 降到 `1.131x`；profile 显示 `config_defaults_merge` 中 `BrNotNil:360000`，证明它覆盖的是通用 default/nil-check shape，而不是某个 workload 专用 opcode。

本轮新增 Lua-style compare-test opcode，并默认只在 compiler facts 已确认 `Int/Int` 的 condition-context 比较中启用：

- `TestEqInt A B k`
- `TestNeInt A B k`
- `TestLtInt A B k`
- `TestLeInt A B k`
- `TestGtInt A B k`
- `TestGeInt A B k`

这些 opcode 约定下一条必须是 `Jmp`，`A/B` 是比较操作数，`k/C` 表示 jump_when。VM handler 会直接消费下一条 `Jmp`，避免先把比较结果 materialize 成 bool。LLVM scalar/control-flow facts 已接入该形状。

验证结论：全量动态 compare-test lowering 会退化，低样本 geomean 约 `1.234x`；原因是动态比较 fallback 成本高于省下的 bool materialization。收窄为 facts-confirmed `Int/Int` 后，低样本 geomean 约 `1.217x`，profile 显示 `gcd_batch TestNeInt:737372`、`state_machine_transitions TestEqInt:1114278`、`config_defaults_merge TestEqInt:540000`。因此 compare-test 可以作为通用 typed operand-shape opcode 保留，但不应对 unknown/dynamic 比较默认启用。

当前 compare-test VM hot path 继续按 Lua-style “test opcode consumes following jump” 形状工作，但 compiler control-flow facts 会记录后继 `Jmp` patch 后的 absolute target pc，避免执行时重复读取、校验后继 `Jmp` 并重新计算 relative target；非 Int/Int fallback 已拆到 cold helper，避免动态比较和错误构造污染 typed hot helper。`TestEqInt` / `TestNeInt` / `TestLtInt` / `TestLeInt` / `TestGtInt` / `TestGeInt` 现在都有直接 dispatch arm，减少 typed compare-test 的二级 opcode match；ordering arm 覆盖 `binary_search`、`prime_trial_division`、`sliding_window_sum`、`fraud_rule_scoring` 等通用 Int/Int 条件。最新默认 VM 正式样本为 `0.783x`，checksum 全部一致，runner 输出 `AOT: disabled`。历史 AOT 样本验证命令 `RUN_AOT=1 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh` 的 VM/Lua geomean 为 `1.063x`，AOT/Lua geomean 为 `0.351x`，VM/Lua/AOT checksum 全部一致；当前默认直接执行仍是解释器 VM，尚未达到 `<0.5x`。这些结果只能说明 typed compare-test 和当前通用 opcode 方向可继续推进。本轮验证过为所有 `Jmp` 预计算 absolute target 会导致 `gcd_batch` timeout，已回退。把 hot fact accessor 标记为 `#[inline(always)]` 的样本没有稳定改善，已回退；typed int sidecar/register cache 低样本退到 `1.210x`，已撤回。继续验证过仅对 dynamic equality/inequality condition 放宽 `TestEqInt` / `TestNeInt` lowering：静态 `CmpInt` 从 `8` 降到 `2`，新增 `TestEqInt/TestNeInt` 覆盖，但默认 VM geomean 退到 `1.042x`，已回退。

本轮继续新增 equality/inequality immediate compare-test：

- `TestEqIntI A sC k`
- `TestNeIntI A sC k`

compiler 只在 condition-context 下、register facts 已确认 lhs 为 `Int`、rhs 是 `i8` 范围内 int literal 时默认发射；`B` 保存 `jump_when`，`C/sc()` 保存 signed immediate。该形状把 `x == 0` / `x != 0` 这类通用 literal compare 分支从 “load literal register + typed compare-test” 收成单条 test opcode，不改变普通表达式比较返回 bool 的语义。profile 证明它覆盖真实 workload 形状：`gcd_batch TestNeIntI:737372`、`state_machine_transitions TestEqIntI:1114278`、`config_defaults_merge TestEqIntI:540000`、`fraud_rule_scoring TestEqIntI:412018`、`route_permission_check TestEqIntI:298053`。使用默认正式样本 `LK_FORCE_VM=1 RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 验证时，开启 immediate lowering 的纯 VM/Lua geomean 为 `1.077x`，关闭该 lowering 的 A/B 为 `1.081x`，checksum 全部一致。收益很小但方向正确；它是 compare shape coverage 优化，不是 `<0.5x` 主路径。

本轮继续新增 zero-compare direct branch：

- `BrEqZeroInt A sBx`
- `BrNeZeroInt A sBx`

compiler 只在 condition-context 下、register facts 已确认 operand 为 `Int`、另一侧是 literal `0` 时默认发射，并且只用于 false edge lowering：`x != 0` 的 false edge 发射 `BrEqZeroInt`，`x == 0` 的 false edge 发射 `BrNeZeroInt`。它把常见 zero compare 从 “immediate compare-test + following `Jmp`” 收成单条 `AsBx` branch，同时保留普通表达式比较返回 bool 的语义。VM dispatch 与 LLVM scalar/control-flow lowering 均已接入；target offset 沿用 `pc + 1 + sBx`，与 `Jmp` / `BrNil` / `BrNotNil` 一致。新增 regression test 覆盖 `while (b != 0)`、`if (a == 0)` 和 `if (a != 0)`，确保 zero branch 不造成循环无法退出。

验证结果：static coverage 中全 workload `instructions` 为 `1992`，`Jmp` 从前序 `198` 降到 `167`，`TestEqIntI` 从 `40` 降到 `10`，新增 `BrEqZeroInt:1`、`BrNeZeroInt:30`。profile 显示 `gcd_batch BrEqZeroInt:737372`、`config_defaults_merge BrNeZeroInt:360000`、`fraud_rule_scoring BrNeZeroInt:412018`、`route_permission_check BrNeZeroInt:298053`、`state_machine_transitions BrNeZeroInt:154285`。该形状是通用 zero-compare branch 优化，可保留。

本轮继续新增 small-int direct branch：

- `BrEqIntI4 A imm4 offset12`
- `BrNeIntI4 A imm4 offset12`

compiler 只在 condition-context 下、register facts 已确认 operand 为 `Int`、另一侧是 `0..15` int literal 时默认发射，并且只用于 false edge lowering：`x != K` 的 false edge 发射 `BrEqIntI4`，`x == K` 的 false edge 发射 `BrNeIntI4`。它把 small-int equality branch 从 “immediate compare-test + following `Jmp`” 收成单条 branch，同时保留普通表达式比较返回 bool 的语义。编码复用 `ABx` payload：高 4 bit 保存 immediate，低 12 bit 保存 signed offset + bias，target 语义仍是 `pc + 1 + offset`。VM dispatch、compiler control-flow facts、LLVM scalar/control-flow lowering 均已接入；反汇编专门显示为 `BrNeIntI4 r16 2 6`，避免输出 packed `Bx`。

验证结果：static coverage 中全 workload `instructions` 现在为 `1984`，`Jmp` 为 `157`，新增 `BrNeIntI4:10`，`TestEqIntI` 为 `6`，`TestEqIntI2` 为 `6`。该轮默认 VM 正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 的 VM/Lua geomean 为 `0.897x`，checksum 全部一致，runner 输出 `AOT: disabled`；`state_machine_transitions` 为 `1.529x`，比早前 `2.073x` 仍明显改善。该形状是通用 small-int equality branch 优化，可保留，但默认 VM 仍未达到 `<0.5x`。

本轮继续新增 small-divisor modulo-zero direct branch：

- `BrModEqZeroIntI4 A divisor4 offset12`
- `BrModNeZeroIntI4 A divisor4 offset12`

compiler 只在 condition-context 下、register facts 已确认 dividend 为 `Int`、divisor 是 `1..15` 的 int literal、另一侧是 literal `0` 时默认发射，并且只用于 false edge lowering：`(x % K) != 0` 的 false edge 发射 `BrModEqZeroIntI4`，`(x % K) == 0` 的 false edge 发射 `BrModNeZeroIntI4`。编码复用 `branch_i4` 的 `ABx` payload：高 4 bit 保存 divisor，低 12 bit 保存 signed offset + bias。VM dispatch 直接读取 dividend register，计算 `x % K` 后跳转或 fallthrough；compiler 不会发 divisor `0`，VM 和 LLVM lowering 仍保留 defensive zero-divisor check。LLVM scalar/subfunction/callee lowering 已同步接入，避免新 opcode 让显式 native/AOT 路径退成 unsupported shape。

验证结果：static coverage 中全 workload `instructions` 从 `1973` 降到 `1952`，`BrNeZeroInt` 从 `30` 降到 `9`，新增 `BrModNeZeroIntI4:21`，`ModIntI` 为 `48`。该轮默认 VM 正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 的 VM/Lua geomean 为 `0.795x`，checksum 全部一致，runner 输出 `AOT: disabled`。首轮同参数样本为 `0.808x`，第二轮为 `0.795x`；保留依据是 coverage 明确减少通用 modulo guard bytecode，且最新 checksum-clean 正式样本优于此前 `0.805x` 记录。该形状覆盖 divisibility guard / modulo-zero branch，不针对某个 workload 名称。

本轮还验证并回退了 register-vs-register short branch 候选：

- `BrEqInt8 A B sC`
- `BrNeInt8 A B sC`
- `BrLtInt8 A B sC`
- `BrLeInt8 A B sC`
- `BrGtInt8 A B sC`
- `BrGeInt8 A B sC`

该形状用 `ABC` 的 `A/B` 保存两个 int register，`C` 保存 signed i8 offset，只适合短跳转 false edge。profile 证明它覆盖通用热点：`binary_search` 中 `BrGtInt8` / `BrNeInt8` 各约 `1.32M`，`sliding_window_sum` 中 `BrLtInt8` 约 `960K`，并把 static `Jmp` 从 `157` 降到 `141`。但默认 VM 正式样本只从 `0.897x` 小幅到 `0.887x`，收益不足；同时显式 AOT smoke 暴露当前 loop 后 dynamic map `GetIndex` native lowering 仍有缺口，不能在默认 artifact 里继续引入该 candidate。该实现已回退；后续若修复 native dynamic map lowering 并确认多样本收益，再重新设计。

长期可选两种方案：

- Lua-style compare opcode 作为 test，并约定下一条是 jump。
- 或显式 `BrEqInt` / `BrNeInt` / `BrLtInt` / `BrLeInt` / `BrGtInt` / `BrGeInt`。

当前阶段不把 `BrTrue/BrFalse` 作为默认 lowering；继续用 facts-driven fused branch lowering、nilness branch、zero/small-int direct branch、typed compare-test 和 register-write 消除推进。下一步如果继续做 branch opcode，应避免 unknown/dynamic 比较走 compare-test。`Br*Int A rhs sBx` 放不进当前 32-bit `AsBx` instruction；small-int i4 branch 证明 packed immediate+offset 可覆盖一部分通用 branch chain，但完整 register-vs-register branch 仍需要单独设计 rhs register fact、复用 `k` bit，或继续采用 Lua-style compare-test + next jump 形状。

继续验证过一个更窄的 VM dispatch 候选：只在 direct nil/zero/small-int branch 或 inline zero branch 的 taken target 是当前 pc 后 1..4 条内的 `Move` / `Move + Jmp` 时直接执行该 move。该候选不触碰 compare-test，也不触碰 backward target，但低样本默认 VM geomean 退到 `0.832x`，`state_machine_transitions` 退到 `1.426x`，已回退。结论是 branch-chain 方向应转向 compiler-level 重排/直接写目标 register，而不是继续在 dispatch 中追加 target peephole。

继续验证过 arithmetic/list tail 后接 `ForLoopI` 的 dispatch peephole：`AddInt` / `AddIntI` / `AddMulInt` / `ListPush` 成功写入后，如果下一条就是 `ForLoopI`，在当前 handler 内直接执行 loop update 和 bounds check。该候选不新增 opcode，也覆盖通用 loop-tail 形状；但默认正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 退到 `0.856x`，`state_machine_transitions` 退到 `1.329x`，checksum 全部一致，已回退。结论是 loop-tail 不能靠解释器相邻 handler 拼接解决；后续应做 compiler-level phi/register-write 重排或更完整的 hot-loop lowering。

继续验证过 `Move2` / `AddIntI` 后接 `Jmp` 的 next-jump dispatch peephole：profile 中 `gcd_batch` opcode steps 从约 `3.43M` 降到约 `2.77M`，`binary_search` 从约 `11.88M` 降到约 `10.68M`，说明它确实减少了动态 `Jmp` dispatch；但普通 release 默认正式样本退到 `0.823x`，checksum 全部一致，已回退。结论是当前不能把局部 tail jump 拼接作为默认优化。

继续验证过 `ForLoopI` static step fact：compiler 把 static range step value 记录到 `PerfForLoopFact`，VM 热路径只读 index/end 两个寄存器，不再每轮读取 step register；但默认正式样本退到 `0.821x`，checksum 全部一致，已回退。结论是 `ForLoopI` 的下一步不应做单点 operand-read 微调，而应转向 loop-carried register rewrite 或完整 hot-loop lowering。

当前已落地 pair compare-test 的第一版：

- `TestEqIntI2 A B C`: 固定用于 condition-context false branch；`A/B` 是两个 int register，`C` 的高/低 nibble 分别保存 `0..15` 的两个 RHS literal，语义为 `(rA == hi4 && rB == lo4)`。该 opcode 只在 `a == K && b == L`、两个变量均为非 cell local、facts 均确认为 `Int`、两个 literal 均在 `0..15` 时发射。VM handler 固定 value=false 时消费后继 `Jmp`；LLVM scalar block 已同步为两条 `icmp eq` 加一条 `and`。profile 显示 `state_machine_transitions` 的动态 `TestEqIntI` 从约 `1.11M` 降到约 `600K`，新增 `TestEqIntI2` 约 `377K`；结合 `BrNeIntI4`、连续 `Move` dispatch cleanup 和 template literal loop caching 后，该轮默认 VM geomean 为 `0.897x`。该形状有效但仍不是 `<0.5x` 主路径，后续需要更系统的 branch-chain lowering。

### Containers

当前阶段保留泛化 opcode：

- `NewList`
- `NewMap`
- `NewObject`
- `NewRange`
- `GetIndex`
- `SetIndex`
- `ListPush`
- `Len`
- `ToIter`
- `Contains`
- `SliceFrom`
- `MapRest`

当前已落地 Lua-style const string field specialization：

- `GetFieldK A B C`: `A = B[C_string]`
- `SetFieldK A B C`: `A[C_string] = B`

这两个 opcode 只覆盖 compiler facts 已确认的 `Map` / `Object` 目标和短字符串 literal key。它们不折叠 mutable map/object 语义，只是把 key 从运行时 register/fact 查询收进 opcode operand；VM 仍走现有 `get_index` / `set_index` runtime helper。LLVM straightline、callee eval、scalar facts 和 scalar block lowering 已同步支持。

验证结果：

- profile coverage: `GetFieldK:405733`、`SetFieldK:144990`。
- `GetIndex` 动态计数降到 `2674709`，`SetIndex` 动态计数降到 `1043867`。
- release 低样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh` 在多次复测中约 `1.142x` 到 `1.220x`，checksum 全部一致；因此只能把它视为减少泛化 index dispatch 的通用结构优化，不能声称已经带来稳定 wall-clock 大幅收益。

本轮新增 string-prefix + int-suffix map key specialization：

- `GetIndexStrI A B C`: `A = B[prefix + C]`
- `SetIndexStrI A B C`: `A[prefix + B] = C`

`prefix` 通过 `PerfKeyFact::string_int` 指向 const string pool，suffix 必须由 register facts 确认为 `Int`。compiler 只在 `Map` 目标和直接 template key 形状（如 `"n${i}"`、`"sku${id}"`）下发射；它不是 `two_sum_map` 专用 opcode，也不折叠 map 语义。VM 运行时直接用 `ShortStr::concat_int` / fallback string 形成 lookup key，并复用 typed string-map fast path；普通 `GetIndex` / `SetIndex` 仍保留给变量 key、object key、list/string index 和动态 unknown key。显式 native/AOT 当前仍可能因为既有 scalar block facts 缺口跳过 full-suite AOT，默认执行路径不受影响。

验证结果：

- static coverage 新增 `SetIndexStrI:3`，全 workload `instructions` 从 `1905` 降到 `1902`。
- profile-enabled 单样本显示 `two_sum_map` opcode steps 从约 `2.04M` 降到约 `1.84M`，`event_join_by_id` 从约 `3.78M` 降到约 `3.55M`；`two_sum_map` 的 string write source 从约 `400K` 降到约 `200K`。
- 正式默认 VM `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 为 `0.783x` vs Lua，checksum 全部一致，runner 输出 `AOT: disabled`。

另一个已验证后以窄条件默认开启的候选：

- `GetList A B C`: `A = B[C]`，其中 compiler facts 必须确认 `B` 是 `List` 且 `C` 是 `Int`。

泛化 `GetList` 曾覆盖很高，profile coverage 显示 `GetList:1219200`，并把 `GetIndex` 降到 `1455509`，但 release 低样本 geomean 退到约 `1.198x` / `1.216x`，即使 handler 改回 `try_get_known_list_index` 的无错误热路径仍无收益。因此当前不做泛化 list opcode 替换，而是只在 `PerfIndexFact` 已确认 `List<Int>` 且 key register facts 为 `Int` 时发射。最新 static coverage 为 `GetIndex:62`、`GetList:4`、`AddListInt:1`、`SubListInt:1`、`MidInt:2`；默认 VM 样本为 `0.783x`，checksum 全部一致。该形状覆盖通用 typed list indexed access、typed list accumulator access 和 integer midpoint，不针对某个 workload 名称。

现有 `GetIndex` 的 list fact fast path 仍保留：当 `PerfIndexFact` 已确认目标是 `List` 且 value kind 是 `Int` 时，VM 会先尝试 `TypedList::Int` backing 直读，避免每次 list read 再进入完整 `TypedList` element type match；运行时 backing 不匹配时仍回落原 `try_get_known_list_index` / `get_index` 语义。`ListPush` lowering 现在会把 pushed value kind 写回 list register fact，空 list 第一次 push 采用 pushed kind，后续 push 做 kind join，但不保留静态 `known_len`，避免循环内 push 产生错误长度 fact。

普通 indexed access 也已接入 direct-to-destination lowering：`let dst = target[key]` 或 `dst = target[key]` 可以把 `GetIndex` / `GetFieldK` / `GetList` 直接写入目标 register，避免 `GetIndex temp; Move dst temp`。这与外部 `map.get(map_value, key)` 的 direct-to-destination lowering 同属通用 register materialization 消除。

本轮保留了一个不新增 opcode 的 map miss fast path：known string key 读取空 `TypedMap::Mixed` 时，VM 直接返回 `Nil`，不再构造 `RuntimeMapKey` 并进入 generic lookup。非空 `Mixed` map 仍返回 `None` 让调用方走原 generic path，因此 object key、short string key 和 heap string key 的混合语义不变。profile 显示 `config_defaults_merge` 的 index metrics 为 `typed_map_direct:396428`、`generic_map_lookup:69429`；同环境 A/B 关闭该 fast path 时 `config_defaults_merge` 从 `1.204x` 退到 `1.383x`。最新正式默认 VM geomean 为 `0.783x`，此前 ordering compare-test hot arm cleanup 后为 `0.785x`，`MidInt` 后为 `0.792x`，`AddListInt` / `SubListInt` 后为 `0.797x`，`ConcatN` register-window 直写两轮为 `0.798x` / `0.793x`，checksum 全部一致，runner 输出 `AOT: disabled`；该优化覆盖 sparse map/default lookup shape，不针对某个 workload 名称。

本轮继续验证过 `GetIndex` 的 map fact micro fast-path：当 `PerfIndexFact` 已确认 target 是 `Map` 时，跳过当前 handler 开头“key 是短 List slice 描述”的罕见 probe。该候选不改变 bytecode、不增加 opcode，且 checksum clean；但默认正式样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 曾从 `0.819x` 退到 `0.828x`，已回退。结论是容器方向不能只靠 `GetIndex` arm 顺序微调，应转向更上层的 map loop/value elision、动态 key lowering 或系统性 hot-loop lowering。

后续再考虑 Lua-style integer key specialization：

- `GetI`
- `SetI`

这些是通用 key shape opcode，不是 workload-specific opcode。加入前必须先用 dynamic counters 证明 `GetIndex` / `SetIndex` 中整数 key 或 const string key 足够热。
当前新分桶已经把 `dynamic_register_key` 细分为 `dynamic_int_key`、`dynamic_short_string_key`、`dynamic_object_key` 和 `dynamic_other_key`。最新 profile 显示 `two_sum_map`、`histogram_group_count`、`log_parse_filter`、`inventory_reorder`、`event_join_by_id` 的 hot dynamic map key 基本都是 `dynamic_short_string_key`，`dynamic_int_key` 没有进入 Top-6；因此 `GetI` / `SetI` 暂不推进，容器方向应改看 dynamic short-string map lowering、map loop/value elision 或更系统的 hot-loop lowering。

### Strings

当前已落地：

- `ConcatN A B C`: 把连续寄存器 `B..B+C-1` 的值拼接到 `A`。

compiler 只在 template string parts 为 `3..=255` 时默认发射 `ConcatN`；2-part template 仍保留 `ConcatString`，单表达式 template 仍保留必要的 `ToString`。该 opcode 是通用 multi-register concatenation shape，用于减少多段 template 的重复 binary concat/materialization，不针对某个 workload 名称。VM fast path 对短 `ShortStr`/`Int` 结果走 `ShortStr`，general path 一次构造结果字符串；LLVM straightline/callee/scalar/subfunction lowering 已同步支持。循环内 template literal parts 现在会被 loop scalar const cache 预加载，避免 `"prefix${i}"` 每轮重复 `LoadString` prefix。`ConcatN` 的连续 parts register window 现在会直接接收 literal、arithmetic expression、`map.get` 和 indexed access 等表达式结果，避免先写临时寄存器再 `Move` 进窗口；template assignment 现在也会把 `ConcatString` / `ConcatN` 直接写入目标 local/register。后续 `MidInt` 又把两个 midpoint expression 收成单条 opcode，本轮 template direct-to-destination 再移除 13 条 template result `Move`，static coverage 因此降到当前 `instructions=1902` / `Move=300`。profile 显示 `log_parse_filter` opcode steps 从约 `4.26M` 降到约 `3.62M`、动态 `Move` 从约 `1.39M` 降到约 `757K`，`template_render_mix` opcode steps 从约 `860K` 降到约 `730K`、动态 `Move` 从约 `255K` 降到约 `125K`。默认 VM 样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=60 bash bench/run_workload_bench.sh` 中，最新 VM/Lua geomean 为 `0.783x`，但 `prime_trial_division`、`state_machine_transitions`、`config_defaults_merge`、`sliding_window_sum` 等解释器慢项仍未解决。

本轮验证过把 `PerfIndexTargetKind::String` 的 `GetIndex` 放到 VM dispatch 的 static fast path，与现有 list fast path 对齐，覆盖字符串 indexed-for/hash 这类通用形状；默认 VM 正式样本退到 `1.032x`，已回退。结论是字符串方向不应继续微调 `GetIndex` arm 顺序，应优先做更上层的 string iteration/value elision 或 layout 优化。

### Call / Return

- `Call`
- `CallDirect`
- `CallNamed`
- `Return`
- `Return0`
- `Return1`

`Return0` / `Return1` 已落地：compiler 对裸 `return;` 和隐式返回发 `Return0`，对单值 `return value` 发 `Return1`；旧 `Return A B` 保留给多返回值和手写/旧 artifact。VM dispatch 对 `Return1` 直接取单 slot，LLVM straightline/callee/scalar/subfunction lowering 使用统一 `return_count()` 语义。默认样本中该改动保持 checksum clean，收益很小，不能作为达标路径。

本轮新增的通用 register-write lowering 不增加 opcode，但会改变现有 opcode 的目标寄存器：

- `math.floor(Int-like)`：当参数可由 literal、local facts 和 Int arithmetic 静态证明为整数时，compiler 直接把参数表达式 lower 到目标 local。例如 `math.floor((lo + hi) / 2)` 直接生成写入目标的 `DivInt`，不再生成 `DivInt temp` 后接 `Move dst temp`。
- `map.get(map_value, key)`：当 `map` 是未被 shadow 的外部全局时，compiler 直接把 `GetFieldK` / `GetIndex` 写入目标 local，不再额外 materialize 临时寄存器。
- `target[key]`：普通 indexed access 也可直接写入目标 local；当 facts 确认 `List<Int> + Int key` 时会发射 `GetList`，当 facts 确认 Map/Object + const string key 时会发射 `GetFieldK`。

该优化不是 workload-specific opcode，也不改变 `math.floor` 对非 Int/dynamic 参数的语义；不满足 Int-like/static-global 条件时仍走原有 call/index 路径。反汇编显示全 workload 静态 `Move` 为 `343`，但默认 VM geomean 为 `0.897x`，说明它是通用清理，不是达标主路径。

继续验证过把 builtin method expression 也接入 direct-to-destination：`.len()`、`.starts_with()`、`.split()`、`.join()` 和 `.set()` 写入目标 local，避免 `method-op temp; Move dst temp`。该候选只把全 workload 静态 `instructions` 从 `1981` 降到 `1978`、`Move` 从 `341` 降到 `338`，但默认正式样本为 `0.825x`，低于回退后复验的 `0.814x`，已回退。结论是当前不应继续做低覆盖 method-result materialization 微调；call/string 方向需要更高层的 split/join/value elision 或 hot-loop lowering。

本轮验证过但未保留：

- `DivIntI A B sC`: 静态覆盖能把 `bench/workloads_business_algorithms.lk` 中 17 条 division literal lowering 成 immediate opcode，但默认 release 样本 VM/Lua geomean 退到约 `1.073x` / `1.078x`，没有改善 `binary_search`，因此已回退。

### Loop

当前短期实现：

- `ForLoopI`

`ForLoopI` 曾临时复用 `Extra = 62`，一次完成 `index += step`、边界判断和跳回，替代静态正/负 step range loop 尾部的 `AddInt + Jmp + 下轮 Cmp/Test`。当前 opcode 编号已随 7-bit 语义分区重排，`ForLoopI` 不再占用该历史槽位；该 opcode 仍已同步接入 VM dispatch、compiler control-flow facts 和 LLVM scalar lowering。

encoding 稳定后再补齐：

- `ForPrepI`

完整 loop opcode 组合对应 Lua 的数值 for loop 思路：初始化、更新、比较和跳回由 loop opcode 处理，避免热循环重复 `Cmp + Test + Add + Jmp`。当前阶段不继续补 `ForPrepI`，先把 `ForLoopI` 的收益和 encoding 迁移边界固定下来。

本轮验证过把 `ForLoopI` 继续拆成正/负 step 与 inclusive/exclusive 四个新 opcode，compiler 默认发新 opcode，旧 `ForLoopI` 保留给旧 artifact；VM-only 默认样本退到 `1.075x`，已回退。结论是：仅把 loop fact 的两个 bool 挪进 opcode shape 不足以改善当前 VM geomean，后续 loop 方向应优先做更系统的 hot-loop/native lowering 或 phi/register-write 消除。

## 迁移顺序

1. 继续当前少量专门 opcode 的通用优化：measurement、facts preservation、materialization elision、typed fast path、hot/cold helper；不要新增 benchmark-shaped opcode。
2. 增加 dynamic opcode histogram 和 key/index/write-source counters。
3. 7-bit opcode + metadata encoding 已完成基础迁移；`AddIntI` / `MulIntI` / `ModIntI`、`AddMulInt` / `Add2Int`、`AddListInt` / `SubListInt`、`MidInt`、`BrNil` / `BrNotNil`、`BrEqZeroInt` / `BrNeZeroInt`、`BrEqIntI4` / `BrNeIntI4`、`BrModEqZeroIntI4` / `BrModNeZeroIntI4` 已作为通用 operand-shape opcode 落地。
4. `BrTrue/BrFalse` 支持已接入但默认不启用；`GetFieldK/SetFieldK` 已作为 const string field operand-shape opcode 落地。typed compare-test 已增加 target-pc control-flow fact、cold fallback split，并补充 `TestEqIntI` / `TestNeIntI` immediate literal shape。`TestEqIntI2` 已作为 facts-confirmed pair compare-test 接入 VM/compiler/LLVM scalar block。`ConcatN` 已作为 3+ part template/multi-register concat opcode 落地，并同步修复 AOT scalar block 中 dynamic string-map `GetFieldK` 与 optional nil branch 的 correctness。`Return0/Return1` 已作为常见返回路径 opcode 落地。`Move2` 已接入普通 block 和 direct-call inline block。下一步优先做更系统的 branch-chain / hot-loop lowering，或重新评估 `GetI/SetI`。
5. 如果默认 `lk file.lk` 的 VM 执行路径要继续压到 geomean `< 0.5x`，优先做更系统的 hot-loop/native lowering 设计和解释器结构优化；native/AOT 只能作为显式性能路径（`LK_NATIVE_RUN=1` 或 `lk compile exe`）存在。解释器 opcode 迁移用于恢复扩展空间和降低通用 dispatch/materialization 成本，不应通过堆 workload-specific opcode 来追这个目标。

## 明确不做

- 不再复用 `Wide`、`Extra` 或其它保留槽硬塞新 opcode；`ForLoopI` 已经是历史临时例外。
- 不加 `SubIntI`；用 `AddIntI` 的 signed immediate 覆盖。
- 不加 `ListFoldAdd`、`MapValuesFoldAdd`、`HistogramInc` 等 workload-specific fused opcode。
- 不把函数指针跳表作为默认 dispatch 方案。
- 不在非 LLVM VM 解释器路径使用 `unsafe`。
