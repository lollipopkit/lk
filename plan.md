# LK VM 彻底重构计划

## 目标

重写 LK VM 内核，而不是继续优化旧 VM。允许迁移期长期不可用，只要求每个已迁移
模块的 Rust 单测正确。

性能目标保持不变：最终 VM/Lua geomean `<= 1.10x`。中期不以 benchmark 作为阻塞，
benchmark 只在新 VM 闭环后恢复。

做完架构迁移后，才开始做 hot path 优化和 benchmark-specific opcode/fusion。

## 当前基线

- 最新性能基线记录在 `bench/README.md`。
- 当前已知主要成本来自旧数据模型和旧执行模型：大 `Val` enum、`Vec<Op>` + optional
  `code32` 双轨、opcode/packed/quickening 多执行路径、call 参数物化、container COW/clone、
  global/context 字符串映射。
- 继续减少几条 bytecode 或新增 benchmark-shaped fused op 不是主线。
- 当前 `Executor32` closure call 仍会为 callee 创建新 `Frame32`/`Vec<RuntimeVal>`；新 VM
  必须把这类 per-call allocation 移出热路径。

## 硬规则

- 允许破坏旧 bytecode、LKB、内部 API 和 CLI/bench/AOT 临时可用性。
- `plan.md` 只记录架构契约和执行顺序，不记录每轮做了什么。
- LLVM 外不新增 `unsafe`；现有 VM raw-pointer bridge 是清理对象。
- 不保留旧 VM 兼容层作为长期路径。
- 单文件不得超过 1500 行。
- 借鉴 Lua/参考 runtime 的低层设计，但不复制 Lua 的侵入式 GC header 或单 table 语义。

## 新架构

### 1. Runtime Value Model

- `Val` 收敛为 immediate + heap object：
  `Nil`、`Bool`、`Int(i64)`、`Float(f64)`、`ShortStr`、`Obj(HeapRef)`。
- `HeapValue` 承载 long string、list、map、closure、native callable、AOT callable、
  task/channel/stream/object、upvalue cell、VM error value。
- 函数类统一到 `Callable`，不再在 `Val` 顶层保留多种函数 variant。
- list/map 使用 typed backing：`Mixed`、`Int`、`Float`、`Bool`、必要 string/object backing；
  类型污染时 materialize 到 `Mixed`。
- `UpvalCell(RuntimeVal)` 是 closure 可变捕获的唯一共享单元；不可变捕获继续按值保存。
- `ErrorVal { message, trace }` 是语言级 raise/handler 之间传递的堆对象；VM 内部断言、
  越界、损坏状态仍返回 `anyhow::Error`。

### 2. Heap And GC

- `HeapRef(u32)` 是稳定句柄，不暴露对象地址，不依赖 Rust 引用跨指令存活。
- `HeapStore` 改为 slot heap：
  `slots: Vec<Option<HeapValue>>`、`marks: Vec<u8>`、`free_list: Vec<u32>`、
  `alloc_since_gc`、`gc_threshold`。
- `alloc/get/get_mut/len/is_empty` 保持当前语义；已回收或越界句柄通过 `None` 暴露给调用者。
- GC 策略采用 per-task stop-the-world 三色 mark-sweep。每个 runtime module/task 拥有自己的
  `HeapStore`，不做跨线程全局暂停。
- 默认 `gc_threshold` 从 1024 次 heap allocation 起步，后续可改为按 live heap 自适应增长；
  回收后重置 `alloc_since_gc`，空槽进入 `free_list`。
- 根集合由 executor 在 GC 触发点组装：
  `RuntimeModuleState32.globals`、共享 register stack 的 `0..stack_top` 活跃区、
  当前 executor captures、handler stack 中的错误值、`RuntimeExport32`/`RuntimeCallable32`
  持有的 shared state。
- 标记阶段必须递归展开所有 `HeapRef`：
  `TypedList::Mixed`、`TypedMap::Mixed/StringMixed`、`RuntimeObject.fields`、
  `Callable::Closure.captures`、`Callable::Runtime32` 关联 state、`UpvalCell`、`ErrorVal.trace`
  中未来可能持有的对象值。
- `TypedList::Int/Float/Bool/String` 和 `TypedMap::StringInt/StringFloat/StringBool` 不含
  `HeapRef`，只需标记容器对象本身。
- `Task/Channel/Stream/StreamCursor` 中由 `Arc` 管理的外部资源不参与 per-task tracing；
  只有其 `HeapValue` 槽位作为对象被标记。

### 3. Canonical Bytecode IR

- 新 runtime IR 是唯一的 `Vec<Instr32>`。
- 指令固定 32-bit，支持 `ABC`、`ABx`、`AsBx`、`Ax`、`sJ`，超限用 `EXTRA/WIDE`。
- 常量池拆成 typed const pool：int、float、string、heap value。
- `Op` 只允许短期作为 builder/debug 过渡结构；最终 runtime 不 match `Op` enum。
- 删除 workload-shaped opcode，例如 `ListFoldAdd`、`MapValuesFoldAdd`、`AddRangeCountImm`。
- 新增 VM 语义指令只进入 `Instr32`：
  `LoadCellVal(dst, cell)`、`StoreCellVal(cell, src)`、`TryBegin(catch_reg, catch_offset)`、
  `TryEnd`。
- `LoadCellVal`/`StoreCellVal` 只操作 `HeapValue::UpvalCell`。传入非 cell 对象是 VM 错误。
- `TryBegin`/`TryEnd` 只维护 VM handler stack，不承诺公开语言语法已经存在。

### 4. Compiler Pipeline

- 新链路：AST/HIR -> SSA/MIR facts -> register allocation -> `Instr32`。
- `PerformanceFacts` 是纯静态编译期产物，只由 SSA/MIR、type facts、escape/liveness 分析生成。
  Executor 不读取 facts，facts 也不是 profile-guided runtime feedback。
- `PerformanceFacts` 是布局决策源：register kind、container kind、escape、call shape、
  branch/test shape、move/clone 偏好、dead write。
- typed lowering 直接产出 typed IR 和 typed register plan，不再堆 peephole/fusion。
- register allocation 面向连续 frame window。
- facts 查不到或为 `Unknown` 时必须 emit 通用 fallback；只有确定的 Int/Float/Bool/container
  facts 才 emit typed opcode。
- dead write、move-preferred、container move 只能消除语义等价的写入或 clone，不能改变错误时机。

### 5. Execution Model

- 新 executor 只执行 `Instr32`。
- `RuntimeModuleState32` 持有共享寄存器栈：
  `stack: Vec<RuntimeVal>`、`stack_top: usize`。初始容量为 256，自然增长，不主动 shrink。
- `Executor32` 持有 `frame_base: usize`、`captures`、`pc`、`handler_stack` 和当前
  `RuntimeModuleState32`；执行路径不再持有独立 `Frame32`。
- register 读写映射为 `state.stack[frame_base + reg]`。
- call 前保存 caller `frame_base` 和 `stack_top`，在共享 stack 上分配 callee window：
  `new_base = stack_top`，按需 resize 到 `new_base + callee.register_count`，参数复制到 callee
  `r0..rN`。
- return 后把返回值写回 caller call window 的 callee slot，恢复 caller `frame_base` 和
  `stack_top`；弹帧不释放 capacity。
- closure/native/AOT 共用同一 call ABI，避免参数 `Vec<Val>` materialization。native 边界可借用
  caller stack slice；只有跨 runtime heap/module 边界才复制必要对象。
- typed arithmetic/comparison/branch 直接读写紧凑 `Val` 或 typed backing，不走 quickening。
- inline cache 只保留动态边界：call、global slot、map/list shape、access。
- `Frame32` 可短期保留为测试辅助或旧路径对照，但不能再作为新 executor 热路径的数据结构。

### 6. Closure And Upvalue

- closure capture 分为不可变捕获和可变/escaped 捕获。
- 不可变捕获按当前值复制到 closure captures，读取仍用 `LoadCapture`，无额外 indirection。
- 可变或 escaped 捕获在外层函数 prologue 装箱为 `HeapValue::UpvalCell(RuntimeVal)`，
  原 local slot 改为保存 cell 的 `RuntimeVal::Obj(HeapRef)`。
- 外层和内层后续读写该变量都 lowering 为 `LoadCellVal` / `StoreCellVal`。
- 多个 closure 捕获同一可变变量时必须共享同一个 cell；任何一个 closure 写入后，其他 closure
  和外层后续读取都看到新值。
- 非 escaped 且不可变的 local 不装箱，继续留在 frame window。
- capture analysis 先基于现有 free-var 与 escape/liveness 信息实现；如果无法证明不可变，
  默认装箱，优先正确性。

### 7. Error And Unwinding

- VM 内部错误和语言级 raise 分开处理。
- VM 内部错误包括非法 opcode、寄存器越界、类型断言失败、堆句柄损坏、native bridge 失败；
  这些继续返回 `anyhow::Error`，不进入语言级 handler。
- 语言级 `Raise` 先查 `handler_stack`：
  - 有 handler：构造 `HeapValue::ErrorVal`，恢复 handler 记录的 `stack_top`/`frame_base`，
    把错误值写入 `catch_reg`，跳到 `catch_pc` 继续执行。
  - 无 handler：保持当前 `anyhow::Error` 行为。
- `ErrorHandler` 记录 `catch_reg`、`catch_pc`、进入 try 时的 `frame_base` 和 `stack_top`。
- `TryBegin(catch_reg, catch_offset)` push handler；`TryEnd` pop 最近 handler。
- 公开语法后续参考 Swift 的 `try` 风格设计，但本阶段只建立 VM handler stack 和 lowering
  预留，不更新语言 spec。

### 8. Context And Globals

- 顶层局部默认是 frame/local slot，不再持续同步到 `VmContext`。
- `VmContext` 只负责 export、module-visible、native-visible 状态。
- global access 改为 slot/handle lookup，并支持 inline cache。
- `RuntimeExport32` 和跨模块 `RuntimeCallable32` 必须显式持有 shared runtime state；跨 heap 导入时
  复制普通 value，但 closure 导入转换为携带 source state 的 runtime callable。

## 执行顺序

1. 建立新 `Val`/`HeapValue`/`Callable`/typed container 模型及单测，包含 `UpvalCell` 和
   `ErrorVal`。
2. 建立 slot-based `HeapStore`、GC 标记遍历 helper、root collection helper 及单测。
3. 建立 `Instr32`、typed const pool、encoder/decoder/disassembler 单测，加入 cell/handler 指令。
4. 合并推进 compiler 和 executor：先建立最小 executor skeleton，再让 compiler 输出可运行
   `Instr32`。每迁移一个 feature，都同步扩展 compiler、executor 和端到端单测。
5. 最小可运行路径必须覆盖 literal/load、move、int/float arithmetic、branch、closure call、
   return。之后依次恢复 container、index、string、global、named call、native call。
6. 重构 shared stack call ABI，移除 closure call 中 per-call `Frame32`/`Vec` 分配。
7. 实现 closure upvalue cell，恢复可变捕获语义。
8. 实现 per-task STW GC 触发点和 root collection。
9. 实现 VM handler stack、`TryBegin`/`TryEnd`、`Raise` handler 路径。
10. 重构 global/context slot、typed container fast path。
11. 新 VM 的 call ABI、global slot、typed container、GC root、upvalue 全部验证后，再删除旧
    `Op` runtime、BC32、packed executor、quickening、旧 fused op。删除前旧路径只允许
    `#[deprecated]` 对照，不允许继续扩展。
12. 新 VM 闭环后恢复 CLI、coverage、bench、AOT、LKB，并更新 `bench/README.md` 性能记录。

## 测试策略

中期硬门槛：

- `cargo test -p lk-core --lib`
- 相关模块 targeted tests：value、heap/gc、IR、compiler、executor、container、call、closure、
  error handler。

大阶段边界：

- `cargo test -p lk-core`

允许中期失败：

- `cargo build --release -p lk-cli`
- `cargo run -p lk-cli -- coverage --runtime bench/workloads_business_algorithms.lk`
- `RUNS=3 EXTRA_RUNS=3 PROFILE_WORKLOADS=1 bench/run_workload_bench.sh`
- `cargo test --all-features --all-targets`

必须新增或保留的测试场景：

- `HeapStore` slot reuse、dangling ref 返回 `None`、GC 后 live object handle 稳定。
- GC root 覆盖 globals、stack、captures、list/map/object/callable/upval cell。
- recursive closure call 不随调用次数分配新 frame `Vec`。
- mutable closure capture：外层写入、闭包读写、多个闭包共享同一 cell。
- `Raise` 无 handler 仍返回 `anyhow::Error`；有 handler 时跳转并写入 `ErrorVal`。
- `PerformanceFacts` 只改变 emitted opcode，不改变 executor 行为。

最终收口时再恢复完整验证和 `bench/README.md` 性能记录。

## 禁止事项

- 不新增 benchmark-specific opcode/fusion。
- 不用旧 VM 局部 hack 掩盖新模型缺口。
- 不因为 CLI/bench 暂时不可用而跳过当前模块单测。
- 不把 `plan.md` 重新变成工作日志。
- 不把 `PerformanceFacts` 变成 runtime profiler 或 executor 依赖。
- 不为了实现 GC/upvalue/handler stack 在 LLVM 外引入 `unsafe`。
