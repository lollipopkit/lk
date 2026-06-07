# Opcode 设计结论

## 结论

当前 `Opcode` 不是长期最优设计，但当前优化阶段应避免 benchmark-shaped 专门 opcode。短期继续用 compiler facts、lowering 消除、typed fast path、hot/cold 拆分，以及 Lua-style operand-shape opcode 推进。

当前 encoding 基础迁移已完成：32-bit instruction 仍保留，但 opcode 从 6 bit 扩到 7 bit，`InstrFormat` 不再写入 instruction bits，而是由 `OpcodeInfo` metadata 决定；`ABC` 的 `C` operand 恢复为 8 bit。`Bx` 访问接口当前仍保持 `u16`，因为常量池、globals、captures 和 LLVM/AOT 索引路径仍按 `u16` 组织；完整 17-bit `Bx` 使用需要后续单独迁移索引类型。

`Opcode::ForLoopI` 仍是一个历史临时例外：它复用了原 `Extra = 62` 槽位，用于静态正/负 step 的整数 range loop。它是为了验证 Lua-style numeric loop opcode 对真实 workload 的收益，不是继续追加 opcode 的先例。后续新增 opcode 应基于当前 7-bit encoding，并且只做通用 operand-shape specialization。

参考 Lua 后，推荐的长期方向是：

- 保留 register VM。
- 保留 32-bit fixed instruction。
- 删除当前指令内的 `InstrFormat` bits。
- 用 opcode metadata table 决定 instruction format、写寄存器行为、test 行为和 top 语义。
- 保留 `ExtraArg` 扩展槽。
- 只引入 Lua-style operand-shape specialization，不引入 workload-specific fused opcode。

## 当前状态

- opcode encoding 已有 7 bit，`0..127` 可用；当前已定义 opcode 仍是 `0..63`。
- `InstrFormat` 已由 `OpcodeInfo` metadata 决定，不再占 instruction bits。
- `ABC` 的 `C` operand 已恢复 8 bit。
- `Bx`/`sBx` 当前 API 仍是 `u16`/`i16` 兼容面；要完整使用 17-bit payload，需要同步扩大 const/global/capture/function 索引类型。
- `Extra` / `Wide` 仍需要重新整理为长期 `ExtraArg` 语义。
- `GetIndex` / `SetIndex` 过于泛化，很多已知 facts 需要运行时反复查询。

因此，后续可以新增 opcode，但必须满足两点：一是基于当前 7-bit encoding，不复用保留槽；二是只加入 counters 证明过的通用 operand-shape opcode。当前已加入 `AddIntI` / `MulIntI` / `ModIntI`、`BrNil` / `BrNotNil`、typed compare-test，以及 `GetFieldK` / `SetFieldK`；后续候选包括 compare-branch、`GetI` / `SetI`、`Return0` / `Return1`，不能加入 workload-specific fused opcode。

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
- `LoadNil`
- `LoadBool`
- `LoadIntI`
- `LoadK`
- `LoadKX`
- `ExtraArg`

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

已实现一个整数 immediate opcode：`AddIntI A B sC`。`x -= 3` 和 step `-1` 都编译成 `AddIntI` 的负 immediate。不要加 `SubIntI`，它浪费 opcode，也容易制造方向 bug。

当前 `AddIntI` 已接入 VM dispatch、compiler lowering、LLVM straightline/scalar lowering 和动态 opcode histogram。profile 显示它确实覆盖 `gcd_batch`、`order_score_pipeline`、`config_defaults_merge` 等 workload 的 small-int add/sub hot path；但 release 低样本 geomean 没有改善，因此它不是 `<0.5x` 的主路径。

本轮新增同一类通用 immediate arithmetic opcode：

- `MulIntI A B sC`: `A = B * sC`
- `ModIntI A B sC`: `A = B % sC`

compiler 只在 RHS 是 `i8` 范围内的 int literal，且 register facts 确认 LHS 为 `Int` 时默认发射；`ModIntI` 不对 literal `0` 发射，VM 和 LLVM lowering 仍保留 divisor-zero 防护。它们已接入 VM dispatch、compiler lowering、LLVM straightline/callee/scalar/subfunction lowering 和 tests。profile 显示覆盖是真实通用数值 shape：`MulIntI` 出现在 `binary_search:1440409`、`stock_max_profit:1080000`、`gcd_batch:160000`，`ModIntI` 出现在 `log_parse_filter:782684`、`inventory_reorder:478001`、`config_defaults_merge:435000`、`route_permission_check:360002`。普通 release 低样本 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=0 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh` geomean 为 `1.139x`，checksum 全部一致；其中 `gcd_batch` 和 `stock_max_profit` 有较高噪声，仍需要正式多样本复验。

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

当前 compare-test VM hot path 继续按 Lua-style “test opcode consumes following jump” 形状工作，但 compiler control-flow facts 会记录后继 `Jmp` patch 后的 absolute target pc，避免执行时重复读取、校验后继 `Jmp` 并重新计算 relative target；非 Int/Int fallback 已拆到 cold helper，避免动态比较和错误构造污染 typed hot helper。默认样本验证命令 `RUN_AOT=0 RUNS=3 EXTRA_RUNS=5 BENCH_PROGRESS=0 BENCH_TIMEOUT=30 bash bench/run_workload_bench.sh` 的最新 geomean 为 `1.253x`，checksum 全部一致；这仍不是 `<0.5x` 达标结果，只能说明 typed compare-test 方向可继续作为通用 control-flow 优化推进。

长期可选两种方案：

- Lua-style compare opcode 作为 test，并约定下一条是 jump。
- 或显式 `BrEqInt` / `BrNeInt` / `BrLtInt` / `BrLeInt` / `BrGtInt` / `BrGeInt`。

当前阶段不把 `BrTrue/BrFalse` 作为默认 lowering；继续用 facts-driven fused branch lowering、nilness branch、typed compare-test 和 register-write 消除推进。下一步如果继续做 branch opcode，应避免 unknown/dynamic 比较走 compare-test；`Br*Int A rhs sBx` 放不进当前 32-bit `AsBx` instruction，需要设计 rhs register fact、复用 `k` bit，或继续采用 Lua-style compare-test + next jump 形状。

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

另一个已验证但默认关闭的候选：

- `GetList A B C`: `A = B[C]`，其中 compiler facts 必须确认 `B` 是 `List` 且 `C` 是 `Int`。

该 opcode 动态覆盖很高，profile coverage 曾显示 `GetList:1219200`，并把 `GetIndex` 降到 `1455509`。但 release 低样本 geomean 退到约 `1.198x` / `1.216x`，即使 handler 改回 `try_get_known_list_index` 的无错误热路径仍无收益；当前通过 `ENABLE_GET_LIST_LOWERING = false` 保留实现但不默认发射。后续若继续做 list 方向，应优先改现有 `GetIndex` list fact fast path 或 list backing layout，而不是仅把它拆成新 opcode。

后续再考虑 Lua-style integer key specialization：

- `GetI`
- `SetI`

这些是通用 key shape opcode，不是 workload-specific opcode。加入前必须先用 dynamic counters 证明 `GetIndex` / `SetIndex` 中整数 key 或 const string key 足够热。

### Call / Return

- `Call`
- `CallDirect`
- `CallNamed`
- `Return`
- `Return0`
- `Return1`

当前代码已经用 `Return 0 0 0` 表达 empty return；长期可迁移为 `Return0`，减少 dispatch arm 内的泛化处理。

### Loop

当前短期实现：

- `ForLoopI`

`ForLoopI` 临时复用 `Extra = 62`，一次完成 `index += step`、边界判断和跳回，替代静态正/负 step range loop 尾部的 `AddInt + Jmp + 下轮 Cmp/Test`。当前 release 低样本 geomean 从 `0.989x` 降到 `0.971x`，checksum 全部一致。该 opcode 已同步接入 VM dispatch、compiler control-flow facts 和 LLVM scalar lowering。

encoding 稳定后再补齐：

- `ForPrepI`

完整 loop opcode 组合对应 Lua 的数值 for loop 思路：初始化、更新、比较和跳回由 loop opcode 处理，避免热循环重复 `Cmp + Test + Add + Jmp`。当前阶段不继续补 `ForPrepI`，先把 `ForLoopI` 的收益和 encoding 迁移边界固定下来。

## 迁移顺序

1. 继续当前少量专门 opcode 的通用优化：measurement、facts preservation、materialization elision、typed fast path、hot/cold helper；不要新增 benchmark-shaped opcode。
2. 增加 dynamic opcode histogram 和 key/index/write-source counters。
3. 7-bit opcode + metadata encoding 已完成基础迁移；`AddIntI` / `MulIntI` / `ModIntI` 和 `BrNil` / `BrNotNil` 已作为通用 operand-shape opcode 落地。
4. `BrTrue/BrFalse` 支持已接入但默认不启用；`GetFieldK/SetFieldK` 已作为 const string field operand-shape opcode 落地。typed compare-test 已增加 target-pc control-flow fact 和 cold fallback split。下一步优先做 `GetI/SetI`、`Return0/Return1`，或继续 facts-confirmed compare-branch 直接 lowering。
5. 如果目标继续压到 geomean `< 0.5x`，优先把 native/AOT、template JIT 或 hot-loop lowering 做成主性能路径；解释器 opcode 迁移用于恢复扩展空间和降低通用 dispatch/materialization 成本，不应通过堆 workload-specific opcode 来追这个目标。

## 明确不做

- 不再复用 `Wide`、`Extra` 或其它保留槽硬塞新 opcode；`ForLoopI` 已经是历史临时例外。
- 不加 `SubIntI`；用 `AddIntI` 的 signed immediate 覆盖。
- 不加 `ListFoldAdd`、`MapValuesFoldAdd`、`HistogramInc` 等 workload-specific fused opcode。
- 不把函数指针跳表作为默认 dispatch 方案。
- 不在非 LLVM VM 解释器路径使用 `unsafe`。
