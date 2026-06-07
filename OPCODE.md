# Opcode 设计结论

## 结论

当前 `Opcode` 不是长期最优设计，但当前优化阶段应避免 benchmark-shaped 专门 opcode。短期继续用 compiler facts、lowering 消除、typed fast path 和 hot/cold 拆分推进；opcode 设计先作为后续迁移目标固定下来。

当前实现有一个明确的临时例外：`Opcode::ForLoopI` 复用了原 `Extra = 62` 槽位，用于静态正/负 step 的整数 range loop。它是为了验证 Lua-style numeric loop opcode 对真实 workload 的收益，不是长期 encoding 重构完成，也不应继续作为在 64-slot 空间里追加 opcode 的先例。

参考 Lua 后，推荐的长期方向是：

- 保留 register VM。
- 保留 32-bit fixed instruction。
- 删除当前指令内的 `InstrFormat` bits。
- 用 opcode metadata table 决定 instruction format、写寄存器行为、test 行为和 top 语义。
- 保留 `ExtraArg` 扩展槽。
- 只引入 Lua-style operand-shape specialization，不引入 workload-specific fused opcode。

## 当前问题

- 当前 opcode 只有 6 bit，`0..63` 已满。
- `InstrFormat` 占 3 bit，导致 opcode 空间小，`C` operand 也被压到 7 bit。
- `Extra` / `Wide` 已占末尾槽位，长期没有足够扩展空间。
- `GetIndex` / `SetIndex` 过于泛化，很多已知 facts 需要运行时反复查询。

因此，不应继续往当前 64 个 opcode 里硬塞新 opcode。`ForLoopI` 是已有收益验证的阶段性例外；后续需要 `AddIntI`、`GetI`、`Return0` 等 opcode 时，应先迁移 encoding，而不是继续复用 `Wide` 或其它保留槽。

## Lua 可借鉴点

Lua 的关键设计不是业务专用 opcode，而是 operand shape 进入指令形状：

- opcode 数量足够，format 不占 instruction bits。
- `OP_EXTRAARG` 用于大常量和扩展参数。
- `OP_ADDI`、`OP_ADDK` 表达 immediate/constant operand。
- `OP_GETI`、`OP_SETI`、`OP_GETFIELD`、`OP_SETFIELD` 表达整数 key 和 const string key。
- `OP_RETURN0`、`OP_RETURN1` 避免常见返回路径走泛化 return。
- `OP_FORPREP`、`OP_FORLOOP` 把数值 for loop 热路径压缩到专门 loop opcode。

LK 应借鉴这些通用 operand-shape opcode，而不是做 `ListFoldAdd`、`MapValuesFoldAdd` 这类 benchmark-shaped opcode。

## 推荐 encoding

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

配套 metadata：

```text
OpcodeInfo {
  format,
  writes_a,
  is_test,
  uses_top,
  sets_top,
}
```

format 由 `OpcodeInfo` 决定，不再写入 instruction bits。这样 opcode 空间变成 128，`C` 回到 8 bit，并且有 `k` bit 表示常量/翻转/测试极性等轻量 flag。

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

只保留一个整数 immediate opcode：`AddIntI A B sC`。`x -= 3` 和 step `-1` 都编译成 `AddIntI` 的负 immediate。不要加 `SubIntI`，它浪费 opcode，也容易制造方向 bug。

`AddK` / `SubK` / `MulK` / `DivK` / `ModK` 可以等动态 opcode histogram 证明收益后再加。

### Branch / Compare

表达式需要 bool 时保留：

- `CmpInt`
- `CmpNeInt`
- `CmpLtInt`
- `CmpLeInt`
- `CmpGtInt`
- `CmpGeInt`

控制流分支不应 materialize bool。长期可选两种方案：

- Lua-style compare opcode 作为 test，并约定下一条是 jump。
- 或显式 `BrEqInt` / `BrNeInt` / `BrLtInt` / `BrLeInt` / `BrGtInt` / `BrGeInt`。

当前阶段先不新增 branch opcode，继续用 facts-driven fused branch lowering 和 register-write 消除推进。

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

encoding 迁移后再考虑 Lua-style operand-shape specialization：

- `GetI`
- `SetI`
- `GetFieldK`
- `SetFieldK`

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
3. 设计并迁移 7-bit opcode + metadata table encoding，恢复 `ExtraArg` 扩展能力。
4. 只加入已被 counters 证明的 Lua-style operand-shape opcode，例如 `Return0/Return1`、`AddIntI`、`GetI/SetI`、`GetFieldK/SetFieldK`。
5. 如果 geomean `< 0.9x` 仍是目标，优先做 template JIT / native hot-loop lowering，而不是继续堆解释器 opcode。

## 明确不做

- 不在当前 64 opcode 空间里继续硬塞新 opcode；`ForLoopI` 已经是临时例外。
- 不加 `SubIntI`；用 `AddIntI` 的 signed immediate 覆盖。
- 不加 `ListFoldAdd`、`MapValuesFoldAdd`、`HistogramInc` 等 workload-specific fused opcode。
- 不把函数指针跳表作为默认 dispatch 方案。
- 不在非 LLVM VM 解释器路径使用 `unsafe`。
