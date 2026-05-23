# LK VM 重构交接进度

本文只记录当前快照、已验证事实、未完成风险和下一步执行顺序。`plan.md` 是架构契约，不写日常流水账；本文也要保持短小，避免旧 session 历史压过当前事实。

## 当前总体状态

当前主线已经从旧 VM 兼容迁移转为新架构收口。核心路径围绕 `RuntimeVal`、slot-based `HeapStore`、`Instr32`、`Module32Artifact`、共享 runtime state、runtime callable ABI 和 native named stack/map source 展开。

当前项目未发布，不需要保持旧 LKB、旧 AOT callable bridge、旧 `Val` runtime shell 或旧 `Op` instruction enum 的向后兼容。已删除的旧路径不能作为 fallback 恢复。

## 当前完成面

- `RuntimeVal` 是唯一 runtime value model：`Nil`、`Bool`、`Int`、`Float`、`ShortStr`、`Obj(HeapRef)`。
- AST literal 已拆为 `LiteralVal`，`Expr::Literal(LiteralVal)` 不再复用 runtime value 名称。
- 旧 top-level `Val` shell、`Val::LongStr`、字符串 intern 表、自定义 clone metrics 已删除。
- `HeapStore` 使用 slot heap，`HeapRef` 是稳定句柄；typed list/map backing 已从旧容器 snapshot 迁出。
- `Instr32` / `Opcode32` / typed const pool / `Module32Artifact` 是当前可执行 artifact 路径。
- `lk compile FILE.lk` 输出 `.lkm` `Module32Artifact` JSON；`lk FILE.lkm` 直接执行 artifact。
- `.lkb` 输入和 `lkb` / `bytecode` 输出目标只保留拒绝文案，不保留执行或生成路径。
- LLVM backend 当前输出嵌入 `Module32Artifact` 的 IR shell，通过 `lk_rt_run_module32_json` 执行 Instr32 artifact。
- `lk compile exe FILE.lk` 已恢复为 host executable launcher：可执行文件内嵌 `Module32Artifact` JSON，并通过 `RuntimeVal` / `HeapStore` 新 VM 路径执行；不恢复旧 LKB 或 AOT callable bridge。
- CLI 内部命名和错误文案已从 `native executable` 收口为 `host executable launcher`，避免把 launcher 路径误标为 true native AOT。
- 旧 AOT/native callable bridge stub 已删除：`lk_rt_make_aot_function`、`lk_rt_call`、`lk_rt_call_method`、`lk_rt_call_native`、`lk_rt_register_native_module_function`。
- native named ABI 已收口为 `NativeArgs32` 的 `Empty` / `Stack` / `Map` source；旧 tuple-vector named fallback 已删除。
- `math.clamp`、`string.replace` 和 dynamic named method helper 已迁到 stack/map named source。
- `copy_native_args32_to_frame` named 参数复制已取消 tuple-vector 中转，改为直接遍历 `NativeArgs32` named source 并写入 frame。
- 跨 runtime/module value copy 的 source heap 参数已改为只读借用；`import_runtime_export` 不再 clone 整个 source module state，只读取 source heap 并把 closure 转成携带 source state 的 `RuntimeCallable32`。
- compiled program/native runtime 路径会传递共享 `Arc<Module32>`；`NativeRuntime32::shared_module()` 不再把 borrowed module 隐式 clone 成新 `Arc<Module32>`。
- `runtime_value_to_callable32` 已拆成显式 `runtime_value_to_callable32_shared` 和 `runtime_value_to_callable32_externalized`；import 路径使用 shared-state materialization，只有 `Program32Result`/测试外部化边界保留复制语义。
- `CallNamed` 调用 `FullState` native 时已移除 named stack 的 `Vec` 中转，改为与 positional args 相同的 inline slot buffer，并新增 FullState named-call 覆盖。
- `execute_compiled_module32_with_ctx` 已改为按 `Module32.globals` slot 顺序直接从 `VmContext` seed 外部 global，不再通过临时 name map 重建 globals；缺失 slot 保持 `Nil`，已覆盖不向 `VmContext` 同步回写。
- `GetIndex` 读取 list/map/object/string heap object 时不再 clone 整个 `HeapValue`；typed list/map 读取按 handle 借用容器，只在返回 long string 元素时分配目标 heap object。
- stdlib `list.push` / `list.concat` / `list.set` 已保留 typed backing；同类型写入和拼接不再 materialize 为 `Mixed`，只有异类型合并才降级。
- stdlib `iter.next` 已直接读取 typed list backing 的首元素，不再为了取首项把整个 list materialize 成 `Vec<RuntimeVal>`。
- stdlib `iter.take` / `iter.skip` 已直接对 typed list backing 做 slice，不再为了切片把 long string list 元素 materialize 成 heap string 再重建 typed backing。
- stdlib `iter.chain` 已对同类型 typed list backing 直接 concat；异类型才降级到 `Mixed`，避免 long string list chain 时额外 materialize heap strings。
- stdlib `iter.chunk` 已按 typed list backing 直接切分 chunk；输出 chunk 保留原 typed backing，避免 long string list chunk 时额外 materialize heap strings。
- stdlib `iter.zip` 已按索引从 typed list backing 读取元素，只 materialize 实际 zip 到的 long string 元素，不再先展开两侧完整列表。
- stdlib `iter.collect` 已直接复制 typed list backing，不再为了返回 list 副本而展开 long string list。
- stdlib `iter.flatten` 已对全 nested typed list 输入直接 concat backing；同类型 long string list flatten 不再 materialize heap strings。
- stdlib `iter.unique` 已对 typed int/float/bool/string backing 直接去重；long string list unique 不再 materialize heap strings。
- stdlib `stream.from_list` 已直接保存 typed list backing，不再把输入 list 预先展开成 `Vec<RuntimeVal>`。
- stdlib `stream.collect` 对 `FromList` cursor 已直接返回 typed list slice；long string list 不再为了 collect 整表 materialize 成 heap strings。
- stdlib `map.keys` / `map.values` 已按 `TypedMap` backing 直读；string-key map 直接产出 typed string key list，typed int/float/bool map values 直接产出对应 typed list。
- stdlib concurrency global `select$block` 已按 typed list 按需读取 arm type、channel、send value 和 guard；inactive send value 不再因为 helper 入口整表 materialize。
- 旧 `TypedList::materialize_mixed` consuming helper 已删除；需要 runtime value vector 的边界统一走 `runtime_values_into_heap()`。
- `iter.map` / `iter.filter` / `iter.reduce` 和 `stream.next` / `stream.collect` / blocking cursor 推进路径已迁到 `FullState` native；raw closure 优先通过 active `RuntimeModuleState32` 调用，避免为普通高阶调用复制 heap/globals。
- stdlib 高阶回调和 task callable 参数已移除无 active state 的 closure 外部化 fallback；`runtime_value_to_callable32_externalized` 不被 stdlib 使用，只保留 VM 外部化边界和测试覆盖。
- LLVM runtime 的 native import replay 已迁到 `RuntimeExport32` / `import_runtime_export`；file/module/items/namespace import 不再保留 Instr32 migration disabled stub。
- LLVM runtime 的空 `install_artifact_core_vm_builtins` hook 已删除。
- `lk_rt_run_module32_json` 执行 artifact 前会按 `Module32Artifact.imports` 注册所需 stdlib modules/concurrency globals，避免 LLVM shell 依赖旧 prelude/global helper。
- LLVM runtime 已删除旧 direct-lowering helper 表面：string interning/global handle/scalar arithmetic/compare/string contains/floor helper 和旧 immediate encoding module；LLVM backend 只保留 `Module32Artifact` shell 入口。
- LLVM runtime 不再导出旧 bundled LKB 注册入口；`lk_rt_register_bundled_module` 已从 FFI surface 删除。
- stdlib LLVM registrar exports 已移动到 `stdlib/src/llvm_bridge.rs`，并由 `lk-stdlib/llvm-bridge` feature gate；`lk-cli` 的 `llvm` feature 显式启用该 bridge。
- `core/src/op` 已整体改名为 `core/src/operator`，只承载 AST/语法层 `BinOp` / `UnaryOp`；旧 runtime `Op` instruction enum 不再存在。
- 旧 prefix optional type 兼容语法 `?T` 已删除；类型注解和 spec 只保留 canonical `T?`。
- type checker 已删除 `Expr` pointer-key `expr_types` cache，不再把 AST 地址作为类型记录 key。
- SSA pipeline 生成的 `PerformanceFacts` 已保留 list/map container value facts；container kind/known len 不再在分析阶段构造后丢弃。
- `Function32` 已携带非序列化 `PerformanceFacts`；compiler lowering 会把 literal、binary result、list/map/range container register facts 写入当前函数。
- compiler lowering 已开始把 `PerformanceFacts` 作为 typed lowering 决策源：`Move` 会传播 register facts，二元 arithmetic 会优先根据 register kind 选择 typed float opcode，facts 缺失时才回退到既有静态推断。
- register copy policy 已开始从结构变成执行事实：compiler 为 `Move` 写入 `PerfRegisterCopyFact`，container materialization 的临时值移动标记为 `move_source`，executor 按该 fact 从源寄存器取值而不是 clone。
- local slot/copy facts 已接入 compiler：参数、let/define、模式绑定和临时 call-param 绑定都会标记 `local_slots`，写入 local slot 的 `Move` 会记录 `PerfLocalCopyFact`。
- container move facts 已接入 rewritten `SetIndex` lowering：compiler 为临时 key/value 写 `PerfContainerMoveFact`，executor 按该 fact 在容器写入时 consume 对应 register；local 变量 key 不标记 move，避免改变后续读取语义。
- dead write facts 已接入纯 literal expression statement：compiler 只为无副作用、无 heap materialization 的 literal load 标记 `dead_writes`，executor 仍校验 const pool 但跳过目标寄存器写入。
- key-op facts 已接入短字符串 literal `GetIndex`：compiler 写 `PerfKeyFact.const_key`，executor 对 map/object 访问直接使用 const key，动态 key 和长字符串 key 继续走通用寄存器路径。
- key-op facts 已接入短字符串 literal `SetIndex`：rewritten map/object 写入会记录 `PerfKeyFact.const_key`，executor 对 map/object 写入直接使用固化 key，动态 key 和长字符串 key 继续走通用寄存器路径。
- control-flow facts 已在 compiler finish 阶段生成：jump/test/try patch 完成后写入 `block_ids` 和 `branch_targets`，当前作为静态 shape fact 覆盖，不改变分支执行语义。
- call-shape facts 已接入普通 `Call` 和 dynamic `CallNamed` lowering：记录 call window base、positional count、named count 和 direct closure/native target kind；executor 优先用固化 fact 构造 call window 与 callable dispatch hint，无 fact 的 artifact/手写 IR 继续按 Instr32 字段回退。
- global slot facts 已接入 `GetGlobal` / `SetGlobal` lowering：executor 优先用固化 slot fact 读写 globals，无 fact 的 artifact/手写 IR 继续按 Instr32 `Bx` 字段回退。
- runtime inline cache 已建立基础结构：`RuntimeModuleState32.inline_caches` 保存 global slot、index target/value shape 和 call shape/target kind；executor 在缺少静态 facts 时会缓存动态边界的 global slot、index shape 与 call shape，不写回 `PerformanceFacts`。
- index target shape facts 已接入 `GetIndex` lowering：compiler 记录 list/map/object/string target kind 以及 list/map value kind，executor 有 fact 时直接进入对应 index path 和 typed list/string-map read path，无 fact 时继续按 heap object kind 动态分派。
- writable index target shape facts 已接入 rewritten `SetIndex` lowering：compiler 记录 list/map/object target kind 和 list/map value kind，executor 有 fact 时直接进入对应写入路径和 typed list/string-map update path；未知和 string target 保持旧动态分派。
- 未使用的 `VmContext::snapshot()` 已删除，避免继续保留全上下文 clone 的旧边界。
- `bench/README.md` 和 `cli coverage --runtime` 的 diagnostics 文案/输出已从 BC32 fallback 与 old `Val` clone counters 改为 Instr32、copy-policy 和 heap-value movement counters。

## 最近验证

本轮完成 `operator` 模块改名、BC32 文案清理、CLI coverage metrics 清理、`lk compile exe` artifact launcher、跨 runtime/module source heap 只读化、stdlib 高阶 native `FullState` 收口和进度文档压缩后已通过：

```sh
cargo fmt --all -- --check
cargo check -p lk-core -p lk-stdlib
cargo test -p lk-core --lib
cargo test -p lk-stdlib --lib
cargo check -p lk-core -p lk-stdlib --features llvm
cargo test -p lk-core llvm --features llvm -- --nocapture
cargo test -p lk-cli
bun run build  # workdir: website
cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture
cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture
cargo test -p lk-core stmt::import -- --nocapture
cargo test -p lk-core llvm --features llvm -- --nocapture
cargo test -p lk-stdlib iter -- --nocapture
cargo test -p lk-stdlib stream_test -- --nocapture
cargo test -p lk-stdlib concurrency_task -- --nocapture
cargo test -p lk-stdlib concurrency_chan -- --nocapture
cargo test -p lk-core vm::runtime32 -- --nocapture
cargo test -p lk-stdlib iter_higher_order_ops_call_runtime_closures -- --nocapture
cargo test -p lk-core vm::exec32::exec32_tests::native::execute_module32_calls_full_state_native_with_named_args -- --nocapture
cargo test -p lk-stdlib test_math_clamp_named_arguments -- --nocapture
cargo test -p lk-stdlib test_string_replace_named_arguments -- --nocapture
cargo test -p lk-stdlib iter_higher_order_ops_call_runtime_closures -- --nocapture
cargo check -p lk-core -p lk-stdlib -p lk-cli
cargo check -p lk-cli --features llvm
cargo check -p lk-stdlib --no-default-features
cargo test -p lk-stdlib --no-default-features --lib
cargo test -p lk-core typ::type_checker -- --nocapture
cargo test -p lk-core stmt::function_test -- --nocapture
cargo test -p lk-core execute32_ -- --nocapture
cargo test -p lk-core vm::exec32::program::tests::seed_module_globals_imports_by_module_slot_order_without_name_map -- --nocapture
cargo test -p lk-core vm::exec32::exec32_tests::native::execute_program32_with_ctx_reads_external_slots_without_syncing_back_to_context -- --nocapture
cargo test -p lk-core execute32_ -- --nocapture
cargo check -p lk-core --features llvm
cargo test -p lk-core llvm --features llvm -- --nocapture
```

`cargo test -p lk-cli` 曾暴露 `cli/src/coverage.rs` 仍打印已删除的 old `Val` clone metrics；该残留已修复并重跑通过。`lk compile exe` 的 CLI 集成测试现在会编译并运行生成的 executable。

本轮补齐 call inline cache 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮补齐 `SetIndex` key movement 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.next` list materialization 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.take` / `iter.skip` typed list slicing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.chain` typed list concat 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.chunk` typed list slicing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.zip` typed list indexing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.collect` typed list copy 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.flatten` / `iter.unique` typed backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib iter -- --nocapture`。

本轮收窄 `stream.from_list` / `stream.collect` typed list backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib stream -- --nocapture`。

本轮收窄 `map.keys` / `map.values` typed map backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib map -- --nocapture`。

本轮收窄 `select$block` typed control lists 后已重跑 `cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`。

## 当前审计结果

当前 grep 期望：

```sh
rg -n "pub enum Val|Val::LongStr|Expr::Val|val::Val|record_val_clone|VAL_CLONES|IMMEDIATE_VAL_CLONES|HEAP_VAL_CLONES|values/(clone|intern)|LiteralLiteralVal|RuntimeLiteralVal|Stringing" core/src stdlib/src cli/src bench README.md docs website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# no matches

rg -n '\bOp\b|Vec<Op>|pub enum Op|enum Op|ListFoldAdd|MapValuesFoldAdd|AddRangeCountImm|BC32|bc32|packed|quickening|fallback-reason|legacy|Legacy|LEGACY|crate::op|\bop::|mod op|core/src/op' core/src stdlib/src cli/src bench README.md docs website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# only intentional matches are OptLevel/Opcode32, LKB removal/rejection text, docs saying removed bytecode VM, and plan.md contract text.

rg -n 'Arc::new\(module\.clone\(\)\)|Arc::new\(\(\*module\)\.clone\(\)\)|shared_module\(\).*or_else|let module = runtime\s*\.module\(\)' stdlib/src core/src -g '*.rs'
# no matches

rg -n 'native .*disabled|disabled during the Instr32 artifact migration' core/src stdlib/src docs bench README.md website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# no matches

rg -n 'named_stack.*to_vec|CallNamed.*to_vec|\.to_vec\(\)' core/src/vm/exec32/named_call.rs core/src/vm/exec32/support.rs core/src/vm/runtime32.rs
# no matches

rg -n 'runtime_value_to_callable32\(' core/src stdlib/src -g '*.rs'
# no matches; use explicit _shared or _externalized variants.

rg -n 'runtime_value_to_callable32_snapshot|snapshot\(&self\)|\.snapshot\(' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'runtime_value_to_callable32_externalized' stdlib/src -g '*.rs'
# no matches.

rg -n 'compile_native_executable|native_launcher_source|temp_launcher_source_path|native executable build failed|old native callable|AOT callable|lk_rt_call|lk_rt_make_aot|lk_rt_register_native' cli/src core/src docs README.md bench/README.md website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# only intentional docs mention the removed old native callable bridge.

rg -n 'lk_rt_add|lk_rt_sub|lk_rt_mul|lk_rt_div|lk_rt_mod|lk_rt_intern_string|lk_rt_to_string|lk_rt_load_global|lk_rt_define_global|lk_rt_float|lk_rt_floor|lk_rt_starts_with|lk_rt_contains|lk_rt_cmp|HandleTable|decode_immediate|encode_immediate|NIL_VALUE|BOOL_TRUE_VALUE|BOOL_FALSE_VALUE|mod encoding|llvm::encoding' core/src/llvm -g '*.rs'
# no matches.

rg -n '\bunsafe\b|extern "C"|extern "Rust"|\*mut|\*const|transmute|MaybeUninit|NonNull|lk_rt_call|lk_rt_make_aot|lk_rt_register_native' core/src stdlib/src cli/src -g '*.rs' -g '!core/src/llvm/**' -g '!stdlib/src/llvm_bridge.rs'
# only `stdlib/src/os.rs` user-facing error strings mention unsafe.
```

`.lkb` / bytecode 相关保留标准：

- CLI 对 `.lkb` 执行和 `lkb` / `bytecode` output target 的拒绝文案是有意保留。
- `docs/llvm/backend.md` / LLVM module docs 中说明 removed LKB / removed bytecode VM 是有意保留。
- 不允许恢复旧 LKB execution、旧 bytecode writer 或旧 AOT callable bridge。

## 当前不能宣称完成的部分

- native executable output 当前是 host artifact launcher，不是 native AOT；后续如果做真正 native AOT，仍必须基于 `Module32Artifact` / `RuntimeVal` / `HeapStore`，不能恢复旧 AOT callable bridge。
- 性能目标 `VM/Lua geomean <= 1.10x` 未达成；`bench/README.md` 记录的最新 quick comparison 仍明显 behind。
- `core/src/ast/parser.rs` 当前 1499 行，不能通过硬拆方式处理；后续只允许谨慎原位修改或先在其他文件降行数后再评估。
- `plan-progress.md` 已压缩为当前事实快照；后续新增进度前应优先替换旧小节，不再追加长流水账。

## 下一步执行顺序

1. 继续审计 `plan.md` 剩余契约，优先处理仍只是 launcher/shell 而非真正 native AOT 的 LLVM 面。
2. 继续补齐 object field slot shape、map/list generation 和 liveness/ownership movement 等 facts/cache 缺口。
3. 继续收窄跨 runtime/module 边界复制；snapshot materialization 只允许留在需要独立 callable 的 VM 外部化边界，热路径继续优先 active/shared state。
4. 性能工作在架构迁移闭环后再做；不要新增 benchmark-specific opcode/fusion。

## 文件行数快照

最近检查：

```text
core/src/ast/parser.rs       1499
core/src/expr/expr_impl.rs    902
core/src/vm/analysis.rs       796
core/src/vm/exec32.rs         902
core/src/vm/exec32/container.rs 1012
core/src/vm/runtime32.rs      548
core/src/vm/exec32/imports.rs 227
core/src/vm/exec32/support.rs 294
core/src/vm/exec32/runtime_callable.rs 890
core/src/llvm/runtime.rs      849
core/src/vm/compiler32.rs    1256
stdlib/src/stream.rs          868
stdlib/src/iter.rs           1077
bench/README.md              170
plan-progress.md             <1500
```

后续改动必须继续保持单文件不超过 1500 行。
