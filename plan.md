# LK VM 彻底重构计划

## 目标

重写 LK VM 内核，而不是继续优化旧 VM。允许长期不可用，只要求每个已迁移模块的
Rust 单测正确。

性能目标保持不变：最终 VM/Lua geomean `<= 1.10x`。中期不以 benchmark 作为阻塞，
benchmark 只在新 VM 闭环后恢复。

做完架构迁移后,才开始做hot path优化和 benchmark-specific opcode/fusion。

## 当前基线

- 最新性能基线记录在 `bench/README.md`。
- 当前已知主要成本来自旧数据模型和旧执行模型：大 `Val` enum、`Vec<Op>` + optional
  `code32` 双轨、opcode/packed/quickening 多执行路径、call 参数物化、container COW/clone、
  global/context 字符串映射。
- 继续减少几条 bytecode 或新增 benchmark-shaped fused op 不是主线。

## 硬规则

- 允许破坏旧 bytecode、LKB、内部 API 和 CLI/bench/AOT 临时可用性。
- `plan.md` 只记录架构契约和执行顺序，不记录每轮做了什么。
- LLVM 外不新增 `unsafe`；现有 VM raw-pointer bridge 是清理对象。
- 不保留旧 VM 兼容层作为长期路径。
- 单文件不得超过 1500 行。
- 借鉴 Lua/参考 runtime 的低层设计，但不复制 Lua GC 或单 table 语义。

## 新架构

### 1. Runtime Value Model

- `Val` 收敛为 immediate + heap object：
  `Nil`、`Bool`、`Int(i64)`、`Float(f64)`、`ShortStr`、`Obj(HeapRef)`。
- `HeapValue` 承载 long string、list、map、closure、native callable、AOT callable、
  task/channel/stream/object。
- 函数类统一到 `Callable`，不再在 `Val` 顶层保留多种函数 variant。
- list/map 使用 typed backing：`Mixed`、`Int`、`Float`、`Bool`、必要 string/object backing；
  类型污染时 materialize 到 `Mixed`。

### 2. Canonical Bytecode IR

- 新 runtime IR 是唯一的 `Vec<Instr32>`。
- 指令固定 32-bit，支持 `ABC`、`ABx`、`AsBx`、`Ax`、`sJ`，超限用 `EXTRA/WIDE`。
- 常量池拆成 typed const pool：int、float、string、heap value。
- `Op` 只允许短期作为 builder/debug 过渡结构；最终 runtime 不 match `Op` enum。
- 删除 workload-shaped opcode，例如 `ListFoldAdd`、`MapValuesFoldAdd`、`AddRangeCountImm`。

### 3. Compiler Pipeline

- 新链路：AST/HIR -> SSA/MIR facts -> register allocation -> `Instr32`。
- `PerformanceFacts` 成为布局决策源：register kind、container kind、escape、call shape、
  branch/test shape。
- typed lowering 直接产出 typed IR 和 typed register plan，不再堆 peephole/fusion。
- register allocation 面向连续 frame window。

### 4. Execution Model

- 新 executor 只执行 `Instr32`。
- frame 是连续 register window；call 参数、返回值、临时值都在 window 内完成。
- typed arithmetic/comparison/branch 直接读写紧凑 `Val` 或 typed backing，不走 quickening。
- inline cache 只保留动态边界：call、global slot、map/list shape、access。
- closure/native/AOT 共用同一 call ABI，避免参数 `Vec<Val>` materialization。

### 5. Context And Globals

- 顶层局部默认是 frame/local slot，不再持续同步到 `VmContext`。
- `VmContext` 只负责 export、module-visible、native-visible 状态。
- global access 改为 slot/handle lookup，并支持 inline cache。

## 执行顺序

1. 建立新 `Val`/`HeapValue`/`Callable`/typed container 模型及单测。
2. 建立 `Instr32`、typed const pool、encoder/decoder/disassembler 单测。
3. 让 compiler 输出 `Instr32`，先覆盖表达式、分支、循环、call、return、container。
4. 写新单一 executor，逐步恢复 value、IR、compiler、executor、container、call 单测。
5. 删除旧 `Op` runtime、BC32、packed executor、quickening、旧 fused op。
6. 重构 call ABI、global/context slot、typed container fast path。
7. 新 VM 闭环后恢复 CLI、coverage、bench、AOT、LKB。

## 测试策略

中期硬门槛：

- `cargo test -p lk-core --lib`
- 相关模块 targeted tests：value、IR、compiler、executor、container、call。

大阶段边界：

- `cargo test -p lk-core`

允许中期失败：

- `cargo build --release -p lk-cli`
- `cargo run -p lk-cli -- coverage --runtime bench/workloads_business_algorithms.lk`
- `RUNS=10 EXTRA_RUNS=20 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`
- `cargo test --all-features --all-targets`

最终收口时再恢复完整验证和 `bench/README.md` 性能记录。

## 禁止事项

- 不新增 benchmark-specific opcode/fusion。
- 不用旧 VM 局部 hack 掩盖新模型缺口。
- 不因为 CLI/bench 暂时不可用而跳过当前模块单测。
- 不把 `plan.md` 重新变成工作日志。
