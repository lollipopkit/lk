# LK vs Lua 性能差距分析

本文基于 `bench-lk-lua/` 的 8 个 micro-benchmark，以及当前 LK VM 调用、闭包、循环和集合实现，对 LK 与 Lua 的性能差距做方向性分析。结论重点：**当前主要差距不在 List/Map，而在函数/闭包调用热路径；闭包创建也偏重，但本轮 Closure Call benchmark 主要测的是“闭包调用”，不是“闭包创建”。**

本文只用于确定优化优先级，不应直接当作正式性能报告。正式引用前应重新运行 benchmark，并记录 LK commit、Lua 版本、机器型号、编译参数、runs、min / median / max / stddev，以及语言内计时和外部 wall-time 两种口径。

## 基准结果回顾

| Benchmark | LK (ms) | Lua (ms) | Ratio |
|-----------|---------|----------|-------|
| Empty Loop | 1 | 0.3 | 3.3x |
| Fibonacci Iterative | 26 | 6.0 | 4.3x |
| Fibonacci Recursive | 731 | 84 | 8.7x |
| Function Call | 6 | 0.8 | 7.5x |
| List Ops | 45 | 25.5 | 1.8x |
| Map Ops | 79 | 59.3 | 1.3x |
| String Concat | 38 | 40.0 | 0.95x |
| Closure Call | 26 | 1.1 | **23.6x** |

> 注：这些结果适合作为优化方向的证据。仓库内 `bench-lk-lua/README.md` 的 sample results 与本文表格来自不同运行批次，数值不应混用。若要作为正式性能报告，应以同一次 release build 和同一台机器的复跑结果为准。

## 基准解释边界

当前 Closure Call benchmark：

```lk
fn make_adder(n) { return |x| x + n; }

let adder = make_adder(1);
for _ in 1..=iters {
    acc = adder(acc);
}
```

闭包只创建 1 次，随后调用 100,000 次。因此 **23.6x 差距的主因应归因于闭包调用热路径，而不是闭包创建**。闭包创建中的环境 snapshot 和多次分配仍然是重要优化点，但需要单独的 “closure create only” benchmark 来量化。

## 根因分析

### 1. 函数/闭包调用热路径过重（最主要问题）

Function Call、Closure Call、Recursive Fibonacci 都直接受调用路径影响。当前调用路径虽然已有 positional fast path / inline cache，但一次用户函数调用仍可能包含下列成本：

```rust
// Call 指令热路径大致包含：
let func = regs[*rf as usize].clone();          // 可能触发 Arc 引用计数
let fun = closure.code.get_or_init(|| ...);    // OnceCell 检查
let frame_info = closure.frame_info();         // 元信息准备/缓存读取
let return_meta = CallFrameMeta { ... };       // 返回帧元信息

vm.exec_function_positional_fast(
    fun,
    args_slice,
    ctx,
    Some(Arc::clone(&closure.captures)),        // captures Arc clone
    Some(Arc::clone(&closure.capture_specs)),  // capture_specs Arc clone
    Some(cache),
    Some(return_meta),
)
```

进入 `exec_function_positional_fast` 后还需要：

1. 设置当前 VM / nested call 状态；
2. 建立 `CallFrame` / `FrameState`；
3. 准备或复用 `ClosureFastCache`；
4. 初始化/调整寄存器窗口；
5. 运行字节码；
6. 恢复调用方状态。

Lua 的普通 Lua 函数调用路径更紧凑，核心是栈上 `CallInfo`、`Proto`、栈顶调整和跳转执行。LK 这里的抽象层、`Arc`、元信息和 frame/cache 维护更多，因此 Function Call、Recursive Fibonacci 这类调用密集 benchmark 明显落后是合理现象。具体倍数需要以当前 commit 的复跑结果为准。

### 2. 闭包调用额外叠加 capture 成本（Closure Call 23.6x）

闭包调用相较普通函数调用还需要处理 captured values：

- 每次调用传递 `captures` / `capture_specs`；
- 热路径中存在 `Arc::clone`；
- `LoadCapture` 根据 capture index 取值；
- capture specs / frame capture 需要在 frame 层传播。

因此 Closure Call 不是简单的 “Function Call + 1 次加法”，而是多了 capture 元数据与闭包 frame 绑定成本。

需要修正的是：**不能直接说“闭包每次调用都会发生堆分配”**。当前实现已有 fast path 和 cache，很多结构可复用；更准确的表述是：

> 闭包调用热路径存在多处引用计数、frame/cache 准备、capture 传播和通用 VM 调用开销；即使不一定每次堆分配，也明显重于 Lua 的闭包调用。

### 3. 闭包创建：环境 snapshot 和结构分散仍是重要问题

`MakeClosure` 当前会创建 `ClosureValue`，并在普通/packed 路径中执行：

```rust
let captured_env = Arc::new(ctx.snapshot());
```

这会复制 `VmContext` 的大量字段，例如 globals、locals、slot values、import context、struct/type 信息等。对于闭包创建密集型程序，这是严重成本。

`ClosureValue` 结构也比较分散，包含多个 `Arc` 字段：

- `params`
- `named_params`
- `body`
- `env`
- `upvalues`
- `captures`
- `capture_specs`
- `default_funcs`
- `code`
- `call_env_pool`
- `layout`
- 多个 frame/default/named-param cache

Lua 闭包通常只分配 closure object，并保存必要 upvalue 引用；不会 clone 整个全局/局部上下文。因此闭包创建需要优化，但它不是当前 Closure Call benchmark 的主要测量对象。

### 4. Val 类型大小影响所有寄存器和集合操作

| 类型 | 大小 | 说明 |
|------|------|------|
| LK `Val` | 通常约 24 bytes | `Arc<str>` 等胖指针变体 + enum tag |
| Lua `TValue` | 通常 16 bytes | value union + type tag |

24B vs 16B 意味着：

- 64B cache line 中可容纳的值更少；
- 寄存器文件、List、Map value 存储的缓存局部性更差；
- 大量 `Val::clone()` 会复制更大的 enum 值，并可能触发 `Arc` 引用计数。

这会带来全局性的潜在影响，但具体幅度需要用当前 target 上的 `std::mem::size_of::<Val>()`、cache miss、allocation profile 和指令级 profile 验证。该项属于高侵入优化，不应优先于调用热路径。

### 5. 空循环 / ForRange 开销

LK 的 range loop 已经有 `ForRangeState` 使用裸 `i64` 保存状态，但每次迭代仍需把迭代变量写回寄存器：

```rust
assign_reg(frame_raw, regs, idx, Val::Int(state.current));
state.current += state.step;
```

Lua 的 numeric for 直接在栈槽中操作数值，指令更少，布局更紧凑。Empty Loop 差距说明 LK 的循环控制和寄存器写回仍有优化空间。具体倍数需要以当前 commit 的复跑结果为准。

### 6. List/Map 操作差距较小

LK 列表使用 `Arc<Vec<Val>>`，push 时通过 `Arc::make_mut()` 做 copy-on-write：

```rust
Op::ListPush { list, val } => {
    match &mut regs[*list as usize] {
        Val::List(arc) => {
            Arc::make_mut(arc).push(pushed_val);
        }
        ...
    }
}
```

独占引用时 `Arc::make_mut()` 不会深拷贝整个 Vec，所以 List/Map 的结果（1.3-1.8x）相对合理。主要差距来自：

1. `Val` 较大；
2. `Val::clone()` / `Arc` 引用计数；
3. Map key 使用 `Arc<str>`；
4. HashMap 层抽象与 Lua table 的专用结构差异。

## 建议优化优先级

### P0: 函数/闭包调用 fast path（最高 ROI）

这是最高 ROI。目标是为最常见场景提供极简路径：

条件：

- 无 named params；
- 无默认参数；
- 参数数量固定且匹配；
- closure body 已编译；
- 普通 positional call；
- call target inline cache 命中。

优化方向：

1. **避免 clone callee `Val`**：Call 指令尽量通过引用读取 callee，只在必要时 clone。
2. **减少 `Arc::clone(captures/capture_specs)`**：frame 可借用或持有稳定引用，避免每次引用计数增减。
3. **精简 frame setup**：复用寄存器窗口和 frame metadata，减少 `CallFrameMeta` / `FrameInfo` 维护成本。
4. **避免每次调用重新初始化 nested IC vec**：普通 monomorphic call 应复用或绕过不需要的 `access_ic` / `index_ic` / `global_ic` / `call_ic` / `for_range_ic` 容器。
5. **减少参数搬运和 clone**：固定参数数量且寄存器布局兼容时，直接建立 callee 寄存器窗口，避免先收集 `positional_values` 再写回。
6. **绕过 named-param machinery**：普通 positional call 不应创建 named slots / named seed。
7. **缓存已解析函数指针和 frame info**：IC 命中时直接进入 compiled `Function`。

预期收益应通过新增 empty function call、recursive empty function 和 closure call no capture benchmark 验证。当前可以判断这是最高优先级，但不应在未测量前承诺具体加速倍数。

### P1: 闭包 capture/upvalue 表示优化

当前 `MakeClosure` 的 `ctx.snapshot()` 应优先消除或缩小范围：

1. **只捕获实际引用的变量**：依赖已有 `CaptureSpec`，避免复制整个 `VmContext`。
2. **共享不可变原型数据**：`params` / `named_params` / `body` / `capture_specs` 应尽量来自 proto，不在每个 closure 中重复分配。
3. **扁平化 ClosureValue**：合并多个小 `Arc`，减少 cache miss 和引用计数。
4. **upvalue chain / cell**：像 Lua 一样保存变量 cell 或 upvalue 引用，而不是环境快照。

注意：该项对闭包创建密集程序收益可能很高；对当前 Closure Call benchmark 的收益取决于调用路径是否还每次传播/clone captures。需要用 `closure create only`、`closure call no capture`、`closure call one capture` 三组 benchmark 拆开量化。

### P2: ForRange 直接 i64 快槽

1. 让 range loop 的迭代变量在 loop body 能直接读 i64 快槽；
2. 必要时才 materialize 为 `Val::Int`；
3. 减少 `assign_reg` 和 enum tag 写回。

该项有明确优化价值，但不是纯机械修改。需要先定义 materialize 边界：普通表达式读取、闭包 capture、调试/错误路径、引用逃逸和集合写入都必须看到正确的 `Val::Int`。建议先做编译期分析，只在迭代变量未逃逸、未被 capture、且 loop body 的使用点可证明为 int-only 时启用。

### P3: Val 16B / 紧凑寄存器文件（难度高）

候选方案：

1. **紧凑 tagged union**：把常见 immediates 和指针 payload 压到更紧凑的布局；
2. **寄存器 SoA**：`tags: Vec<Tag>` + `payloads: Vec<u64>`；
3. **字符串表示调整**：避免 `Arc<str>` 胖指针直接撑大 `Val`，可考虑 intern id 或 thin pointer wrapper。

该项会影响几乎所有代码，建议放在 P0/P1/P2 之后。推进前应先加入一个小的布局检查测试或诊断输出，确认当前平台上的 `Val`、集合元素和寄存器文件实际尺寸。

### P4: 集合和字符串局部优化

List/Map 当前不是最大瓶颈，但可以继续优化：

1. 热路径减少 `Val::clone()`；
2. 小字符串/intern key 优化；
3. Map/list mutation 场景识别独占引用，减少 `Arc` refcount 操作；
4. 针对 homogeneous int list/map 做专用快速路径。

## 建议补充 benchmark

为了避免误判，应增加下列用例：

| Benchmark | 目的 |
|-----------|------|
| empty function call | 测纯调用成本 |
| add function call | 当前 Function Call 的拆分对照 |
| closure create only | 单独量化 `MakeClosure` / `ctx.snapshot()` |
| closure call no capture | 区分 closure dispatch 和 capture 读取 |
| closure call one capture | 当前 `make_adder` 的对照 |
| recursive empty function | 放大 frame setup 成本 |
| named argument call | 单独测 named-param machinery |
| native function call | 区分 RustFunction 与 VM closure 调用 |

脚本层也建议输出：

- LK commit；
- `lua -v`；
- runs；
- min / median / max / stddev；
- 外部 wall-time 版本和语言内计时版本。

## 总结：修正后的判断

| 差距来源 | 主要影响 | 优化难度 | 优先级 |
|----------|----------|----------|--------|
| 函数/闭包调用热路径 | Function Call、Closure Call、Recursive Fib | 中 | P0 |
| capture/upvalue 与闭包创建 | Closure Call、Closure Create | 中 | P1 |
| ForRange 写回和循环控制 | Empty Loop、迭代类 benchmark | 中 | P2 |
| `Val` 24B → 16B / 寄存器布局 | 全局影响 | 高 | P3 |
| List/Map 局部优化 | 集合 benchmark | 中 | P4 |

**最高 ROI 的优化不应表述为“闭包每次调用堆分配”或单纯“消除闭包环境克隆”。更准确的 P0 是：为普通函数/闭包 positional call 建立更薄的 fast path，减少 `Val` clone、`Arc` clone、frame/cache 元信息准备和 named-param 通用路径开销。**
