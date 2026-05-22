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

## 当前不能宣称完成的部分

- legacy `Val` 仍存在，并且仍有多条 bridge 路径。
- `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime` 是过渡层，不是最终 runtime model。
- `VmContext::legacy_*` API 仍存在，只是被隔离为 delegation。
- `vm::legacy_registers` 仍在 `vm/` 目录下，只是命名和可见性已经降级。
- dynamic method helper 中仍有位置参数、命名参数、list/map 的 materialization 路径。
- `RuntimePositionalArgs::Prefixed` 在 native 或 fallback 路径仍可能 materialize。
- old LKB、CLI、AOT、LLVM、benchmark、stdlib 全链路还没有按新 runtime model 收口。
- `core/src/vm/compiler32.rs` 和 `core/src/vm/compiler32/tests.rs` 仍接近 1500 行限制，需要继续拆分。
- `runtime_model.rs`、`context.rs` 仍较大，需要继续按领域边界拆分。

## 下一步严格优先级

1. 继续清理 legacy register 模型。
   - 目标：把 `vm::legacy_registers` 从 VM 核心命名空间进一步移走，或随 legacy `Val` ops 一起退役。
   - 建议先看 `core/src/val/values/mod.rs`、`core/src/val/values/ops.rs`、`core/src/vm/registers.rs`、`core/src/vm/mod.rs`。
   - 验证：`cargo test -p lk-core val::values -- --nocapture`、相关 replacement test、`cargo check -p lk-core -p lk-stdlib`。

2. 收紧 dynamic callable / method 调用路径。
   - 目标：减少 `context/core_methods.rs` 和 callable dispatch 中的位置参数、命名参数、receiver prefix 的临时 `Vec`。
   - 优先为 named args 和 property-call map/list 提供 view 或 direct frame writer。
   - 不要为了兼容旧 ABI 保留绕路；如果旧 helper 阻碍模型，直接删或移动到 legacy bridge。
   - 验证：`cargo test -p lk-core vm::exec32::exec32_tests -- --nocapture`、`cargo test -p lk-core vm::runtime32 -- --nocapture`。

3. 压缩 `OwnedRuntime` 过渡层。
   - 目标：把 legacy `Val` container conversion 完全放在 runtime model 外侧，或者用显式 legacy bridge 替代 `TypedList::OwnedRuntime`、`TypedMap::OwnedRuntime`。
   - 修改时优先保持 `HeapValue` 和 runtime-visible typed container 简洁。
   - 验证：`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-core val::values -- --nocapture`。

4. 审计并退役 `VmContext::legacy_*` API。
   - 目标：新 VM 的 globals、exports、natives 只依赖 runtime slots 和 runtime exports。
   - 保留旧 API 只应发生在明确 legacy bridge 内。
   - 验证：`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests -- --nocapture`。

5. 继续拆分超大文件。
   - 目标：所有单文件保持在 1500 行以内，并按真实领域边界拆分，而不是机械搬代码。
   - 优先对象：`core/src/vm/compiler32.rs`、`core/src/vm/compiler32/tests.rs`、`core/src/val/runtime_model.rs`、`core/src/vm/context.rs`。
   - 验证：每拆一块跑对应模块单测和 `cargo fmt --all -- --check`。

6. 收口 core hard gate。
   - 目标：当以上核心模块稳定后，运行 `cargo test -p lk-core --lib`。
   - 如果失败，先判断是否是旧兼容测试不再适用；当前契约允许删除旧兼容路径，但需要同步测试语义。

7. 最后再恢复 CLI / LKB / AOT / benchmark / website 文档链路。
   - 这不是当前下一步第一优先级。
   - 如果语言 spec 或网站内容受影响，按 AGENTS 要求同步 `website/src/spec/LANG.md`、`website/src/spec/LANG_zh.md`，并用 `cd website && bun run build` 验证。

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
- `core/src/val/runtime_model.rs`
- `core/src/val/runtime_model/heap.rs`
- `core/src/val/runtime_model/legacy.rs`
- `core/src/vm/runtime32.rs`
- `core/src/vm/gc32.rs`
- `core/src/vm/exec32.rs`
- `core/src/vm/exec32/runtime_callable.rs`
- `core/src/vm/exec32/return_values.rs`
- `core/src/vm/exec32/stack.rs`
- `core/src/vm/exec32/program.rs`
- `core/src/vm/context.rs`
- `core/src/vm/context/core_methods.rs`
- `core/src/vm/context/legacy.rs`
- `core/src/vm/compiler32.rs`
- `core/src/vm/compiler32/builder.rs`
