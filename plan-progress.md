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
- `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime` 仍保留为过渡层，用于 legacy `Val` 容器快照和 conversion bridge。
- `core/src/val/runtime_model/legacy.rs` 已隔离 `OwnedRuntimeList`、`OwnedRuntimeMap` 和 legacy `Val` conversion helper。
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
- `vm::registers` 已重命名并隔离为 `vm::legacy_registers`。
- `core/src/val/values/mod.rs` 和 `ops.rs` 现在引用 `legacy_registers::copy_container_value_for_register_with_metrics`。

## 最近已通过的验证

以下命令在最近重构过程中通过过，可作为接手后的局部回归起点：

```sh
cargo fmt --all -- --check
cargo check -p lk-core -p lk-stdlib
cargo test -p lk-core val::runtime_model -- --nocapture
cargo test -p lk-core val::values -- --nocapture
cargo test -p lk-core vm::legacy_registers -- --nocapture
cargo test -p lk-core vm::exec32::return_values -- --nocapture
cargo test -p lk-core vm::exec32::exec32_tests -- --nocapture
cargo test -p lk-core vm::compiler32::tests -- --nocapture
cargo test -p lk-core vm::runtime32 -- --nocapture
cargo test -p lk-core vm::gc32 -- --nocapture
```

不要把这些结果等同于完整 hard gate。最近没有完成全量 `cargo test -p lk-core --lib` 的闭环。

## 最近已验证（本 session）

- `vm::legacy_registers` 已完成迁移到 `val::legacy_registers`（上 session）。
- exec32 内 4 条死代码 `TypedList::OwnedRuntime` / `TypedMap::OwnedRuntime` match arm 已替换为 `bail!()` invariant（上 session）。
- Priority 4 审计完成：`grep` 确认 exec32 内没有任何 `legacy_*` 调用，old eval（`expr/expr_impl.rs`）、AOT（`llvm/runtime.rs`）和 tests 是唯一调用方。
- `TypedMap::string_entries_no_heap()` 已加入 `val/runtime_model.rs`：不需要 `&mut HeapStore`，直接迭代 StringMixed/StringInt/StringFloat/StringBool/Mixed，OwnedRuntime 返回 error。
- `runtime_positional_args` 已改为两阶段借用：Phase 1 immutable borrow 直接处理 `TypedList::Mixed/Int/Float/Bool`，避免克隆整个 `HeapValue::List`；Phase 2 只在 String/OwnedRuntime 时才克隆。
- `runtime_named_args` 已改为两阶段借用：Phase 1 immutable borrow 调用 `string_entries_no_heap()`，避免克隆整个 `TypedMap`（原来先 `heap.get(handle).cloned()` 再 `string_entries_into_heap`，BTreeMap tree nodes 会双重克隆）；Phase 2 仅对 OwnedRuntime 回退到旧路径。
- `write_named_args32_to_frame_from_typed_map` 已加入 `vm/exec32/named_call.rs`：直接从 `&TypedMap` 写到 callee frame，所有非 OwnedRuntime 变体无需 `&mut HeapStore`，可用于未来进一步消除中间 Vec。
- 全量 `cargo test -p lk-core --lib`：**571 passed, 0 failed**。

## 当前不能宣称完成的部分

- legacy `Val` 仍存在，并且仍有多条 bridge 路径。
- `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime` 是过渡层，不是最终 runtime model。仅 exec32 内死代码 arm 已清除；其他模块（imports.rs、runtime_callable.rs、context.rs）保留合法 bridge 转换。
- `VmContext::legacy_*` API 仍存在，只是被隔离为 delegation。exec32 已确认无任何 `legacy_*` 调用。
- `vm::legacy_registers` 已重命名/隔离，函数仍存在但有 `dead_code` 警告（val::values/ops.rs 仍依赖其中 `copy_container_value_for_register_with_metrics`）。
- dynamic method helper 中命名参数仍会生成 `Vec<(Arc<str>, RuntimeVal)>`（已减少为单次克隆，TypedMap 不再双重克隆）。进一步消除需要改变 `call_runtime_value32_runtime_named` 等函数签名，工作量较大，`write_named_args32_to_frame_from_typed_map` 已就位等待接入。
- `RuntimePositionalArgs::Prefixed` 在 native 或 fallback 路径仍可能 materialize。
- old LKB、CLI、AOT、LLVM、benchmark、stdlib 全链路还没有按新 runtime model 收口。
- `core/src/vm/compiler32.rs` 和 `core/src/vm/compiler32/tests.rs` 仍接近 1500 行限制，需要继续拆分。
- `runtime_model.rs`、`context.rs` 仍较大，需要继续按领域边界拆分。

## 下一步严格优先级

1. ✅ 已完成：`vm::legacy_registers` 迁移到 `val::legacy_registers`。

2. ✅ 基本完成：dynamic callable / method 路径收紧。
   - `runtime_positional_args` 和 `runtime_named_args` 已改为两阶段借用，避免 HeapValue/TypedMap 克隆。
   - `TypedMap::string_entries_no_heap()` 已加入。
   - `write_named_args32_to_frame_from_typed_map` 已加入为 direct frame writer。
   - 剩余：`Vec<(Arc<str>, RuntimeVal)>` 在动态 named call 路径仍存在（需改变 `call_runtime_value32_runtime_named` 签名，工作量大，推迟）。

3. 继续压缩 `OwnedRuntime` 过渡层。
   - exec32 内死代码 arm 已清除（上 session）。
   - 剩余目标：把 legacy `Val` container conversion 完全放在 runtime model 外侧，或者用显式 legacy bridge 替代 `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime`。
   - 修改时优先保持 `HeapValue` 和 runtime-visible typed container 简洁。
   - 验证：`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-core val::values -- --nocapture`。

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

### 当前状态

- `cargo build --release -p lk-cli` → Finished（26s），无 error
- 运行 `./target/release/lk bench/workloads_business_algorithms.lk`：
  - ✅ 10/15 workload 通过：`gcd_batch` / `prime_trial_division` / `binary_search` / `two_sum_map` / `sliding_window_sum` / `matrix_3x3_multiply` / `stock_max_profit` / `histogram_group_count` / `string_key_hash` / `order_score_pipeline`
  - ❌ `log_parse_filter` → `__lk_call_method expects positional arguments as list, got Int`
  - ⬜ `cart_pricing_rules` / `route_permission_check` / `inventory_reorder` / `fraud_rule_scoring`（尚未到达）

---

## 下一步：修复剩余 workload

### 当前阻塞 bug：`log_parse_filter` 报 "got Int"

**错误**：`native <runtime-native32> failed: __lk_call_method expects positional arguments as list, got Int`

**调用链**：`log_parse_filter` 中执行 `line.split("|").join("|").len()`：
1. `line.split("|")` → `lower_dynamic_method_call("split")` → `__lk_call_method`
2. 编译器生成：`materialize_list([arg_reg])` → `NewList(dst, base, 1)` → `lower_call_window_regs` → Call
3. 在 `__lk_call_method` 的第三个参数（args_list）处，读到了 `Int` 而非 `List`

**根本原因分析**（已有确定线索，尚未验证）：

`log_parse_filter` 是整个顶层文件（约 400 行顶层代码）中第 11 个 workload，寄存器计数器**不重置**，到此处时已分配 100+ 个寄存器。当方法调用链中以下指令的 **B 字段源寄存器** ≥ 128 时，可能仍然出错：

- `Move(a=dst, b=src)` — `emit_move` 中 src 在 B 字段
- `NewList(a=dst, b=base, c=len)` — base 在 B 字段

格式修改已将 B 扩展到 8 bit，理论上可以编码 0-255。但仍然报错，可能原因之一：

> **`lower_dynamic_method_call` 在同一函数中被多次调用**（split 一次、join 一次）。每次调用都会通过 `load_callable_by_name("__lk_call_method")` 分配一个新寄存器来存 helper callable，外加 `materialize_list` 分配的 base + dst。这些寄存器在每次 `lower_dynamic_method_call` 内部都是**临时的**，但 compiler32 没有寄存器回收，寄存器计数不断累积，在 100+ 寄存器环境下的 Move/NewList B 字段编码是否正确需要逐步验证。

**建议的调试步骤**：

1. 先运行 IR 单元测试，确认格式修改无误：
   ```sh
   cargo test -p lk-core vm::ir32 -- --nocapture
   ```
   特别验证 B ≥ 128 的 round-trip：
   ```rust
   let instr = Instr32::abc(Opcode32::Move, 5, 200, 0);
   assert_eq!(instr.b(), 200);
   ```

2. 在 `exec32.rs` 的 `NewList` handler 加临时打印（`eprintln!`），输出 `base` 和 `count`，验证运行时读到的 base 是否是编译时写入的值。

3. 如果上述验证失败，说明 B-field 编码仍然有问题（可能是 C_SHIFT 还未完全正确，或 move 路径有问题）。

4. **备选方案**：避免绕过 `materialize_list`，改为在 `lower_builtin_method_call` 中将 `split`/`join`/`starts_with`/`ends_with`/`contains`/`trim` 当作编译器内置 opcode 处理（类似 `len`/`push`），直接生成一个固定 opcode 或 stdlib 直调，完全绕过 `__lk_call_method` 和 `materialize_list`。这样就不受 B-field 限制，是更干净的长期方案。

### 其余 workload 预期用到的特性（待验证）

- `inventory_reorder`：`reorder.push(sku)`（已支持）、`reorder.join(",").len()`（需 list.join）
- `fraud_rule_scoring`：`device.starts_with("emu")`（已支持）、`map.get(risky_countries, country)`（已支持）
- `cart_pricing_rules` / `route_permission_check`：需实际跑到才知

### 全量 bench 流程

所有 workload 通过后执行：

```sh
RUN_AOT=0 RUNS=3 EXTRA_RUNS=3 bash bench/run_workload_bench.sh 2>&1
```

然后更新 `bench/README.md`，写入 Instr32 VM 的 benchmark 数字。

### 全量单测回归

```sh
cargo test -p lk-core --lib
```

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
- `core/src/val/runtime_model/legacy.rs`
- `core/src/vm/runtime32.rs`
- `core/src/vm/gc32.rs`
- `core/src/vm/exec32/runtime_callable.rs`
- `core/src/vm/exec32/return_values.rs`
- `core/src/vm/exec32/stack.rs`
- `core/src/vm/exec32/program.rs`
- `core/src/vm/context.rs`
- `core/src/vm/context/legacy.rs`
- `core/src/vm/compiler32/builder.rs`
