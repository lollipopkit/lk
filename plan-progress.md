# LK VM 重构交接进度

本文是当前重构任务的 handoff 文档，用于保证其他 agent 接手时保持 continuity。`plan.md` 仍然是唯一契约和路线图；本文只记录当前快照、已验证事实、未完成风险和下一步执行顺序。不要把日常流水账写回 `plan.md`。

## 当前总体状态

当前工作已经从“减少几条 bytecode”的方向转向 VM 数据模型和执行模型重构。新的核心路径已经包含 runtime value model、slot heap、GC roots、`Instr32`/`runtime32`、共享执行栈、upvalue/cell、handler 栈和 runtime callable 调用 ABI。这个方向是对的，但还没有完成，不能宣称 VM 重构闭环。

当前允许长期不可用；接下来只需要保证被修改代码对应的单测正确。不需要维持旧实现兼容，不需要迁移前测试全部通过。

## 已经完成或已推进

- `core/src/val/runtime_model.rs` 已成为新 runtime value model 的主入口，并 re-export `HeapRef`、`HeapStore`。
- `core/src/val/runtime_model/heap.rs` 引入 slot-based `HeapStore`，承载 heap ref、heap object 和 heap GC 测试。
- `RawList`、`RawMap` 已从 `HeapValue` 删除。
- `TypedList::Legacy`、`TypedMap::Legacy` 已删除。
- `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime` 已删除。
- `core/src/val/runtime_model/legacy.rs` 已删除；旧 `Val` 容器隔离到 `Val::List` / `Val::Map` 兼容变体。
- `TypedMap::string_entries_into_heap`、`TypedList::from_runtime_slice` 已加入，减少容器构造时的额外 materialization。
- `ReturnValues32` 已引入，`run_function_inner` 返回结构化 return values；小返回值使用 inline variants，超过 4 个才走 `Vec`。
- closure named args 的同 runtime 调用路径已改为直接写入 callee frame，避免先构造 ordered `Vec<RuntimeVal>`。
- `call_runtime_callable32_runtime_named` 已改为直接 copy 到 callee frame，不再中转 positional/named `Vec`。
- `call_runtime_value32_runtime_with_receiver` 已加入，trait method receiver-prefix 调用不再在 raw closure 快路径构造 `receiver + args` `Vec`。
- FullState native 调用已使用 stack-inline `InlineNativeArgs32`。
- `CallableValue::Native { function_index, arity }` 已删除。
- `Opcode32::LoadNative` 现在直接构造 `CallableValue::RuntimeNative32`。
- `VmContext` 的 legacy `Val` symbol table 已隔离到 `core/src/vm/context/legacy.rs`。
- `VmContext` 现在持有 `legacy: LegacyValContext` 和 `runtime_globals`，旧 `legacy_*` API 只是 delegation。
- `context/core_methods.rs` 已改用新的 list/map materialization helper。
- trait method runtime dispatch 已改用 `call_runtime_value32_runtime_with_receiver`。
- `core/src/vm/gc32.rs` 已加入，包含 `GcRoots32`、runtime export collection、`RuntimeModuleState32::gc_roots` 和 callable GC 测试。
- `core/src/vm/exec32/gc.rs` 已加入 executor root scanning 和 `maybe_collect_garbage`。
- `core/src/vm/runtime32.rs` 已加入并从 `vm/mod.rs` 导出。
- `RuntimeCallable32`、`RuntimeModuleState32`、`RuntimeExport32`、`NativeRuntime32`、`NativeArgs32`、`NativeFunction32`、`NativeEntry32` 等 runtime 类型已从 `ir32.rs` 移出。
- `RuntimeModuleState32` 已包含共享 `stack: Vec<RuntimeVal>` 和 `stack_top: usize`，初始容量为 256。
- `RuntimeCallable32` 通过 `Arc<Mutex<RuntimeModuleState32>>` 共享 runtime state。
- `compiler32` 已拆出 `core/src/vm/compiler32/builder.rs`，包含 register allocation、emit、jump patching、const pool、return 和 finish helper。
- `exec32.rs` 已大幅拆分，当前主文件约 664 行。
- `exec32` 下新增或使用了 `arithmetic.rs`、`callable_ops.rs`、`cell.rs`、`container.rs`、`globals.rs`、`program.rs`、`gc.rs` 等分块。
- program/import glue 已移到 `exec32/program.rs`。
- dynamic runtime raw closure 调用已改为使用 active shared stack window，不再把 entry frame 强制 reset 到 0。
- `NewList` 已改为读取 borrowed register slice 并走 `TypedList::from_runtime_slice`；只有 closure capture 仍使用 owned range。
- `core/src/val/legacy_registers.rs` 已删除，容器 copy metrics helper 已内联到 `Val::copy_container_value_with_metrics`。

## 最近已通过的验证

以下命令在最近重构过程中通过过，可作为接手后的局部回归起点：

```sh
cargo fmt --all -- --check
cargo check -p lk-core -p lk-stdlib
cargo test -p lk-core val::runtime_model -- --nocapture
cargo test -p lk-core val::values -- --nocapture
cargo test -p lk-core vm::exec32::return_values -- --nocapture
cargo test -p lk-core vm::exec32::exec32_tests -- --nocapture
cargo test -p lk-core vm::compiler32::tests -- --nocapture
cargo test -p lk-core vm::runtime32 -- --nocapture
cargo test -p lk-core vm::gc32 -- --nocapture
```

不要把这些结果等同于完整 hard gate。

## 最近已验证（本 session）

- `core/src/val/legacy_registers.rs` 已删除。
- `TypedList::OwnedRuntime` / `TypedMap::OwnedRuntime` 已从 runtime model 删除。
- Priority 4 审计完成：`grep` 确认 exec32 内没有任何 `legacy_*` 调用，old eval（`expr/expr_impl.rs`）、AOT（`llvm/runtime.rs`）和 tests 是唯一调用方。
- `TypedMap::string_entries_no_heap()` 已加入 `val/runtime_model.rs`：不需要 `&mut HeapStore`，直接迭代 StringMixed/StringInt/StringFloat/StringBool/Mixed。
- `runtime_positional_args` 已改为两阶段借用：Phase 1 immutable borrow 直接处理 `TypedList::Mixed/Int/Float/Bool`，避免克隆整个 `HeapValue::List`；Phase 2 只在 String 时才克隆。
- `runtime_named_args` 已改为 immutable borrow 调用 `string_entries_no_heap()`，避免克隆整个 `TypedMap`。
- `write_named_args32_to_frame_from_typed_map` 已加入 `vm/exec32/named_call.rs`：直接从 `&TypedMap` 写到 callee frame。
- 全量 `cargo test -p lk-core --lib`：**561 passed, 0 failed**。

## 当前不能宣称完成的部分

- legacy `Val` 仍存在，并且仍有多条 bridge 路径。
- 旧 `Val` 容器仍以 `Val::List` / `Val::Map` 兼容变体存在；它们已被隔离在 runtime model 外。
- `VmContext::legacy_*` API 仍存在，只是被隔离为 delegation。exec32 已确认无任何 `legacy_*` 调用。
- dynamic method helper 中命名参数仍会生成 `Vec<(Arc<str>, RuntimeVal)>`（已减少为单次克隆，TypedMap 不再双重克隆）。进一步消除需要改变 `call_runtime_value32_runtime_named` 等函数签名，工作量较大，`write_named_args32_to_frame_from_typed_map` 已就位等待接入。
- `RuntimePositionalArgs::Prefixed` 在 native 或 fallback 路径仍可能 materialize。
- old LKB、CLI、AOT、LLVM、benchmark、stdlib 全链路还没有按新 runtime model 收口。
- `core/src/vm/compiler32.rs` 和 `core/src/vm/compiler32/tests.rs` 仍接近 1500 行限制，需要继续拆分。
- `runtime_model.rs`、`context.rs` 仍较大，需要继续按领域边界拆分。

## 下一步严格优先级

1. ✅ 已完成：删除 `core/src/val/legacy_registers.rs`。

2. ✅ 基本完成：dynamic callable / method 路径收紧。
   - `runtime_positional_args` 和 `runtime_named_args` 已改为两阶段借用，避免 HeapValue/TypedMap 克隆。
   - `TypedMap::string_entries_no_heap()` 已加入。
   - `write_named_args32_to_frame_from_typed_map` 已加入为 direct frame writer。
   - 剩余：`Vec<(Arc<str>, RuntimeVal)>` 在动态 named call 路径仍存在（需改变 `call_runtime_value32_runtime_named` 签名，工作量大，推迟）。

3. ✅ 已完成：删除 `OwnedRuntime` 过渡层。
   - legacy `Val` container conversion 已移出 runtime model。
   - `HeapValue::List/Map` 的 typed backing 不再携带旧 heap snapshot。

4. ✅ 已完成：`VmContext::legacy_*` API 审计。
   - exec32 确认无任何 `legacy_*` 调用；`expr/expr_impl.rs`、`llvm/runtime.rs` 和 tests 是唯一合法调用方。

5. ✅ 已完成：所有文件 ≤ 1500 行。

6. ✅ 已完成：`cargo test -p lk-core --lib` → 571 passed, 0 failed。

7. ✅ 已完成：CLI / LKB / AOT / benchmark / website 文档链路。
   - CLI `cargo build -p lk-cli` 已修复 4 条编译错误：
     - `execute32_with_ctx` 返回类型由 `Val` 改为 `Program32Result`，CLI 的 `(Val, VmContext)` 类型标注和 `display_string` 调用已删除。
     - `Program32Result::first_return_is_nil()` 和 `display_first_return()` 已加入 `core/src/vm/exec32.rs`。
     - `format_runtime_val`、`format_typed_list`、`format_typed_map`、`format_map_key` helper 已加入 `exec32.rs`，实现 `RuntimeVal` 的递归格式化。
     - `ModuleResolver::resolve_module` 不存在；CLI 的 `register_package_modules` 已改为 `resolve_runtime_module`。
     - `repl.rs` 同步修复：`Val::Nil` match 和 `display_string` 调用已替换为 `first_return_is_nil()` / `display_first_return()`。
   - 验证：`cargo build -p lk-cli` → Finished，无 warning；`cargo run -p lk-cli -- examples/fib.lk` → `55`。
   - 验证：`cargo test -p lk-cli` → 29 passed, 0 failed。
   - 验证：`cargo test -p lk-core --lib` → 571 passed, 0 failed。
   - 验证：`bun run build`（website）→ built in 1.06s。
   - 验证：`RUNS=3 bash bench/run_workload_bench.sh` → 完成，AOT compile disabled（预期），解释器路径正常输出 benchmark 表格。

## 本 session 完成的工作（bench workload 修 bug）

### 已修复

1. **"String has no method 'split'"**  
   在 `core/src/vm/context/core_methods.rs` 中新增：
   - `dispatch_string_builtin_method`：处理 `split`/`starts_with`/`ends_with`/`contains`/`trim`，返回 `RuntimeVal`
   - `dispatch_list_builtin_method`：处理 `join`，返回字符串 `RuntimeVal`
   - `extract_string_arc` / `make_string_val` 辅助函数
   - 两者均已接入 `call_method_positional_runtime` 和 `call_method_named_runtime`，在 map dispatch 之后、trait dispatch 之前执行

2. **"Call return base 132 must match call window base 4"**  
   `core/src/vm/exec32.rs` 中 `Opcode32::Call` handler：
   - 删除 `if instr.a() != instr.b() { bail!(...) }` 检查
   - 改为只使用 `instr.a()`（8 bit A 字段）作为 call window base；B 字段不再用于 Call

3. **`Instr32` ABC 格式修改（OPCODE 7→6 bit，B 7→8 bit）**  
   `core/src/vm/ir32.rs` 改动：
   - `OPCODE_BITS: u32 = 6`（原 7），释放 1 bit 给 B 字段
   - `B_MASK: u32 = 0xFF`（原 0x7F），B 字段现在可以编码寄存器 0-255
   - `C_SHIFT: u32 = Self::B_SHIFT + 8`（原 `B_SHIFT + 7`），修复 B/C 字段 1 bit 重叠 bug
   - `BX_MASK: u32 = 0x7FFF`（原 0x3FFF），ABx 格式 BX 扩展为 15 bit
   - `AX_MASK: u32 = (1 << 23) - 1`（原 22 bit）
   - `MAX_ABX_CONSTS: usize = 1 << 15`（原 14 bit）
   - `Extra = 62`（原 126），`Wide = 63`（原 127）；`abx` debug_assert 改为 `bx < (1 << 15)`
   - **注意**：此格式变更使所有 `.lkb` 预编译文件失效，但 bench 流程是即时编译 `.lk`，不加载 `.lkb`，无实质影响

### 当前状态（本 session 继续）

- `cargo test -p lk-core vm::ir32 -- --nocapture` → 10 passed。
- `cargo build --release -p lk-cli` → Finished。
- `./target/release/lk bench/workloads_business_algorithms.lk` → 15/15 workload 全部通过，`log_parse_filter` checksum=`916180`，后续 `cart_pricing_rules` / `route_permission_check` / `inventory_reorder` / `fraud_rule_scoring` 均已到达并通过。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo test -p lk-core --lib` → 571 passed。
- `RUN_AOT=0 RUNS=3 EXTRA_RUNS=3 bash bench/run_workload_bench.sh` → checksum 全部一致；当前 Instr32 VM 几何平均 `17.648x` vs Lua，性能明显退化，不能宣称性能目标完成。

### 已修复：长顶层程序寄存器累计导致 Instr32 C 字段截断

排查过程中确认 `Instr32` 的 B 字段修复已生效，但 ABC 格式仍是 `A=8/B=8/C=7`。`log_parse_filter` 之前在 release 下表现为 `Add expected numbers or strings, got Int and Obj` 或 `got Nil`，debug 下会触发 `Instr32::abc` 的 `c < 128` 断言。根因是 `compiler32` 没有回收语句级临时寄存器，长顶层 benchmark 文件会把二元操作 RHS 推到 C 字段不可编码范围。

当前修复：
- `Stmt::Expr`、`Stmt::Assign`、`Stmt::CompoundAssign` 结束后回收语句临时寄存器。
- 简单 `let name = ...` 和 `define` 使用稳定 local slot，再把表达式结果 move 进去，表达式临时寄存器不再永久占用。
- `Stmt::Block` 恢复进入 block 前的 locals/cell locals，并在非 return 路径回收 block 内局部寄存器。
- 保留顶层 `let` 的 global export 语义；函数体仍可读取顶层 `workload_filter` 等配置。
- `dynamic_numeric_binary` 的错误上下文现在包含 opcode、pc 和寄存器号，后续定位寄存器读错会更直接。

### 下一步

1. 性能回归是当前最大风险：当前 runner 结果比 2026-05-22 README 快照慢很多，优先从 compiler32 生成的顶层全局/局部布局、runtime builtin call path、string/list/map method materialization 和 shared stack 调用路径查。
2. 如果继续碰到 ABC C 字段上限，不能只扩 B 字段；需要 either 更系统的寄存器分配/回收，或设计 `Wide`/扩展 ABC 编码。
3. `bench/README.md` 已记录这次 `RUN_AOT=0` 的 Instr32 VM 快照，后续优化必须以 checksum 一致和 runner 输出为准。

### 本 session 继续：legacy 收口第一刀

用户明确要求“先把 legacy 全部移除，迁移到新架构”。本轮先切 active runtime 面：

- 删除 `core/src/val/legacy_registers.rs` 和 `val::legacy_registers` 模块。
- 旧模块里唯一生产调用方 `copy_container_value_for_register_with_metrics` 已内联迁移为 `Val::copy_container_value_with_metrics`，保留原 metrics 计数语义。
- `lk-stdlib` 不再直接访问 `OwnedRuntimeList/OwnedRuntimeMap` 的 `values`/`heap`/`entries` 字段；runtime/test helper 遇到 `OwnedRuntime` 改为明确 invariant，`list.get` 临时走 `TypedList::runtime_values_into_heap`。
- `exec32` import、runtime callable heap copy、native runtime helper 遇到 `OwnedRuntime` 改为 `bail!`，不再把 legacy snapshot 当作新 VM 跨 heap copy 源。
- 删除 `OwnedRuntimeList::copy_into_typed_list` 和 `OwnedRuntimeMap::copy_into_typed_map`，保留目前仍被旧 `Val` 边界使用的 `runtime_values_into_heap` / `entries_into_heap` 过渡入口。

验证：

- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::runtime_model -- --nocapture` → 15 passed。
- `cargo test -p lk-core val::values -- --nocapture` → 9 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `cargo test -p lk-core --lib` → 562 passed。
- `cargo fmt --all` 已执行；之前 `cargo fmt --all -- --check` 发现两处格式差异。

该小节的 `OwnedRuntime` 剩余项已被下一小节继续处理；以最新小节为准。

### 本 session 继续：删除 `OwnedRuntime` 过渡层

上一刀后继续把 legacy 容器从 runtime model 中剥离：

- 删除 `TypedList::OwnedRuntime` / `TypedMap::OwnedRuntime` enum variant。
- 删除 `OwnedRuntimeList` / `OwnedRuntimeMap` 类型和 `core/src/val/runtime_model/legacy.rs`。
- 删除 `TypedList::from_legacy_values` / `TypedMap::from_legacy_entries`，新 runtime model 不再接收无 `HeapStore` 的旧 `Val` 容器快照。
- 旧 `Val::list` / `Val::map` 改为 `Val::List` / `Val::Map` 兼容变体，暂时把 old evaluator / parser / serializer 需要的旧容器留在 `Val` 边界内，不再污染 `HeapValue::List/Map` 的 typed backing。
- `Val` 的 clone、display、serde、type inference、template constant folding、AOT runtime `to_iter` 和 test bridge 已跟随处理 `Val::List/Map`。
- exec32、stdlib、GC mark、runtime native display、import/callable copy、named call direct writer 中所有 `OwnedRuntime` 分支已删除。
- 搜索确认 `OwnedRuntime`、`from_legacy_values`、`from_legacy_entries`、`runtime_model/legacy`、`legacy_registers` 均无源码残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::runtime_model -- --nocapture` → 14 passed。
- `cargo test -p lk-core val::values -- --nocapture` → 9 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `cargo test -p lk-core --lib` → 561 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo build --release -p lk-cli` → passed。
- `./target/release/lk bench/workloads_business_algorithms.lk` → 15/15 workload completed；`log_parse_filter` checksum=`916180`。

当前剩余 legacy 面：

- `Val::List` / `Val::Map` 是旧 evaluator/parser/serde 的隔离兼容层；它们不再进入 runtime model typed container。最终还需要删除 old `Val` 容器 API 或迁移 old eval/AOT 到 `RuntimeVal + HeapStore`。
- `core/src/val/runtime_bridge.rs` 仍是 `#[cfg(test)]` 测试桥。
- `VmContext::legacy_*` 和 old expr/AOT 仍存在。

### 本 session 继续：收窄 `VmContext` 和 trait legacy 表面积

继续按“先把 legacy 全部移除，迁移到新架构”的方向切掉无生产价值的旧接口：

- 删除 `VmContext::legacy_get_value`、`legacy_define`、`legacy_define_const` alias；调用方统一改到唯一剩余写入口 `legacy_set`。
- 删除 `LegacyValContext::define_const` 和内部 `const_globals` 状态；const 赋值约束已经由 type checker 覆盖，旧 Val symbol table 不再维护第二套 const 规则。
- `TraitMethodValue` enum 已删除；trait impl registry 现在直接保存 `RuntimeVal` callable。
- 删除 `TypeRegistry::get_legacy_method`；Executor32 trait dispatch 直接调用 runtime callable，old `Expr::eval_with_ctx` 碰到 runtime trait method 会明确报错。
- `Val::display_string` 不再尝试通过旧 trait method fallback 调用 `to_string` / `show`，只保留内建格式化。
- 搜索确认 `TraitMethodValue`、`get_legacy_method`、`legacy_define`、`legacy_define_const`、`legacy_get_value`、`const_globals` 均无源码残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core vm::context -- --nocapture` → 4 passed。
- `cargo test -p lk-core --lib` → 561 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。

当前剩余 legacy 面：

- `VmContext::legacy_get` / `legacy_set` / `legacy_assign` / `legacy_remove` 仍被 old eval 和 LLVM AOT runtime 使用；exec32 仍无 `legacy_*` 调用。
- `Val::List` / `Val::Map` 仍是 parser/serde/old eval/AOT 的隔离兼容层。
- `core/src/val/runtime_bridge.rs` 已在后续小节删除。
- `compiler32/support.rs` 的旧 literal conversion 命名已在下一小节收口为 AST literal conversion。

### 本 session 继续：compiler32 literal conversion 去 legacy 命名

这一步不改变 lowering 语义，只把仍被误称为 legacy runtime conversion 的 AST literal 转换收口到新 VM 术语：

- `const_heap_value_from_legacy` 改名为 `const_heap_value_from_literal`。
- `const_runtime_value_from_legacy` 改名为 `const_runtime_value_from_literal`。
- `compiler32_lowers_legacy_list_and_map_values_to_heap_consts` 改名为 `compiler32_lowers_literal_list_and_map_values_to_heap_consts`。
- compiler32 的错误文案从 `legacy value` 改为 `AST literal value` / `AST value kind`。
- 搜索确认 `const_heap_value_from_legacy`、`const_runtime_value_from_legacy`、`legacy_list_and_map`、`legacy value`、`legacy value kind` 在 `core/src/vm` 下均无残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 561 passed。

当前剩余 legacy 面：

- `VmContext::legacy_get` / `legacy_set` / `legacy_assign` / `legacy_remove` 仍被 old eval 和 LLVM AOT runtime 使用；exec32 仍无 `legacy_*` 调用。
- `Val::List` / `Val::Map` 仍是 parser/serde/old eval/AOT 的隔离兼容层，阻止 `Val` 完全收敛到 plan.md 要求的 immediate + heap object。
- `core/src/val/runtime_bridge.rs` 已在后续小节删除。

### 本 session 继续：测试 helper 去 legacy 命名

- `Val::legacy_runtime_native32` 改名为 `Val::runtime_native32_for_test`。
- 对应测试改名为 `runtime_native32_for_test_is_stored_as_callable_heap_value`。
- `runtime_bridge.rs` 的测试转换路径同步改用新 helper。
- 搜索确认 `legacy_runtime_native32` 无源码残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core val::values -- --nocapture` → 9 passed。
- `cargo test -p lk-core val::runtime_bridge -- --nocapture` → 3 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。

### 本 session 继续：删除 `val/runtime_bridge.rs`

- 删除 `core/src/val/runtime_bridge.rs`。
- 删除 `val::runtime_bridge` test-only module 和 `pub(crate) use runtime_bridge::*`。
- `stmt::if_let_test` 不再依赖 crate 级 legacy bridge；需要的 `Val -> RuntimeVal` fixture conversion 已收进测试文件本地 helper `test_val_to_runtime_value`。
- 搜索确认 `runtime_bridge`、`legacy_val_to_runtime_val`、`legacy_runtime_val_to_val` 无源码残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core stmt::if_let_test -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 558 passed。

当前剩余 legacy 面：

- `VmContext::legacy_get` / `legacy_set` / `legacy_assign` / `legacy_remove` 仍被 old eval 和 LLVM AOT runtime 使用；exec32 仍无 `legacy_*` 调用。
- `Val::List` / `Val::Map` 仍是 parser/serde/old eval/AOT 的隔离兼容层，阻止 `Val` 完全收敛到 plan.md 要求的 immediate + heap object。

### 本 session 继续：反序列化新增 runtime 直达路径

为减少新代码继续依赖 `Val::List` / `Val::Map`，`core::val::de` 增加直接输出 `RuntimeVal + HeapStore` 的 API：

- 新增 `RuntimeDecodedValue { value: RuntimeVal, heap: HeapStore }`。
- 新增 `from_json_str_runtime` / `from_yaml_str_runtime` / `from_toml_str_runtime`。
- 新增 `parse_runtime_with_format` 和 `parse_runtime_with_format_into_heap`。
- JSON/YAML/TOML array/object 现在可直接解码为 `HeapValue::List` / `HeapValue::Map`，并通过 `TypedList::from_runtime_values` / `TypedMap::from_runtime_entries` 保留 typed backing。
- `stdlib/src/runtime_native.rs` 删除本地重复的 JSON/YAML/TOML -> `RuntimeVal` 转换逻辑，改为复用 `de::parse_runtime_with_format_into_heap`。
- 新增 `val::de_test` 覆盖 runtime JSON 解码不生成 `Val` 容器，以及 TOML format override 解码到 heap map。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core val::de_test -- --nocapture` → 24 passed。
- `cargo test -p lk-stdlib json::tests -- --nocapture` → 2 passed。
- `cargo test -p lk-stdlib yaml::tests -- --nocapture` → 2 passed。
- `cargo test -p lk-stdlib toml::tests -- --nocapture` → 2 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 560 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。

当前剩余 legacy 面：

- 旧 `from_json_str` / `from_yaml_str` / `from_toml_str` 和 `parse_with_format` 仍返回 `Val`，因此仍会构造 `Val::List` / `Val::Map`。
- `VmContext::legacy_get` / `legacy_set` / `legacy_assign` / `legacy_remove` 仍被 old eval 和 LLVM AOT runtime 使用；exec32 仍无 `legacy_*` 调用。
- `Val::List` / `Val::Map` 仍是 parser/serde/old eval/AOT 的隔离兼容层，阻止 `Val` 完全收敛到 plan.md 要求的 immediate + heap object。

### 本 session 继续：删除源码 legacy 命名和旧反序列化入口

继续按“先把 legacy 全部移除，迁移到新架构”收口：

- `TypedList::to_legacy_values` / `TypedMap::to_legacy_entries` 改名为 `to_val_values` / `to_val_entries`，runtime model 不再暴露 legacy 命名。
- `core/src/vm/context/legacy.rs` 改名为 `core/src/vm/context/val_bindings.rs`。
- `LegacyValContext` 改名为 `ValBindingContext`。
- `VmContext::legacy_get` / `legacy_set` / `legacy_assign` / `legacy_remove` 改名为 `get_val_binding` / `set_val_binding` / `assign_val_binding` / `remove_val_binding`。
- old eval、AOT replay 和相关 tests 已同步到新的 `Val` binding API；exec32 仍不使用该表。
- 清理 `core/src` / `stdlib/src` 里剩余的 `legacy` 文案和测试命名；当前源码搜索无 `legacy` / `Legacy` / `LEGACY` 残留。
- 删除旧 `from_json_str` / `from_yaml_str` / `from_toml_str` / `parse_with_format`，不再提供返回 `Val` 容器的结构化反序列化入口。
- 删除 `Val` 的 `From<serde_json::Value>` / `From<serde_yaml::Value>` 结构化转换；`Val::try_from` 只保留 scalar serde 转换，array/object 必须走 runtime decoder。
- `Val: Deserialize` 仅保留 scalar AST literal 支撑，不再作为 JSON/YAML/TOML 结构化数据入口。
- `val::de_test` 已改为只验证 `RuntimeVal + HeapStore` 反序列化。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core val::de_test -- --nocapture` → 8 passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 39 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `cargo test -p lk-core --lib` → 537 passed。

当前剩余旧模型面：

- `Val::List` / `Val::Map` 仍存在，主要服务 AST/parser 常量、old `Expr::eval_with_ctx`、pattern tests、type checker、LLVM AOT runtime collections 和少量 `Val` 运算测试。
- `VmContext` 仍有 `ValBindingContext`，用于 old eval/AOT replay；名称已经去 legacy，但数据模型仍不是最终 runtime globals/slots。
- 下一步应继续迁移或删除 old eval/AOT 对 `Val` 容器的依赖，才能把 `Val` 收敛到 plan.md 要求的 immediate + heap object。

### 本 session 继续：收窄 `Val` 隐式容器转换

继续减少旧 `Val` 容器的生产入口，防止新代码通过宽泛 `Into<Val>` 隐式生成 `Val::List` / `Val::Map`：

- 删除生产代码里的 `impl From<Vec<T>> for Val`。
- 删除生产代码里的 `impl From<HashMap<_, _, _>> for Val` 和 `impl From<hashbrown::HashMap<_, _, _>> for Val`。
- old eval / pattern 中仍需要构造 `Val::Map` 的位置改为显式 `Val::string_map_from_hashmap(HashMap<String, Val>)`。
- 单测中需要构造旧 `Val` 容器的地方改为 `#[cfg(test)]` helper：
  - `Val::test_list_from_values`
  - `Val::test_string_map_from_hashmap`
  - `Val::test_from`
  - `TestIntoVal`
- 这些 helper 只在 `cfg(test)` 下可用，生产 API 不再接受 `Vec` / `HashMap` 隐式转 `Val`。
- 搜索确认 `core/src` / `stdlib/src` 没有 `From<Vec`、`From<HashMap`、`Val::from(vec!...)`、`Val::from(HashMap...)` 残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 39 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core stmt::if_let_test -- --nocapture` → 15 passed。
- `cargo test -p lk-core --lib` → 537 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。

当前剩余旧模型面：

- `Val::List` / `Val::Map` enum variants and explicit constructors still exist.
- AST/parser 常量、old `Expr::eval_with_ctx`、pattern matching/type checking、LLVM AOT runtime collections and old `Val` arithmetic tests still rely on explicit `Val` containers.
- 下一步应优先把 AST literal representation 和 old eval/AOT collection paths 迁到 `RuntimeVal + HeapStore` 或删除旧路径。

### 本 session 继续：AST/parser literal 脱离旧 `Val` 容器

继续减少前端和 compiler32 重新制造 `Val::List` / `Val::Map` 的入口：

- `Expr::fold_constants` 不再把 `Expr::List` / `Expr::Map` 常量折叠成 `Expr::Val(Val::List/Map)`。
- parser AST 测试期望值已从 `Expr::Val(Val::list/map/test_*)` 改为 `Expr::List` / `Expr::Map`。
- match / destructuring 中手写 AST fixture 已改为 `Expr::List` / `Expr::Map`，不再通过 `Expr::Val(Val::List/Map)` 构造匹配输入。
- `compiler32` 新增 `const_heap_value_from_expr_literal`，可直接把静态 `Expr::List` / `Expr::Map` lowering 为 `LoadHeapConst`。
- `compiler32_lowers_literal_list_and_map_values_to_heap_consts` 已改为使用 AST literal 输入，覆盖新 lowering 路径。
- 删除 `value_can_fold_into_val_container`，常量折叠层不再判断哪些旧 `Val` 容器可被折入。
- 搜索确认 `core/src` 中没有 `Expr::Val(Val::list`、`Expr::Val(Val::map`、`Expr::Val(Val::test` 残留；剩余 `Val::test_*` 命中集中在旧 `Val` 自身测试和 old eval/if-let fixture。
- 搜索确认 `core/src` / `stdlib/src` 源码无 `legacy` / `Legacy` / `LEGACY` 命名残留。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core ast::ast_test -- --nocapture` → 32 passed。
- `cargo test -p lk-core vm::compiler32::tests::compiler32_lowers_literal_list_and_map_values_to_heap_consts -- --nocapture` → 1 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core expr::match_test -- --nocapture` → 11 passed。
- `cargo test -p lk-core stmt::destructuring_test -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 537 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` 仍存在，显式构造器仍被 old eval、旧 `Val` 运算、LLVM AOT runtime collections 和部分 `ValBindingContext` 测试 fixture 使用。
- `VmContext` 的 `ValBindingContext` 仍服务 old eval/AOT replay；active exec32 路径仍不应回到它。
- 下一步优先删除或迁移 old eval 对 `Val` 容器的求值路径，随后处理 LLVM AOT runtime collection 的旧 `Val` handle 模型。

### 本 session 继续：删除 compiler32 的旧 `Val` 容器 literal 后门

AST literal 已经迁到 `Expr::List` / `Expr::Map` 后，继续把新 VM 编译链路里的兼容入口关掉：

- `Compiler32::lower_val` 不再接受 `Val::List` / `Val::Map` 作为可 materialize 的 AST literal。
- `const_heap_value_from_literal` 只保留 long string 转 heap const，不再把旧 `Val` 容器转成 `ConstHeapValue32::List/Map`。
- `const_runtime_value_from_literal` 不再递归接受旧 `Val` 容器。
- `const_heap_value_from_expr_literal` 仍直接支持 `Expr::List` / `Expr::Map`，静态 list/map literal 的 `LoadHeapConst` 路径保留在新 AST 表达式模型上。
- 新增 `compiler32_rejects_val_container_literals`，防止后续重新把 `Expr::Val(Val::List/Map)` 接回 compiler32。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core vm::compiler32::tests::compiler32_rejects_val_container_literals -- --nocapture` → 1 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 63 passed。
- `cargo test -p lk-core ast::ast_test -- --nocapture` → 32 passed。
- `cargo test -p lk-core expr::match_test -- --nocapture` → 11 passed。
- `cargo test -p lk-core stmt::destructuring_test -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 538 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` 仍存在于旧 `Val` 模型本身，以及 old eval、旧 `Val` 运算、LLVM AOT runtime collections 和部分 `ValBindingContext` fixture。
- compiler32 active literal path 已不再接受旧 `Val` 容器；后续应继续迁移 old eval 和 LLVM AOT collection handle 模型，最后删除 `Val::List` / `Val::Map` enum variants。

### 本 session 继续：收窄 PerformanceFacts/type checker 的旧容器识别

继续关闭新编译/分析链路中把旧 `Val` 容器当正常 list/map 输入的路径：

- `PerfValueKind::from_val` 不再通过 `Val::as_list()` / `Val::as_map()` 把旧 `Val` 容器识别为 `PerfValueKind::List/Map`。
- PerformanceFacts 中 list/map kind 仍来自 `SsaRvalue::List` / `SsaRvalue::Map`，即新 AST/SSA literal 形态。
- 新增 `perf_value_kind_from_val_keeps_containers_unknown`，确认 scalar/string 仍可识别，但旧 `Val` list/map 返回 `Unknown`。
- type checker 的 `check_literal` / `infer_val_type` 不再把 `Val::List` / `Val::Map` 推导成 `Type::List/Map`，改为旧模型边界的 `Type::Any`。
- 新增 `test_val_container_literals_are_not_typed_as_containers`，确认 `Expr::Val(Val::List/Map)` 不再被 type checker 当作容器 literal。
- 删除因此不再使用的 `infer_list_element_type`。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core vm::analysis::tests::perf_value_kind_from_val_keeps_containers_unknown -- --nocapture` → 1 passed。
- `cargo test -p lk-core vm::ssa -- --nocapture` → 9 passed。
- `cargo test -p lk-core typ::type_checker::tests::test_val_container_literals_are_not_typed_as_containers -- --nocapture` → 1 passed。
- `cargo test -p lk-core typ::type_checker::tests -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 540 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` 仍存在于旧 `Val` 本体、old eval、旧 `Val` 运算测试、LLVM AOT runtime collections 和部分 fixture。
- 新 VM 前端/编译/静态分析主路径已经不再把 `Expr::Val(Val::List/Map)` 当作容器 literal；后续继续处理 old eval 和 LLVM AOT collection handle。

### 本 session 继续：一次性收口 type system 的旧 `Val` 容器边界

按用户要求，不再只改单个小入口；本次把 type system 相关的旧 `Val` 容器识别一起收掉：

- `Val::dispatch_type()` 不再把 `Val::List` / `Val::Map` 暴露为 `Type::List/Map`，改为 `Type::Any`。
- `Type::validate()` 不再通过 `Val::as_list()` / `Val::as_map()` 接受旧 `Val::List/Map` 作为 `Type::List/Map/Tuple`。
- `Type::validate()` 仍接受新 runtime heap 模型中的 `Val::Obj(HeapValue::List/Map)`，并通过 `TypedList::to_val_values()` / `TypedMap::to_val_entries()` 做元素验证；这保留新 runtime model 的类型验证能力。
- 新增 `old_val_containers_do_not_satisfy_container_types`，确认旧 `Val` 容器不再满足容器类型，也不再给 dispatch 暴露容器类型。
- 结合上一节，本次 type system 相关边界已经覆盖：
  - `PerfValueKind::from_val`
  - `TypeChecker::check_literal`
  - `TypeChecker::infer_val_type`
  - `Val::dispatch_type`
  - `Type::validate`
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core val::val_test::tests::old_val_containers_do_not_satisfy_container_types -- --nocapture` → 1 passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 40 passed。
- `cargo test -p lk-core typ::type_checker::tests -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 541 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` variants 和显式 constructor 仍存在，主要剩在 old eval、旧 `Val` 运算/访问测试、LLVM AOT runtime collections、if-let fixture。
- type system / compiler32 / PerformanceFacts 已不再把旧 `Val` 容器当作新架构容器；下一步可一次性迁移 old eval collection tests 到 exec32，随后删除 old eval 的 list/map/range 构造分支。

### 本 session 继续：迁出 old eval 的 collection/range literal 执行分支

继续按“legacy 全部移除、迁移到新架构”的要求，本次不只切单个入口，而是把旧 evaluator 里 collection/range 这一整块执行面迁出去：

- `core/src/expr/expr_test.rs` 中 list/map/range literal、literal access、bracket access、trailing comma、ternary map key 等测试已迁到 `execute_source32`。
- `core/src/expr/match_test.rs` 已整体从手写 AST + `Expr::eval_with_ctx` 改为 source + `execute_source32`，覆盖 literal、variable、wildcard、list、map、or、guard、int/float range、nested pattern 和 no-match fallback。
- `core/src/op/op_test.rs` 的 list/map literal 运算和嵌套容器比较已迁到 `execute_source32`。
- `Expr::eval_with_ctx` 现在遇到 `Expr::List` / `Expr::Map` / `Expr::Range` 直接报错，提示使用 Executor32；不再构造旧 `Val::List` / `Val::Map` 容器。
- 为补齐迁移后的新 VM 语义，`exec32` 的 dynamic add 现在支持 runtime heap map merge。
- `exec32` 的比较路径现在支持 runtime heap list/map 的深度 equality，并把 `CmpLt/Le/Gt/GeInt` 的执行 helper 从只读 Int 放宽为数值比较，覆盖 float range pattern。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core expr::match_test -- --nocapture` → 11 passed。
- `cargo test -p lk-core stmt::if_let_test -- --nocapture` → 15 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 63 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 541 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` variants 和显式 constructor 仍存在于旧 `Val` 运算/访问模型、LLVM AOT runtime collections、部分 old statement/destructuring fixture。
- `Expr::eval_with_ctx` 仍存在，用于 scalar/control/template/optional 等尚未完全迁出的旧路径；但 collection/range literal 执行已不再依赖它。
- 下一步应继续迁移 `Val` 旧容器运算/访问测试和 LLVM AOT collection handle，最终删除 `Val::List` / `Val::Map` variants。

### 本 session 继续：关闭旧 `Val` 容器算术语义

继续从基础模型收口旧容器行为，本次把 `Val` 自身的 list/map `+` / `-` 运算迁出旧模型：

- `Val::add_with_metrics` / `Val::sub_with_metrics` 已删除；`BinOp` 的旧 `Val` 算术现在只保留 scalar/string 语义。
- `impl Add/Sub for &Val` 不再支持 `Val::List` / `Val::Map` 的拼接、差集或 map merge。
- 旧 list concat/subtract helper（`concat_lists_with_metrics`、`append_to_list*` 等）已删除。
- `val::val_test` 中旧容器算术用例改为确认旧 `Val` 容器算术不再支持。
- `op::op_test` 已迁到 `execute_source32`，由新 VM 覆盖 list/map literal 的 `+` / `-`、map merge、map key removal 和嵌套容器 equality。
- `exec32` 新增 runtime heap list/map 的 dynamic subtraction：
  - list - list：删除出现在 RHS list 中的元素；
  - list - value：删除第一个相等元素；
  - map - map：按 RHS keys 删除；
  - map - key：按 runtime map key 删除，兼容 short/heap string key 表示。
- `Expr::eval_with_ctx` 的旧上下文测试中 list `+/-` 已改到 `execute_source32`，不再通过旧 `Val` 运算兜底。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 34 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 535 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "add_with_metrics|sub_with_metrics|concat_lists_with_metrics|append_to_list|legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` variants 和 constructor 仍存在，主要服务旧访问/display/serde、LLVM AOT runtime collections、`Pattern::matches` old path 和少量 fixture。
- `Val::access` 仍能读取旧 `Val` 容器，这是下一块需要迁出的基础面。
- LLVM AOT runtime collections 仍大量用旧 `Val` 容器 handle；后续需要单独按 AOT 边界迁移或删除旧 AOT collection runtime。

### 本 session 继续：关闭旧 `Val::access` 容器读取语义

继续把 collection 基础语义迁到新 VM，本次收掉旧 `Val::access` 对 `Val::List` / `Val::Map` 的读取分支：

- `Val::access` 现在不再支持旧 map string key lookup、旧 list integer indexing、旧 list slice、旧 string slice-by-list-key、旧 list `.len`、旧 map integer index-to-pair。
- `Val::access` 只保留 scalar/string、heap object field、channel metadata 等非旧容器读取路径。
- 删除旧 access 专用 copy helper：`access_copy_value`、`copy_container_value_with_metrics`、`access_copy_slice`。
- 删除旧 slice helper：`range_key_bounds`、`normalize_slice_bound`。
- `core/src/val/val_test.rs` 中 map/list literal access、越界 access、nested literal access 已迁到 `execute_source32`，由 exec32/runtime heap container 负责；旧 `Val::access` 只保留 string index 覆盖。
- 新 VM 的 list 负索引当前按错误处理；相关测试改为确认 `[10, 20, 30][-1]` 失败，不再恢复旧 `Val` 负索引语义。
- `core/src/expr/expr_test.rs` 的 collection 相关旧 evaluator fixture 已迁走，`seed_env()` 不再构造旧 `Val::List` / `Val::Map`。
- `core/src/stmt/if_let_test.rs` 删除测试专用 `Val::Map -> RuntimeVal` bridge，所有 if-let 用例直接执行 LK source，经 parser/compiler32/exec32 路径验证。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 34 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core stmt::if_let_test -- --nocapture` → 15 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 535 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` variants 和 constructor 仍存在，主要服务旧 display/serde/test conversion、LLVM AOT runtime collections、`Pattern::matches` old path 和少量 `Val` 单元测试。
- `Expr::eval_with_ctx` 仍保留旧 scalar/control/template 路径；collection/range literal 和 collection access 已迁到 exec32 测试覆盖。
- LLVM AOT runtime collections 仍调用旧 `Val::access` / `Val` handle；后续需要按 AOT 边界迁移或删除旧 AOT collection runtime。

### 本 session 继续：关闭 old `in` / pattern 容器语义和旧 list membership cache

继续按 `plan.md` 的 runtime value model 契约收口旧容器语义，本次删除非 LLVM 路径中仍主动消费旧 `Val::List` / `Val::Map` 的行为：

- `BinOp::In` 的旧 `Val` evaluator 不再支持旧 list membership、旧 list subset、旧 map key membership，也不再保留原先 `Int/Float/Bool/Nil in map` 的字符串化 key 兼容逻辑。
- `BinOp::In` 在旧 evaluator 中只保留 string-in-string；新 VM 的 list/map/string membership 继续由 compiler32/exec32 的 `Contains` 路径覆盖。
- `Pattern::matches` 的 old path 不再解构旧 `Val` list/map，也不再为 rest pattern 构造旧 `Val::List` / `Val::Map`。
- 删除失去生产调用方的旧 list membership cache：`core/src/val/values/cache.rs`、`cached_list_contains*`、`Val::list_contains` / `list_contains_all`。
- `val::val_test` 中原本主动验证旧 `Val::List` / `Val::Map` display/equality 的测试已迁到 `execute_source32`，由 runtime heap container 覆盖显示和深度 equality。
- 删除 `val_containers_stay_outside_runtime_heap_model`，不再用测试保护旧容器变体作为长期模型。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core expr::match_test -- --nocapture` → 11 passed。
- `cargo test -p lk-core stmt::destructuring_test -- --nocapture` → 15 passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 34 passed。
- `cargo test -p lk-core val::values -- --nocapture` → 6 passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 532 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::List` / `Val::Map` variants 仍在 `Val` enum、clone/display/serde/PartialEq/test conversion/type fallback 中。
- 旧 `as_list()` / `as_map()` materialization API 仍被 LLVM AOT runtime collections 和少量 runtime helper 使用；这是删除 `Val::List` / `Val::Map` 前的最大剩余块。
- `Expr::eval_with_ctx` 仍存在旧 scalar/control/template 路径，但已不再承载 collection literal/access/arithmetic/membership/pattern 主语义。

### 本 session 继续：移除顶层 `Val::List` / `Val::Map` 变体

本次完成 `plan.md` 中 runtime value model 的一个关键收口：`Val` 顶层不再携带 list/map 兼容变体。

- 从 `Val` enum 删除 `List(Arc<Vec<Val>>)` / `Map(Arc<FastHashMap<ArcStr, Val>>)`。
- `Val::list(...)` / `Val::map(...)` 过渡构造函数保留，但现在直接构造 `Val::Obj(HeapValue::List/Map)`：
  - list 使用 `HeapValue::List(TypedList::Mixed(...))`；
  - map 使用 `HeapValue::Map(TypedMap::from_runtime_entries(...))`。
- `Val::as_list()` / `Val::as_map()` 仍作为 AOT/旧 helper 的 materialization API，但只从 heap-backed `HeapValue::List/Map` 读取。
- 删除 `Val::List` / `Val::Map` 在 clone、serde、display、PartialEq、type checker literal fallback、template constant folding、AOT iterator fallback 中的匹配分支。
- `val::val_test` 中旧负向测试改为 `heap_val_containers_satisfy_container_types`，确认测试 helper 现在创建 heap-backed container，并能通过 `Type::List/Map` validation。
- 没有修改或拆分 `core/src/ast/parser.rs`。

验证：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 34 passed。
- `cargo test -p lk-core val::runtime_model -- --nocapture` → 14 passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 13 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 63 passed。
- `cargo test -p lk-core --lib` → 532 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "Val::List|Val::Map|Self::List|Self::Map" core/src stdlib/src -g '*.rs'` → no top-level `Val` variant matches；剩余 `Self::List/Map` 仅属于 runtime model / analysis / opcode enum 名称。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::list` / `Val::map` / `as_list` / `as_map` 仍存在，主要服务 LLVM AOT runtime collections 和少量旧测试/helper；它们现在已经是 heap-backed materialization API，不再是顶层 `Val` 变体。
- `Val::Obj(Arc<HeapValue>)` 本身仍与 `RuntimeVal::Obj(HeapRef)` 并存，未完全达到 `plan.md` 里最终 `Val::Obj(HeapRef)` 的形式。
- LLVM AOT runtime collections 仍大量通过 `Val` materialization 操作 list/map；后续需要按 AOT 边界迁移或删除旧 AOT collection runtime。

### 本 session 继续：删除 LLVM AOT collection materialization 边界

本次继续按“legacy 全部移除”收口，不再保留 `Val` 的 crate 内 list/map materialization 过渡 API。

- 删除 `Val::list(...)` / `Val::map(...)` / `Val::as_list()` / `Val::as_map()`。
- 删除 `core/src/llvm/runtime/collections.rs`，不再导出旧 AOT collection helper：
  - `lk_rt_build_list` / `lk_rt_build_map`；
  - `lk_rt_list_*`；
  - `lk_rt_map_*`；
  - 旧 `lk_rt_access` / `lk_rt_index` / `lk_rt_len` / `lk_rt_to_iter` collection bridge。
- LLVM backend 当前已在 `core/src/llvm/backend.rs` 中禁用，本次没有迁移旧 AOT collection ABI；后续恢复 AOT 时必须基于 `Instr32` 和 `RuntimeVal`/`HeapStore` 重新设计。
- `lk_rt_register_native_module_function` 不再把 native module exports materialize 成 `Val::Map`，现在明确返回 `-1` 并报告 AOT native module replay 已在 Instr32 迁移期间禁用。
- `lk_rt_apply_native_imports` 保留入口，但 item/native module replay 不再尝试转换旧 `Val` map exports。
- 删除 LLVM runtime collection tests，仅保留 scalar runtime helpers、global load/store、imports 收集和 LKB 禁用测试。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "Val::list\\(|Val::map\\(|as_list\\(|as_map\\(|lk_rt_build_list|lk_rt_build_map|lk_rt_list_|lk_rt_map_|lk_rt_to_iter|lk_rt_index|lk_rt_len|lk_rt_access" core/src stdlib/src -g '*.rs'` → no matches。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core --lib` → 526 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::Obj(Arc<HeapValue>)` 仍未收敛为 `Val::Obj(HeapRef)`；这是 runtime value model 的下一块基础迁移。
- LLVM runtime 仍保留 scalar/global/import/AOT function 壳层，作为已禁用 backend 的外围入口；collection bridge 已删除。
- 旧 `Expr::eval_with_ctx` scalar/control/template 路径仍存在，但 collection literal/access/arithmetic/membership/pattern 主语义已不在 old evaluator 承载。

### 本 session 继续：删除 old `Val` object/channel/type validation helper

继续收窄 `Val::Obj(Arc<HeapValue>)` 的创建和消费表面，本次删除只服务 old evaluator / old Val API 的 helper：

- `Expr::eval_with_ctx` 不再构造 struct literal；`Expr::StructLiteral` 与 list/map 一样直接要求使用 `Executor32`。
- 删除 `Val::object(...)`、`Val::as_object()`、`Val::val_to_object_field(...)`，old `Val` 不再提供 runtime object 构造 API。
- 删除 `Val::task/channel/stream/stream_cursor` 构造 helper 及对应 `as_*` helper；stdlib/runtime modules 已走 `RuntimeVal::Obj(HeapRef)` + `HeapStore`。
- 删除 `impl From<(u64, i64, Type)> for Val`，不再用旧 `Val` conversion 构造 channel heap object。
- 删除 `Type::validate(&Val)`。这是旧 `Val` 运行时验证入口，且当前只有测试调用；新类型检查走 `TypeChecker`，新 runtime 容器验证走 `RuntimeVal`/`HeapStore`。
- 删除 `val::val_test` 中对 `Type::validate(&Val)` 的 heap container 测试，不再用测试保护 old `Val` container validation。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "Val::object\\(|as_object\\(|val_to_object_field|\\.validate\\(&|pub fn validate\\(|as_task\\(|as_channel\\(|as_stream\\(|as_stream_cursor\\(|Val::task\\(|Val::channel\\(|Val::stream\\(|Val::stream_cursor\\(|From<\\(u64, i64, Type\\)>" core/src stdlib/src -g '*.rs'` → no matches。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src -g '*.rs'` → no matches。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 33 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core typ::type_checker::tests -- --nocapture` → 15 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 63 passed。
- `cargo test -p lk-core --lib` → 525 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::Obj(Arc<HeapValue>)` 仍存在，主要服务 heap string、old callable/AOT 壳层、display/serde/materialization 测试；但 old list/map/object/channel 构造 API 已删除。
- old evaluator 仍保留 scalar/control/template/function-call 壳层；list/map/struct literal 已不再由 old evaluator 构造 heap object。
- `Val::call` / `call_named` 仍是旧 callable bridge，且 LLVM runtime scalar call helpers 仍会进入该路径；后续可继续禁用或迁到 `RuntimeVal`/`Executor32`。

### 本 session 继续：删除 old `Val::call` callable bridge

继续收窄旧 `Val` 执行路径，本次删除 old evaluator / LLVM runtime 通过 `Val` 调用函数的桥接层：

- 删除 `core/src/val/values/call.rs`，`Val` 不再提供 `call` / `call_vm` / `call_named` / `call_named_vm`。
- `Expr::eval_with_ctx` 的函数调用、call expression、具名参数调用不再尝试通过 `Val::call` 执行 callable，统一返回“use Executor32”错误。
- LLVM runtime 的 `lk_rt_call` / `lk_rt_call_native` 只保留直接 AOT raw function pointer 调用；非 AOT 的 `Val` callable fallback 已禁用。
- LLVM runtime 的 `lk_rt_call_method` 整体禁用，避免继续通过 `Val::access` + `Val::call` 维持旧方法调用桥。
- 删除 `RuntimeState::decode_values`，不再为 LLVM call fallback materialize `Vec<Val>` 参数。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "decode_values\\(|\\.call\\(|call_named\\(|call_named_vm|call_vm\\(|Val::call|core/src/val/values/call" core/src stdlib/src -g '*.rs'` → no old `Val` callable bridge matches；剩余 stdlib `iter::call_callable` 是 `RuntimeVal`/`Executor32` 路径。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 63 passed。
- `cargo test -p lk-core --lib` → 525 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：最大仍为 `core/src/ast/parser.rs` 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val::Obj(Arc<HeapValue>)` 仍存在，主要服务 long string、AOT function handle、old display/serde/type dispatch 壳层；但 old callable execution bridge 已删除。
- LLVM runtime 仍保留 scalar/global/import/AOT function pointer 壳层；backend 仍整体禁用，后续恢复必须基于 Instr32。
- old evaluator 仍保留 scalar/control/template/access 壳层；函数调用、list/map/struct/range/select 已不再由 old evaluator 执行。

---

## 接手注意事项

- 不要把 `plan.md` 当进度日志；它是契约。
- 不要做 workload-shaped fused opcode 或 benchmark hack。
- 不要为了旧兼容保留明显碍事的路径；当前项目未发布，允许大改和删除。
- 除 LLVM 相关部分外，不要引入 `unsafe`。
- 不要回滚 dirty tree 中非自己改动的文件。
- 不要因为某个旧测试失败就自动恢复旧行为；先判断测试是否还符合新 VM 契约。
- 优先改基础模型，再改执行器，再改外围 CLI/AOT/benchmark。

## 建议优先阅读文件

- `plan.md`
- `core/src/vm/ir32.rs`（Instr32 格式，刚修改）
- `core/src/vm/exec32.rs`（Call handler 修改，NewList handler）
- `core/src/vm/compiler32.rs`（`materialize_list`, `emit_move`）
- `core/src/vm/compiler32/call.rs`（`lower_dynamic_method_call`, `lower_builtin_method_call`, `lower_call_window_regs`）
- `core/src/vm/compiler32/builder.rs`（`emit_move`）
- `core/src/vm/context/core_methods.rs`（string/list dispatch，本 session 新增）
- `core/src/val/runtime_model.rs`
- `core/src/val/runtime_model/heap.rs`
- `core/src/vm/runtime32.rs`
- `core/src/vm/gc32.rs`
- `core/src/vm/exec32/runtime_callable.rs`
- `core/src/vm/exec32/return_values.rs`
- `core/src/vm/exec32/stack.rs`
- `core/src/vm/exec32/program.rs`
- `core/src/vm/context.rs`
- `core/src/vm/context/val_bindings.rs`
- `core/src/vm/compiler32/builder.rs`

### 本 session 继续：删除 `Val` callable/AOT 壳层并收窄 heap display/serde

继续按“legacy 全部移除，迁移到新架构”收窄旧 `Val::Obj(Arc<HeapValue>)` 表面，本次删除旧
`Val` 可调用对象和 AOT raw function handle 入口：

- 删除 `AotFunction` 与 `CallableValue::Aot`，当前 AOT backend 仍禁用；后续恢复 AOT 必须基于
  `Instr32`/`RuntimeVal`/`HeapStore` 重新建 callable ABI。
- 删除 `Val::aot_function`、`Val::runtime_callable32`、`Val::runtime_native32_for_test`、
  `Val::as_runtime_callable32`、`Val::is_callable` 以及对应测试。
- `lk_rt_make_aot_function`、`lk_rt_call`、`lk_rt_call_native` 不再保留 raw function pointer
  direct-call bridge，迁移期统一返回 `nil` 并打印禁用信息。
- 删除 Executor32/import/runtime callable copy 路径中对 `CallableValue::Aot` 的分支。
- 删除 old `Val` container literal 兼容测试，不再通过测试直接构造
  `Val::Obj(HeapValue::List/Map)` 来保护旧模型。
- `Val::Obj` 不再持有 `Arc<HeapValue>`，收窄为只承载 long string 的 `ArcStr`；容器、object、
  callable、task/channel/stream/error 的真实语义只由 `RuntimeVal::Obj(HeapRef)` + `HeapStore`
  承载。
- `Val` 的 `Display`、`Serialize`、`dispatch_type`、`type_name` 只处理 scalar/string，不再半支持
  runtime heap container/object/callable。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "CallableValue::Aot|AotFunction|aot_function_from_val|call_aot_function_raw|Val::aot_function|runtime_callable32\\(|runtime_native32_for_test|as_runtime_callable32|\\.is_callable\\(" core/src stdlib/src -g '*.rs'`
  → no matches。
- `rg -n "(^|[^A-Za-z])Val::Obj\\(|Arc<HeapValue>|CallableValue::Aot|AotFunction|Val::aot_function|runtime_callable32\\(|runtime_native32_for_test|as_runtime_callable32|\\.is_callable\\(" core/src stdlib/src -g '*.rs'`
  → `Val::Obj` 仅剩 scalar long string shell；无 `Arc<HeapValue>` / callable / AOT 旧入口。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 32 passed。
- `cargo test -p lk-core val::val_test::tests::val_stays_within_two_words -- --nocapture` → 1 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core --lib` → 520 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制。

### 本 session 继续：删除剩余旧容器 materialization 与 Val buffer 壳层

继续按“legacy 全部移除，迁移到新架构”推进，本次把上一轮未写入进度的事实补齐，并继续切掉
无生产调用方的旧 `Val` 缓冲区：

- 旧 `Val::Obj` 名称已从 `Val` 删除并收敛为 `Val::LongStr(ArcStr)`；`Obj` 名称只属于
  `RuntimeVal::Obj(HeapRef)`。
- 删除旧容器 materialization helper：
  `Val::object_field_to_val`、`TypedList::to_val_values`、`TypedMap::to_val_entries`。
- 删除旧 string/map key 缓存模块 `core/src/val/values/map_key_cache.rs` 以及对应 module wiring。
- `TypedList`/`TypedMap` 跨 backing equality 已基于 `RuntimeVal`/`entries()` 比较，不再依赖旧
  `Val` 容器快照。
- 删除 `core/src/vm/alloc.rs` 中无调用方的旧 TLS `Vec<Val>` / map-entry / named-arg buffer API：
  `TLS_VAL_BUF`、`with_val_buffer`、`with_map_entries`、`with_indexed_vals`、
  `with_reg_val_pairs`、`with_named_pairs` 等。
- LLVM runtime 的迁移期 scalar globals 已移到 `RuntimeState::aot_globals`，不再通过
  `VmContext::set_val_binding` / `get_val_binding` 存取 AOT global。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "(^|[^[:alnum:]_])Val::Obj|object_field_to_val|to_val_values\\(|to_val_entries\\(|Arc<HeapValue>|CallableValue::Aot|AotFunction|Val::aot_function|runtime_callable32\\(|runtime_native32_for_test|as_runtime_callable32|\\.is_callable\\(|TLS_VAL_BUF|with_val_buffer|with_map_entries|map_key_cache|ctx\\.set_val_binding|ctx\\.get_val_binding" core/src stdlib/src -g '*.rs'`
  → 只剩 `Expr::eval_with_ctx` / `Pattern` / `VmContext` tests 的 old expression `Val` binding 面；无
  `Val::Obj`、AOT callable、旧容器 materialization、TLS `Val` buffer 或 LLVM `ctx` global 访问。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core vm::alloc -- --nocapture` → 3 passed。
- `cargo test -p lk-core --lib` → 516 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制；`core/src/vm/alloc.rs`
  已降到 130 行。

当前剩余旧模型面：

- `VmContext::val_bindings`、`Expr::eval_with_ctx` 和 `Pattern` guard/range 仍保留 old scalar
  expression compatibility；它们已经不再被 exec32 或 LLVM runtime global path 使用。
- LLVM backend/runtime 仍是迁移期禁用/标量壳层；后续恢复 AOT 必须基于
  `Instr32`/`RuntimeVal`/`HeapStore`。

### 本 session 继续：删除 old expression evaluator 与 VmContext Val binding

继续移除 `plan.md` 中明确要求淘汰的旧 context/global 字符串映射和 old evaluator 路径：

- 删除 `Expr::eval()` / `Expr::eval_with_ctx()`，表达式行为测试统一迁到 `execute_source32`。
- `Pattern` 的 old `Val` guard/range evaluator 不再调用 `Expr::eval_with_ctx`；match/if-let 的真实语义由
  compiler32/exec32 路径覆盖。
- 删除 `VmContext::val_bindings`、`core/src/vm/context/val_bindings.rs` 和所有 old `Val` binding API：
  `get_val_binding`、`set_val_binding`、`assign_val_binding`、`remove_val_binding`、
  `push_scope`、`pop_scope`、`is_local_name`、`bind_param_at_slot`。
- 删除只保护旧 `Val` context 行为的 `stmt_test::test_environment` 和 `vm::context` Val binding tests。
- closure capture 测试中的后置冲突变量改为 `define_runtime_value`，继续验证新 runtime global 不影响
  lexical capture。
- 删除 `UnaryOp::eval_val`、`BinOp::eval`、`BinOp::eval_vals` 和 metrics wrapper；LLVM scalar math
  helper 现在直接使用 `Val` 运算符，不再通过 old expression/value evaluator。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "eval_with_ctx|Expr::eval\\(|\\.eval\\(\\)|get_val_binding|set_val_binding|assign_val_binding|remove_val_binding|bind_param_at_slot|is_local_name|ValBindingContext|val_bindings|context/val_bindings|TLS_VAL_BUF|CallableValue::Aot|AotFunction|map_key_cache" core/src stdlib/src -g '*.rs'`
  → no matches。
- `rg -n "(^|[^[:alnum:]_])Val::Obj" core/src stdlib/src -g '*.rs'` → no matches；`RuntimeVal::Obj`
  仍是新 runtime model 的合法 heap handle。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core vm::context -- --nocapture` → 2 passed。
- `cargo test -p lk-core stmt::function_test::tests::test_outer_returns_closure_value -- --nocapture` → 1 passed。
- `cargo test -p lk-core stmt::stmt_test -- --nocapture` → 69 passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core --lib` → 513 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制；`core/src/vm/context.rs`
  已降到 774 行，`core/src/expr/expr_impl.rs` 已降到 838 行。

当前剩余旧模型面：

- `Val` 仍作为 scalar/string shell 存在，主要服务 parser constants、LLVM scalar runtime helper 和旧
  value-level arithmetic tests；容器/object/callable/global context 旧路径已删除。
- LLVM backend/runtime 仍是迁移期禁用/标量壳层；后续恢复 AOT 必须基于
  `Instr32`/`RuntimeVal`/`HeapStore`。

### 本 session 继续：LLVM runtime scalar shell 迁到 RuntimeVal

继续按“legacy 全部移除，迁移到新架构”收口 AOT/LLVM 迁移期外壳，本次把 LLVM runtime 的
scalar handle/global 模型从旧 `Val` 切到新 `RuntimeVal`：

- `core/src/llvm/encoding.rs` 的 immediate encode/decode 现在接收和返回 `RuntimeVal`，不再依赖旧
  `Val`。
- `RuntimeState::aot_globals` 从 `FastHashMap<String, Val>` 改为
  `FastHashMap<String, RuntimeVal>`。
- LLVM runtime `HandleTable` 从 `Vec<Val>` 改为 `Vec<RuntimeVal>`，并新增 `HeapStore` 保存
  long string handle。
- `lk_rt_intern_string` / `lk_rt_to_string` / `lk_rt_load_global` / `lk_rt_define_global` / scalar math
  helpers 均改为读写 `RuntimeVal`。
- `lk_rt_cmp` 不再通过 `BinOp::cmp(&Val, &Val)`，改为本地 `RuntimeVal` scalar/string compare。
- LLVM runtime 中旧 `AOT/Val` 错误文案已改为 AOT runtime bridge 文案。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "use crate::val::Val|crate::val::Val|(^|[^A-Za-z0-9_])Val::|Vec<Val>|FastHashMap<String, Val>|AOT/Val|HandleTable.*Val|decode_value\\(&self, raw: i64\\) -> Val" core/src/llvm -g '*.rs'`
  → no matches。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core llvm::runtime -- --nocapture` → 7 passed。
- `cargo test -p lk-core llvm::encoding -- --nocapture` → 4 passed。
- `cargo test -p lk-core --lib` → 513 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding / 部分 value-level scalar tests 的壳存在。
- LLVM backend/runtime 仍是迁移期禁用外壳；collection/call ABI 已删或禁用，后续恢复 AOT 必须基于
  `Instr32`/`RuntimeVal`/`HeapStore`。

### 本 session 继续：删除 AST/Val serde 与旧便捷转换入口

继续收窄 `Val` 的职责，让它只作为 AST scalar literal 壳，不再承担通用序列化/反序列化或表达式转换
边界：

- `Expr`、`Pattern`、`SelectPattern`、`SelectCase`、`TemplateStringPart`、`MatchArm` 删除
  `Serialize` / `Deserialize` derive；AST 不再要求 `Val` 实现 serde。
- 删除 `ValLiteralVisitor` 和 `impl Deserialize for Val`；结构化 JSON/YAML/TOML 入口继续只走
  `RuntimeVal + HeapStore` runtime decoder。
- 删除 `core/src/val/values/serde_impl.rs` 和 `impl Serialize for Val`。
- 删除 `Val::try_from<T: Serialize>`，不再通过 serde_json 把任意 Rust 数据折回旧 `Val`。
- 删除无调用方的 `Val::dispatch_type()` / `Val::display_string()`。
- 删除无调用方的 `impl TryInto<Val> for &Expr` 和 `impl From<Val> for Expr`，避免继续暴露
  old scalar expression bridge。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "TryInto<Val>|From<Val> for Expr|Val::try_from|impl Serialize for Val|impl<'de> Deserialize<'de> for Val|serde_impl|pub fn dispatch_type\\(&self\\)|pub fn display_string\\(&self" core/src stdlib/src -g '*.rs'`
  → no old `Val` API matches；剩余 `runtime_value_display_string` / `runtime_dispatch_type` 属于
  `RuntimeVal` 新路径。
- `cargo test -p lk-core val::de_test -- --nocapture` → 8 passed。
- `cargo test -p lk-core ast::ast_test -- --nocapture` → 32 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core --lib` → 513 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding / value-level scalar operator tests 的壳存在。
- `Val::LongStr(ArcStr)` 仍未按 `plan.md` 最终形态收敛为 heap-backed object handle；后续应继续把
  AST scalar literal lowering 与 runtime immediate/heap string 模型对齐。

### 本 session 继续：删除旧 `Val` arithmetic operator API

继续收窄 `Val` 的运行时表面积，本次把旧的 `&Val` 运算符实现从 value model 中移除，让算术语义
只保留在 exec32/runtime 路径和显式 AST literal folding helper 中：

- 删除 `core/src/val/values/ops.rs` 以及 `val::values::ops` module wiring。
- 删除 `impl Add/Sub/Mul/Div/Rem for &Val`，不再暴露 `&Val + &Val` 这类旧运行时 API。
- 删除无调用方的测试 helper `Val::test_from` / `TestIntoVal`。
- `Expr::fold_constants` 改为调用私有 `fold_literal_arith` helper；这是 AST literal folding，不再通过
  `Val` 的 trait operator 伪装成 runtime value 运算。
- `BinOp::cmp(&Val, &Val) -> Result<bool>` 改为 `cmp_literals(&Val, &Val) -> Option<bool>`，只表达
  “能否静态折叠 literal comparison”，不再构造旧 value-op 错误。
- `val::val_test` 中旧 `&Val` 运算单测改为 source-level exec32 行为测试。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "impl Add for &Val|impl Sub for &Val|impl Mul for &Val|impl Div for &Val|impl Rem for &Val|err_op\\(|op\\.cmp\\(|cmp\\(&self, l: &Val|test_from\\(|TestIntoVal|mod ops;|values/ops" core/src stdlib/src -g '*.rs'`
  → no old `Val` operator/test helper matches；`core/src/op/mod.rs` 的 `mod ops` 是 `BinOp` module，合法。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 22 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core --lib` → 503 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding shell 存在。
- `Val::LongStr(ArcStr)` 仍未按 `plan.md` 最终形态收敛为 heap-backed object handle；后续应继续把
  AST scalar literal lowering 与 runtime immediate/heap string 模型对齐。

### 本 session 继续：删除旧 `Val` introspection / ordering / intern helper

继续收窄 `Val`，本次移除旧 scalar shell 上不应继续暴露给 runtime 的辅助 API：

- 删除无调用方的 `Val::str_intern`、`Val::intern_str`、`Val::string_key_arcstr`。
- `Val::concat_strings` 仍仅用于 AST literal folding，注释已改为 literal folding 语义，不再标为 hot path。
- 删除 `Val::type_name()`；compiler32 错误文案改用内部 `ast_literal_kind(&Val)` helper。
- 删除 `impl PartialOrd for Val`；`BinOp::cmp_literals` 现在使用私有 `cmp_literal_ordering`，比较语义不再作为
  `Val` 的公共 value-level ordering 暴露。
- `val::val_test` 中旧 `Val` ordering 测试改为 source-level exec32 比较行为测试。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "impl PartialOrd for Val|partial_cmp\\(&.*Val|Val::type_name|pub fn type_name\\(&self\\)|str_intern\\(|intern_str\\(|string_key_arcstr\\(" core/src stdlib/src cli/src -g '*.rs'`
  → no old `Val` helper matches；剩余 `RuntimeVal` / `HeapValue` 的 `type_name()` 合法。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 18 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core op::op_test -- --nocapture` → 4 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo test -p lk-core --lib` → 499 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding shell 存在，且仍包含 `LongStr(ArcStr)`。
- CLI coverage、LKB、LLVM/native AOT 仍处于 Instr32 migration disabled 状态；最终闭环仍需恢复到新 IR。

### 本 session 继续：删除 `Val::access` 与剩余旧 Val pattern API

继续按“legacy 全部移除，迁移到新架构”收窄 `Val` 的职责，本次删除旧 `Val` 上的字段/索引读取入口和
无调用方的宽泛转换：

- 删除 `Val::access` / `access_impl` 后的残留测试和 helper；`Val` 不再提供 list/map/string access API。
- 删除 `Val::ascii_char_value`，字符串索引读取现在只走 exec32/runtime access 路径。
- 删除 `impl<T> From<Box<T>> for Val`、`impl<T> From<Option<T>> for Val`、`impl From<()> for Val`，
  避免任意 Rust 容器继续隐式折回旧 `Val` 壳。
- `Expr::fold_constants` 不再尝试通过 `Val::access` 折叠常量 access；source-level 访问行为仍由
  compiler32/exec32 测试覆盖。
- 删除无调用方的 `Pattern::matches(&Val, Option<&VmContext>)` 和内部 `matches_impl`；真实 pattern
  语义继续由 compiler32 pattern lowering 与 exec32 执行路径承担。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "ascii_char_value|Val::access\\(|\\.access\\(&|pub\\(crate\\) fn access|fn access_impl|From<Box|From<Option|From<\\(\\)>|Val::from\\(None|Val::from\\(Some|Val::from\\(Box" core/src stdlib/src -g '*.rs'`
  → no matches。
- `rg -n "legacy_|LegacyValContext|legacy|eval_with_ctx|pub fn eval\\(|fn eval\\(|Expr::eval|runtime_bridge|Val::List|Val::Map|copy_container_value|ValLiteral|serde_impl|val_bindings|collections.rs|values/call|map_key_cache" core/src stdlib/src -g '*.rs'`
  → no legacy bridge/API matches；剩余 `legacy` 文案只属于 LLVM/AOT disabled migration message。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 31 passed。
- `cargo test -p lk-core expr::expr_test -- --nocapture` → 21 passed。
- `cargo test -p lk-core --lib` → 512 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行，未超过 1500 行限制。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding / value-level scalar operator tests 的壳存在。
- `Val::LongStr(ArcStr)` 仍未按 `plan.md` 最终形态收敛为 heap-backed object handle；后续应继续把
  AST scalar literal lowering 与 runtime immediate/heap string 模型对齐。

### 本 session 继续：删除 `Val::From<T>` 并收紧 named-call map materialization

继续按“先把 legacy 全部移除，迁移到新架构”收口，本次切掉两个仍会把新旧 value 边界混在一起的面：

- 删除 `core/src/val/values/convert.rs` 和 `mod convert;`。
- 删除 `impl From<String> for Val`、`impl From<&str> for Val`、`impl From<i64> for Val`、
  `impl From<f64> for Val`、`impl From<bool> for Val`。
- `core/src/ast/ast_test.rs` 中剩余 `.into()` literal 全部改为显式 `Val::Int` / `Val::Bool` /
  `Val::Float`，避免测试继续依赖旧隐式转换。
- `__lk_call_method_named` 不再把 named args map 立即展开成 `Vec<(Arc<str>, RuntimeVal)>`。
  现在只校验并传递 `HeapRef`，runtime callable/property closure 分支通过
  `write_named_args32_to_frame_from_typed_map` 从 `TypedMap` 直接写入 callee frame。
- 新增 `call_runtime_value32_runtime_named_map`，native 和跨 `Runtime32` fallback 仍按需 materialize，
  但常见 runtime closure/property call 不再走 named map -> Vec -> frame 的旧中转。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "Expr::Val\\([^\\n]*\\.into\\(\\)|let [^:]+: Val = .*\\.into\\(\\)|Val::from\\(|impl From<.*> for Val|mod convert;|values/convert" core/src stdlib/src cli/src -g '*.rs'`
  → no matches。
- `rg -n "runtime_named_args\\(" core/src stdlib/src cli/src -g '*.rs'`
  → no matches。
- 剩余 `Vec<(Arc<str>, RuntimeVal)>` 只在 `TypedMap::string_entries_*` helper、`CallNamed`
  bytecode/native fallback、以及 `materialize_named_arg_map` fallback 中存在；`__lk_call_method_named`
  的 runtime closure/property path 已改为 direct frame writer。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core ast::ast_test -- --nocapture` → 32 passed。
- `cargo test -p lk-core val::val_test -- --nocapture` → 18 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture` → 12 passed。
- `cargo test -p lk-core --lib` → 499 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding 壳存在，且仍包含 `LongStr(ArcStr)`。
- `CallNamed` bytecode 和 native / cross-runtime fallback 仍会 materialize
  `Vec<(Arc<str>, RuntimeVal)>`；后续可以继续把 `CallNamed` 的 closure 分支也改成 stack/direct
  named source。
- CLI coverage、LKB、LLVM/native AOT 仍处于 Instr32 migration disabled 状态；最终闭环仍需恢复到新 IR。

### 本 session 继续：CallNamed closure 分支改为 stack direct named writer

继续清掉 call 参数 materialization，本次把 `Opcode32::CallNamed` 的同 runtime closure 分支从
`read_named_call_args() -> Vec<(Arc<str>, RuntimeVal)> -> write_named_args32_to_frame()` 改为直接读
caller stack window 并写入 callee frame：

- 新增 `write_named_args32_to_frame_from_stack`，从 caller stack 的 name/value pair 读取命名参数，
  用 `&str` 对比 `Function32.param_names`，不为 closure 分支构造 named `Vec`。
- `call_closure_named_stack_args` 现在接收 `named_count`，在共享 stack 上 split caller/callee
  window 后调用 direct writer。
- `read_named_call_args` 仍保留给 `RuntimeNative32` 和跨 `Runtime32` fallback；closure 分支不再调用它。
- 移除 `write_named_args32_to_frame_from_typed_map` 上过期的 `#[allow(dead_code)]`。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `rg -n "read_named_call_args\\(|call_closure_named_stack_args\\(|write_named_args32_to_frame_from_stack|Vec<\\(Arc<str>, RuntimeVal\\)>|runtime_named_args\\(" core/src/vm core/src/val stdlib/src -g '*.rs'`
  → closure 分支只命中 `call_closure_named_stack_args(..., named_count, ...)` 和
  `write_named_args32_to_frame_from_stack`；`read_named_call_args` 只剩 native / cross-runtime fallback。
- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-stdlib` → passed。
- `cargo test -p lk-core vm::compiler32::tests::compiler32_lowers_named_args_to_normal_call_window -- --nocapture`
  → 1 passed。
- `cargo test -p lk-core vm::compiler32::tests -- --nocapture` → 62 passed。
- `cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture` → 12 passed。
- `cargo test -p lk-core stmt::function_test -- --nocapture` → 28 passed。
- `cargo test -p lk-core --lib` → 499 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行；
  `core/src/vm/exec32/named_call.rs` 361 行，未接近 1500 行限制。

当前剩余旧模型面：

- `Val` 仍作为 AST scalar literal / constant folding 壳存在，且仍包含 `LongStr(ArcStr)`。
- `RuntimeNative32` 和跨 `Runtime32` named fallback 仍会 materialize
  `Vec<(Arc<str>, RuntimeVal)>`；这是跨 native/runtime ABI 的剩余边界，不再是同 runtime closure 热路径。
- CLI coverage、LKB、LLVM/native AOT 仍处于 Instr32 migration disabled 状态；最终闭环仍需恢复到新 IR。

### 本 session 继续：恢复 CLI coverage 到统一 Instr32 编译/执行路径

继续按“legacy 全部移除，迁移到新架构”收口 CLI 工具链，本次把 coverage 从迁移期 disabled
状态恢复到新 `Instr32` module：

- `cli/src/coverage.rs` 不再 hard bail；现在会解析 source、构建 stdlib/package resolver context，
  输出 `Module32` 的静态 coverage（functions/natives/globals/instructions/registers/consts/opcodes）。
- `--runtime` 会执行 `Program::execute32_with_ctx`，复用执行结果里的 `Program32Result.module` 输出静态
  coverage，并打印 `VmRuntimeMetrics`。
- `core/src/vm/exec32/program.rs` 新增 `compile_program32_module_with_ctx`，把 imports replay、
  `VmContext::runtime_globals_iter()` 和 `Compiler32::compile_module_with_natives_and_globals` 收成唯一 helper。
- `execute_program32_raw_with_ctx` 复用该 helper，coverage 不再在 CLI 里复制一套 external globals 编译逻辑。
- 修复 `coverage --runtime bench/workloads_business_algorithms.lk` 之前失败的
  `Compiler32 undefined callable __lk_call_method`：root cause 是 coverage 静态编译路径没有对齐真实
  `execute_program32_raw_with_ctx` 的 imports + runtime globals 编译流程。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-cli` → passed。
- `cargo run -p lk-cli -- coverage examples/fib.lk` → 输出 2 functions / 41 instructions / 36 globals。
- `cargo run -p lk-cli -- coverage --runtime examples/fib.lk` → 输出 coverage + runtime metrics，
  `opcode_steps=4`、`register_writes=2`。
- `cargo run -p lk-cli -- coverage --runtime bench/workloads_business_algorithms.lk` → 15 个 workload 全部完成；
  输出 9 functions / 2590 instructions / 52 globals，runtime metrics 中
  `opcode_steps=198001101`、`register_writes=162931811`。
- `cargo test -p lk-cli` → 25 passed。
- `cargo test -p lk-core --lib` → 499 passed。
- `cargo test -p lk-stdlib --lib` → 95 passed。
- `rg -n "legacy|Legacy|LEGACY" core/src stdlib/src cli/src -g '*.rs'`
  → no source matches。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行；`cli/src/coverage.rs` 124 行；
  `core/src/vm/exec32.rs` 789 行；`core/src/vm/exec32/program.rs` 75 行。

当前剩余旧模型面：

- LKB compile/run 仍 disabled；下一步应以新的 `Module32` artifact 格式替代 LKB，而不是恢复旧 bytecode。
- LLVM/native AOT 仍 disabled；需要在 `Instr32` module/runtime ABI 上重建，不回接旧 `Val`/old bytecode。
- `RuntimeNative32` 和跨 `Runtime32` named fallback 仍会 materialize
  `Vec<(Arc<str>, RuntimeVal)>`；同 runtime closure hot path 已经 direct frame writer。

### 本 session 继续：`lk compile` 迁到可执行 `.lkm` Instr32 module artifact

继续推进 `plan.md` 第 12 步，本次把 CLI compile 从 LKB disabled stub 切到新 `Module32` 输出：

- `lk compile [FILE]` 现在解析 source、构建与运行/coverage 共享的 stdlib/package resolver context，
  调用 `compile_program32_module_with_ctx`，写出 `FILE.lkm`。
- 新增 `Module32Artifact` JSON artifact，编码 imports、globals、functions、typed const pool 和
  raw `Instr32` words；不编码 inline native entries。
- `lk FILE.lkm` 现在会 decode artifact、replay imports、按 module globals 从当前 `VmContext`
  seed runtime globals，并执行 `Module32`。
- `.lkm` 不再是不可执行文本 listing；它是当前 LKB 替代物的可运行 module artifact。
- CLI source 执行不再读取 raw bytes 后按 `LKB` magic auto-detect；`.lkb` 输入现在直接报
  `LKB execution has been removed`。
- `cli/src/coverage.rs` 改为复用 `build_vm_context`，coverage、compile、run 三条 CLI 路径共用同一
  stdlib/package resolver 初始化。
- `cli/src/paths.rs` 的 `lkb` / `bytecode` target 文案改为指向 `lk compile FILE` 的 `.lkm`
  输出。
- `README.md` 和 `website/src` 中仍提到 `.lkb` / bytecode 的用户文档已更新为 `Instr32` /
  `.lkm`。
- `website/src/spec/LANG.md` 和 `LANG_zh.md` 已同步 CLI compile/run `.lkm` 说明。
- 没有修改或拆分 `core/src/ast/parser.rs`。

当前已确认：

- `cargo fmt --all -- --check` → passed。
- `cargo check -p lk-core -p lk-cli` → passed。
- `cargo test -p lk-core vm::artifact32 -- --nocapture` → 1 passed。
- `cargo test -p lk-cli --test lkb_cli_test -- --nocapture` → 9 passed。
- `cargo test -p lk-cli` → 25 passed。
- `cargo run -p lk-cli -- compile examples/fib.lk` → 输出 `examples/fib.lkm`；临时生成物已删除。
- CLI tests 覆盖 `.lkm` 运行：普通 source、file import、package path dependency compile 后均能
  通过 `lk FILE.lkm` 执行。
- `cd website && bun run build` → built in 892ms。
- `rg -n "compile output is disabled|LKB execution is disabled|disabled LKB compile|LKB compile should be disabled|coverage is disabled|legacy|Legacy|LEGACY" cli/src cli/tests -g '*.rs'`
  → no matches。
- `rg -n "LKB|lkb|bytecode|字节码|\\.lkb" README.md website/src docs -g '*.md' -g '*.ts' -g '*.svelte'`
  → no matches。
- 单文件行数检查：`core/src/ast/parser.rs` 仍为 1499 行；`cli/src/main.rs` 391 行；
  `cli/src/coverage.rs` 104 行；`core/src/vm/artifact32.rs` 347 行。

当前剩余旧模型面：

- LLVM IR / native executable output 仍 disabled，需要后续基于 `Instr32` module/runtime ABI 重建。
- `.lkm` artifact 目前是 JSON，不是紧凑二进制格式；后续如需发布级 artifact，应继续补版本化
  binary encode/decode 和兼容性策略。
- `RuntimeNative32` 和跨 `Runtime32` named fallback 仍会 materialize
  `Vec<(Arc<str>, RuntimeVal)>`。
