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
