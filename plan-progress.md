# LK VM 重构交接进度

本文只记录当前快照、已验证事实、未完成风险和下一步执行顺序。`plan.md` 是架构契约，不写日常流水账；本文也要保持短小，避免旧 session 历史压过当前事实。

## 当前总体状态

当前主线已经从旧 VM 兼容迁移转为新架构收口。核心路径围绕 `RuntimeVal`、slot-based `HeapStore`、`Instr32`、`Module32Artifact`、共享 runtime state、runtime callable ABI 和 native named stack/map source 展开。

当前项目未发布，不需要保持旧二进制产物、旧 AOT callable bridge、旧 `Val` runtime shell 或旧 `Op` instruction enum 的向后兼容。已删除的旧路径不能作为 fallback 恢复。

## 当前完成面

- `RuntimeVal` 是唯一 runtime value model：`Nil`、`Bool`、`Int`、`Float`、`ShortStr`、`Obj(HeapRef)`。
- AST literal 已拆为 `LiteralVal`，`Expr::Literal(LiteralVal)` 不再复用 runtime value 名称。
- 旧 top-level `Val` shell、`Val::LongStr`、字符串 intern 表、自定义 clone metrics 已删除。
- `HeapStore` 使用 slot heap，`HeapRef` 是稳定句柄；typed list/map backing 已从旧容器 snapshot 迁出。
- `HeapStore` GC mark 阶段已改为借用 heap object 并收集子 `HeapRef` / `RuntimeCallable32` 边，不再为了递归 mark 先 clone 整个 `HeapValue`。
- `HeapValue::Stream` / `HeapValue::StreamCursor` 已携带 runtime roots，GC 可从 heap stream/cursor 对象标记 stdlib stream registry 中保存的 list/function/current value root。
- executor 自动 GC 触发点已覆盖语言级 `Raise` handler catch 值；catch 后写入 active stack 的 `ErrorVal` 在下一条指令触发 GC 时保持 live。
- `Instr32` / `Opcode32` / typed const pool / `Module32Artifact` 是当前可执行 artifact 路径。
- `lk compile FILE.lk` 输出 `.lkm` `Module32Artifact` JSON；`lk FILE.lkm` 直接执行 artifact。
- CLI 已不再特殊识别旧二进制输入或旧输出目标；执行入口只区分源码和 `.lkm` `Module32Artifact`，compile target 只保留当前支持的 Instr32/LLVM/native 目标。
- LLVM backend 已开始从 `Module32Artifact` 做 true native lowering：简单无 import 的 i64/f64/bool/nil/short string/long string literal/simple const list/simple const map return、可静态显示的 `ToString` / `ConcatString`、integer/float arithmetic、integer/float comparison、`Test` / `Jmp` CFG、简单 scalar module global slot entry、direct function call 到可静态显示 positional args/return value、caller-side f64/bool/nil args、callee-local i64/f64 arithmetic、callee i64/f64 comparison 和 callee bool/nil return 会直接生成 native `main` + `printf` IR，不再嵌入 artifact shell。
- LLVM backend 已把常量显示和 IR 文本 helper 从 `backend.rs` 拆出，`backend.rs` 降到 1328 行；新增源码级 static template string native lowering 覆盖，`"answer=${42}, ratio=${1.5}, ok=${true}"` 会生成直接 native string constant，不经过 artifact shell。
- LLVM backend 静态 `ToString` 已对齐 exec32：只支持 Nil/Bool/Int/Float/String；静态 list/map/object 不再错误按 display string native lowering。
- LLVM backend 已补齐 scalar `Not`、bool/nil equality 和 nil check 的 native lowering；`Not` 只对齐 exec32 的 Bool/Nil 语义，string/list/map/object 不走 truthiness native lowering。
- LLVM backend straight-line native lowering 已跟踪可静态显示的 module global slot，源码级 `let text = "ok"; return text;` 不再因 string `GetGlobal` 退出到 unsupported shape。
- LLVM backend 已补齐可静态显示 string equality / inequality，源码级 `let text = "ok"; return text == "ok";` 直接生成 native bool return，不经过 artifact shell。
- LLVM backend 已补齐静态 list/map equality / inequality，并支持嵌套常量容器递归比较；源码级 `[[1, 2]] == [[1, 2]]`、嵌套 map `!=` 和 direct-call callee 内 list 参数 equality 不再退出 true native lowering。
- LLVM backend 已补齐静态 object identity equality / inequality；同一静态 object alias 比较为 true，不同 object literal 即使字段相同也按 VM heap handle 语义比较为 false。
- LLVM backend simple positional direct-call 已覆盖 string 参数与 callee 内静态 string equality，`fn same(x) { return x == "ok"; } return same("ok");` 可直接 native lowering。
- LLVM backend straight-line evaluator 已对静态 int/float arithmetic 和 int/float/string comparison 做常量折叠；entry 和 direct-call callee 中 template/interpolation 的 `${1 + 2}` / `${1.5 + 2.25}` / `${1 < 2}` 可继续生成 true native string constant，不再因 `ToString` 看到 SSA numeric/bool register 退出 native lowering。
- LLVM backend 的 int/float native division/modulo 已补齐 exec32 divisor-zero 语义：static float folding 不再把除零折成 `inf` / `NaN` 后继续 `ToString`，scalar block lowering 会在 native IR 中检查 divisor zero 并以非零 exit 结束，不再直接发可能偏离 VM 的 `sdiv` / `fdiv` / `frem`。
- LLVM backend direct-call callee 已可读取 main straight-line lowering 已确认的静态 module global，`let offset = 2; fn f(x) { return x + offset; } return f(40);` 不再因为 callee `GetGlobal` 退回 unsupported shape。
- LLVM backend direct-call callee 已可把静态可显示值写回 module global 并继续读取；`counter := 1; fn set_counter() { counter = 2; return counter; } return set_counter();` 会在 native lowering 中保留这个静态 global side effect。
- LLVM backend direct-call callee 已支持可静态判定的简单 `Test` / `Jmp` 分支；`fn pick(x) { if x < 2 { return 10; } return 20; } return pick(1);` 会直接选择 native return，不恢复 artifact shell。
- LLVM backend scalar block `Test` 已按 LK truthiness 处理 `Bool` / `Nil` / `Int` / `Float`；源码级 `if 0` 固定走 truthy，`if nil` 固定走 falsy，不再因非 Bool scalar branch 退出 true native lowering。
- LLVM backend entry straight-line evaluator 已支持静态 `Test` / `Jmp` 分支选择；源码级 `if "ok"` 和 `if [1, 2, 3]` 可直接选择 native return，不需要进入 scalar block lowering 或 artifact shell。
- LLVM backend 已确认源码级静态 conditional 和 match expression native lowering 覆盖；compiler 生成的基础 `Test` / `Jmp` / `Move` 控制流可直接返回 true-native 常量结果，不恢复 artifact shell。
- LLVM backend 已确认源码级 `if let` / `match` 的 range、guard 和 or-pattern native lowering 覆盖；复杂 pattern control 会生成 native comparison/branch/add IR 或 true-native i64 return，不恢复 artifact shell。
- LLVM backend 已确认源码级 range `for` loop、inclusive/negative-step range `for` loop、range `for` 中的 `break` / `continue`、static-list indexed `for` loop、static-string indexed `for` loop 和 static-map entry tuple-pattern `for` loop native lowering 覆盖；range loop 保留 native branch/add 结构，static iterable loop 可在当前 evaluator 中直接产出 native i64 return，都不恢复 artifact shell。
- LLVM backend 已确认源码级静态 nullish/logical short-circuit native lowering 覆盖；`??`、`&&`、`||` 生成的 `IsNil` / `Test` / `Jmp` / `Move` 控制流可直接生成 true native IR，不恢复 artifact shell。
- LLVM backend direct-call callee 静态分支选择也已按 LK truthiness 处理静态 `Nil` / scalar / string/list/map 参数；`fn pick(x) { if x { return 10; } return 20; } return pick(0);` 会直接选择 native return。
- LLVM backend 已保留 const list/map 静态 shape 并补齐 `IsList` / `IsMap` native lowering，已可对 `LoadHeapConst(List/Map)` 直接生成 native bool return。
- LLVM backend 已补齐静态 string/list/map `Len` native lowering，`LoadHeapConst(List/Map)` 的长度可直接生成 native i64 return。
- LLVM backend 已补齐静态 string/list/map `GetIndex` 与 `Contains` native lowering，并增加源码级常量 list/map/string index、list/map/string contains、direct-call callee 内常量 index/contains 覆盖；静态 string value 已携带 ShortStr/heap-string key kind，string-key map contains/index/equality 按 exec32 typed string-map 语义归一，mixed map `SetIndex` 按 exec32 精确 key 语义处理，不再需要 artifact shell fallback。
- LLVM backend 已补齐静态 string/list `SliceFrom` native lowering，entry artifact 和 direct-call callee 内静态 list slice 都可直接输出 native string/list display，不再因 `SliceFrom` 退出到 unsupported shape。
- LLVM backend 已补齐静态 `MapRest` native lowering，entry 和 direct-call callee 都可按 VM map rest 语义移除静态 key 并输出 map display，不再因基础 map destructuring 退出 true native lowering。
- LLVM backend 已确认源码级静态 `if let` list/map destructuring native lowering 覆盖；list rest binding 的 `SliceFrom` 和 map rest binding 的 `MapRest` 可继续输出 true-native return，不恢复 artifact shell。
- LLVM backend 已补齐静态 `NewList` native lowering，entry 和 direct-call callee 都可从连续静态寄存器窗口构造 list display，不再因基础 list construction 退出到 unsupported shape。
- LLVM backend 已补齐静态 `NewMap` native lowering，entry 和 direct-call callee 都可从连续 key/value 寄存器窗口构造 map display，不再因基础 map construction 退出 true native lowering。
- LLVM backend 已补齐静态 `NewObject` native lowering 和 object `GetIndex`，entry 和 direct-call callee 都可构造静态对象并读取静态字段，不再因基础 struct literal / field access 退出 true native lowering。
- LLVM backend 已确认源码级静态 optional access native lowering 覆盖；`user?.score` 可在 user 为静态 object 时直接返回字段值，user 为静态 `nil` 时直接返回 `nil`，不恢复 artifact shell。
- LLVM backend 已补齐静态 `NewRange` native lowering，entry 和 direct-call callee 都可按 VM range 语义构造 typed int list display，不再因基础 range construction 退出 true native lowering。
- LLVM backend 已补齐静态 list/map/object `SetIndex` native lowering，并为静态 heap-like value 维护 alias 更新；源码级 list/map/object assignment 和 direct-call callee 内 list assignment 不再因基础 indexed mutation 退出 true native lowering。
- LLVM backend 已支持静态 `TryBegin` / `TryEnd` 和 handler-local `Raise` native lowering；entry artifact 和 direct-call callee 内的静态 handler catch path 可直接返回 VM 同形态 Error value display，不再因实际命中 `Raise` 退出 true native lowering，也不恢复 runtime shell。
- LLVM backend 静态 `IsList` 已对齐 VM iterable 语义：静态 string 和 list 都判为 true，静态 map 仍只走 `IsMap`；基础 pattern/list-like check 不再因 string shape 与 exec32 分叉。
- LLVM backend 已支持静态 `CallNamed` native lowering；entry 和 direct-call callee 内对静态函数、静态 named key、完整 named 参数窗口的调用可直接重排到 callee 参数寄存器并生成 true native IR，不再因基础 named call 退出到 artifact shell。
- LLVM backend 已支持静态 closure capture native lowering；源码级 `fn make(base) { return |value| base + value; }` 这类经 compiler 装箱为 `UpvalCell` 的闭包，可在 true native evaluator 中处理 `LoadHeapConst(UpvalCell)`、`StoreCellVal`、`MakeClosure`、`LoadCapture`、`LoadCellVal` 后继续直接调用，不再因基础不可变 capture 闭包退出到 artifact shell。
- LLVM backend 测试已拆出 `core/src/llvm/tests/basic.rs`、`core/src/llvm/tests/direct_calls.rs`、`core/src/llvm/tests/objects.rs` 和 `core/src/llvm/tests/strings.rs`，后续继续补 native lowering 覆盖时不再顶到单文件 1500 行上限。
- 未使用的 `Expr::parse_cached()` owned-clone 兼容 helper 已删除；表达式缓存只保留共享 `parse_cached_arc()` 入口。
- stdlib `time` 模块的过期“older global”迁移注释已删除，模块面只描述当前 RuntimeNative32 路径。
- unsupported LLVM/AOT shape 现在直接报错，不再 fallback 到 `lk_rt_run_module32_json` Instr32 artifact shell 或 host executable launcher；不恢复旧 AOT callable bridge。
- compiler const list/map lowering 已直接从 AST slice 构造 typed const heap value，不再为了检测常量容器临时 clone `Expr::List(elements.to_vec())` / `Expr::Map(entries.to_vec())`。
- 旧 AOT/native callable bridge stub 已删除：`lk_rt_make_aot_function`、`lk_rt_call`、`lk_rt_call_method`、`lk_rt_call_native`、`lk_rt_register_native_module_function`。
- native named ABI 已收口为 `NativeArgs32` 的 `Empty` / `Stack` / `MapHandle` source；旧 tuple-vector named fallback 和 borrowed `Map(&TypedMap)` 构造面已删除。
- `NativeArgs32::MapHandle` named source 已接入 same-runtime `RuntimeNative32`，typed-map named 参数可按 heap handle 延迟读取，不再为了普通 native named 调用 clone 整个 `TypedMap`。
- `math.clamp`、`string.replace` 和 dynamic named method helper 已迁到 stack/map named source。
- `copy_native_args32_to_frame` named 参数复制已取消 tuple-vector 中转，改为直接遍历 `NativeArgs32` named source 并写入 frame。
- 跨 runtime/module value copy 的 source heap 参数已改为只读借用；`import_runtime_export` 不再 clone 整个 source module state，只读取 source heap 并把 closure 转成携带 source state 的 `RuntimeCallable32`。
- 跨 heap/module value copy 已改为按 `&HeapValue` / `&TypedList` / `&TypedMap` 引用递归复制，避免在复制 Mixed list/map/object/error trace 前先 clone 整个 source heap object。
- `RuntimeExport32` import 路径已同步改为按 `&HeapValue` / `&TypedList` / `&TypedMap` 引用递归导入，不再在 import 边界先 clone 整个 source heap object。
- `RuntimeExport32` 的 value/state/module 字段已收窄为 crate 内部可见；跨 crate 调用改用 `new()`、`value()`、`state_lock()`、`shared_state()` 和 `shared_module()`，不再公开裸 shared-state 字段或 struct literal 构造面。
- `RuntimeExport32` 和 `VmContext` 不再实现隐式 `Clone`；需要共享 runtime state/module 的边界改用显式 `shallow_clone_shared()` / `shallow_clone_shared_runtime()`，避免重新引入 whole-context 或 whole-state snapshot 语义。
- LSP stdlib completion、unknown-export diagnostics、变量路径校验和 document symbol 输出已迁到当前 `RuntimeExport32` / `RuntimeVal` / `HeapStore` / grouped symbol 表面；旧 `Module::exports()`、`val::Val`、`Expr::Val` 和扁平 variable/import 兼容输出已删除，中文 README 集成示例也改为 `StmtParser` + `execute32_with_ctx()`。
- `RuntimeCallable32` named-map 调用边界已直接借用 caller heap 中的 `TypedMap`，不再为了跨 runtime named 参数写 frame 预先 clone 整个 named map。
- compiled program/native runtime 路径会传递共享 `Arc<Module32>`；`NativeRuntime32::shared_module()` 不再把 borrowed module 隐式 clone 成新 `Arc<Module32>`。
- `runtime_value_to_callable32` 已收口为 shared-state materialization；`runtime_value_to_callable32_externalized` 已删除，测试也不再通过 heap/globals snapshot 构造 runtime callable。
- `Program32Result::first_return_function()` 已删除；`Program32Result` 导出模块改为消费式 `into_exports()`，import 解析不再为了 export map clone 整个 heap/globals。
- `call_runtime_callable32_test` 和 `Executor32::seed_param_arg` 已收窄为 `#[cfg(test)]`；正常构建不再公开测试 callable 执行 API，也不再保留其 result-state clone 边界。
- 公开 program/context 执行入口已收口为 `execute_program32_with_ctx` / `Program::execute32_with_ctx`；旧 `execute_program32_raw_with_ctx` / `Program::execute32_raw_with_ctx` 命名面已删除。
- `RuntimeCallable32::new(module, captures, heap, globals)` 已删除；正常构建和测试都只保留 shared-state `RuntimeCallable32::with_state()` 构造路径，不再保留 heap/globals snapshot 构造器。
- `RuntimeCallable32` 的 module/function/captures/state 字段已收窄为 crate 内部可见；正常构建不再公开 callable shared-state 内部字段。
- `RuntimeCallable32` 不再实现隐式 `Clone`；需要共享 module/captures/state 的边界必须显式调用 `shallow_clone_shared()`，避免重新引入 callable snapshot 语义。
- closure captures 已统一存储为 `Arc<Vec<RuntimeVal>>`；`RuntimeCallable32::with_state()` 只接收共享 captures，不再保留 `Vec` captures 兼容入口，closure clone/import/call 不再复制 captures vector。
- `ReturnValues32::from_slice()` 已删除；exec32 `Return` 现在直接从 stack slots 构造 inline return values，返回后 callee frame 不再保留返回 heap object root，旧 `return_move` performance fact 也已删除。
- `TypedList::from_runtime_slice()` 和 exec32 `read_register_slice()` 已删除；`NewList` 的非 move-source 路径改为 executor 内部 register-window typed list builder，move-source 路径也改为 `take_register_list()`，按 register slots 直接判定 typed backing 并 consume 源槽；两条路径都不再退回先整窗 clone 成 `Vec<RuntimeVal>` 再分类。
- `LoadHeapConst(List)` 已改为 const-list 专用 typed builder，递归 materialize const elements 后按值形状显式构造 `TypedList`；全 string const list 会保留 `TypedList::String`，不再通过通用 `TypedList::from_runtime_values()` 分类。
- core serde runtime decode 的 JSON/YAML/TOML array 路径已改为 decode-local typed builder，数组值按 runtime shape 显式构造 `TypedList`，不再通过公开 `TypedList::from_runtime_values()` 通用分类边界。
- `TypedList::slice_from()`、stdlib `iter` / `stream` typed list slice helper 已从 slice `to_vec()` 改为按 typed backing 显式逐项收集，继续避免把 typed slice 当作 bulk snapshot 边界。
- stdlib `iter.collect` 和 `stream.from_list` 已删除 owned list arg helper；需要返回 list 副本或持久化 stream spec 的边界改为先借用 `TypedList`，再在本地显式复制 typed backing。
- `CallNamed` 调用 `FullState` native 时已移除 named stack 的 `Vec` 中转，改为与 positional args 相同的 inline slot buffer，并新增 FullState named-call 覆盖。
- `execute_compiled_module32_with_ctx` 已改为按 `Module32.globals` slot 顺序直接从 `VmContext` seed 外部 global，不再通过临时 name map 重建 globals；缺失 slot 保持 `Nil`，已覆盖不向 `VmContext` 同步回写。
- `GetIndex` 读取 list/map/object/string heap object 时不再 clone 整个 `HeapValue`；typed list/map 读取按 handle 借用容器，只在返回 long string 元素时分配目标 heap object。
- exec32 typed int/float/bool list 写入异类型值时已先校验 index，再按原长度一次性构造 mixed 输出，不再先 collect 整表 mixed 后二次覆盖目标 slot；新增 typed int list 污染覆盖。
- exec32 `SliceFrom` / `ToIter` 读取 list/map/string heap object 时也不再 clone 整个 `HeapValue`；读取阶段只生成返回值构造计划，释放 heap borrow 后再分配新 list/string；map `ToIter` 已改为按 `TypedMap` variant 生成迭代 snapshot，不再先通过 `typed_map_entries()` 展开成通用 runtime-entry map。
- exec32 dynamic list `+` / `-` 已按 `TypedList` backing 直读和构造结果；`+` 的异类型 fallback 已删除 `RuntimeListSnapshot::into_runtime_values()` / `string_list_to_runtime_values()` 批量展开 helper，`-` 的异形 list/list 和 remove-first fallback 也已改为按 lhs backing 过滤并保留 typed shape，不再把保留项转成 `Vec<RuntimeVal>` 后交给 `TypedList::from_runtime_values()` 重新分类；旧 `runtime_value_to_list_values` helper 已删除。
- exec32 dynamic list `+` 的同类型 concat/push 输出已改为按目标容量一次性构造 `Vec`，不再在 snapshot backing 上直接 `extend` / `push` 触发潜在二次增长；typed int concat+push 和 typed string concat 都保留 typed backing。
- exec32 dynamic map `+` / `-` 已按 `TypedMap` backing 借用合并/过滤构造结果；旧 `runtime_value_to_map_entries()` 通用展开 helper 已删除，typed string-int/float/bool map arithmetic 不再先 materialize 成 runtime-entry map。
- exec32 dynamic map `+` / `-` 的 merge 和 single-key/map-key 删除路径已从 `clone_typed_map(lhs)` 后 mutating set/remove 改为按 `TypedMap` variant 过滤构造结果；被 RHS 覆盖或删除的 lhs entry 不再被复制到临时输出，string-int/float/bool map 仍继续保留 typed backing。
- exec32 typed map equality 已改为按 `TypedMap` variant 迭代并用 `TypedMap::get()` 查 rhs；不再为了比较先构造 lhs/rhs runtime-entry map。
- `TypedMap` 自身的 cross-variant equality 和 stdlib runtime display 已改为按 variant 直接遍历；公开 `TypedMap::entries()` runtime-entry vector 展开 API 已删除。
- exec32 `MapRest` 已改为借用源 `TypedMap` 并按 variant 过滤构造 rest map，保留 string-int/float/bool map shape，不再先 clone 整个 map 再删除 key；map iteration 也改为按 `TypedMap` variant 生成 pairs。
- `VmContext` core builtins 的 `__lk_make_struct` / `__lk_merge_fields` 已按 `TypedMap` variant 读取 fields，不再通过 `entries_into_heap()`、context-local `typed_map_entries()` 或 runtime-entry `BTreeMap` 展开 typed string-key maps；`__lk_merge_fields(nil, overlay)` 直接保留 overlay typed backing，`__lk_set_field` 也不再为了返回更新后的 map/object 先 clone 整个 `HeapValue`。
- `VmContext` trait registration builtins 的 list 参数解析已改为借用 `TypedList` backing 后按 variant 构造 method entry values；旧的整 `TypedList` clone 边界已删除，string-list 只在释放 heap borrow 后按需 materialize runtime string values。
- `VmContext` trait registration builtins 已删除 `runtime_list_values()` 整表 `Vec<RuntimeVal>` 返回边界；trait method/impl entries 现在按 list 长度和索引逐项读取，只 materialize 当前字段。
- `TypedMap::entries_into_heap()` 已删除；stdlib `map.delete` 直接按 `TypedMap` variant 删除 key，并保留 `StringInt` / `StringFloat` / `StringBool` backing。
- `TypedMap::get_into_heap()` / `get_str_into_heap()` no-op materialization wrapper 已删除；core map/object access 改为直接借用 `TypedMap` / `RuntimeObject` backing。
- core method helper 的 positional string list 参数转换已改为按 typed string backing 生成 runtime args，不再 clone 整个 `HeapValue::List` 后走通用 materialization。
- core method helper 的 `list.join()` 已改为借用 source list 生成 join parts，释放 heap borrow 后再创建返回 string；不再为避开 borrow conflict clone 整个 `TypedList`。
- stdlib `list.push` / `list.concat` / `list.set` 已保留 typed backing；同类型写入和拼接不再 materialize 为 `Mixed`，只有异类型合并才降级。
- stdlib `list.push` / `list.concat` / `list.set` 入口已从 whole `TypedList` clone 改为借用 source list 生成 typed build plan；`list.concat` 的异类型 fallback 已删除 list-local `RuntimeListSnapshot::into_runtime_values()` 批量展开 helper，结果显式构造 `TypedList::Mixed`；只有对应 backing 和必要返回值被复制，string-list 降级到 mixed 的 heap materialization 延迟到释放 source borrow 后执行。
- stdlib `list.push` / `list.concat` 的同类型 typed backing 输出已改为按目标容量一次性构造 `Vec`，不再 `clone` backing 后 `push` / `extend` 触发潜在二次增长。
- stdlib `list.push` 的 int/float/bool typed backing 污染分支也改为按目标容量一次性构造 `Mixed` 输出，不再先 collect 数字项后单独 push 新值。
- stdlib `list.set` 同类型 typed backing 路径已改为按原长度一次性构造 replacement 输出；越界错误路径不再先 clone 整个 backing，成功路径也不再 clone 后原地覆盖，并继续返回新 list 和 old value、保留 `Int`/`Float`/`Bool`/`String` typed backing。
- stdlib `list.push` / `list.set` 的 string-list 污染和 typed-list 污染分支已删除 `materialize_string_values()` / `set_materialized_list()` 通用 helper，污染边界直接构造目标 `Mixed` 输出并保留 old value 返回语义。
- stdlib `list.len` / `list.join` / `list.get` / `list.first` / `list.last` 已直接借用 list backing；只有 `list.push` / `list.concat` / `list.set` 这类返回修改后新 list 的路径复制 backing。
- stdlib `iter.next` 已直接读取 typed list backing 的首元素，不再为了取首项把整个 list materialize 成 `Vec<RuntimeVal>`。
- stdlib `iter.take` / `iter.skip` 已直接对 typed list backing 做 slice，不再为了切片把 long string list 元素 materialize 成 heap string 再重建 typed backing。
- stdlib `iter.chain` 已对同类型 typed list backing 直接 concat；异类型才降级到 `Mixed`，避免 long string list chain 时额外 materialize heap strings。
- stdlib `iter.chain` 和 `iter.flatten` 内部同类型 typed backing concat 已与 `list.concat` 对齐为按目标容量一次性构造输出 `Vec`，不再 clone 左侧 backing 后 extend 右侧。
- stdlib `iter.chain` / `iter.flatten` 已从 by-value `TypedList` concat helper 改为借用 source list 生成 snapshot/plan，异类型 concat/flatten 降级时直接按 snapshot 追加到 Mixed 输出；旧 `maybe_typed_list_arg` / `typed_list_to_runtime_values` owned helper 和 iter-local `RuntimeListSnapshot::into_runtime_values()` 批量展开 helper 已删除。
- stdlib `iter.chunk` 已按 typed list backing 直接切分 chunk，入口改为借用 source list 后只复制输出 chunks；输出 chunk 保留原 typed backing，避免 long string list chunk 时额外 materialize heap strings，也不再 clone 整个输入 `TypedList`。
- stdlib `iter.zip` 已按索引从 typed list backing 读取元素，入口改为借用两侧 source list 并只 snapshot 实际 zip 到的元素；只 materialize 实际 zip 到的 long string 元素，不再先展开或 clone 两侧完整列表。
- stdlib `iter.collect` 已直接复制 typed list backing，不再为了返回 list 副本而展开 long string list。
- stdlib `iter.map` / `iter.filter` / `iter.reduce` / `iter.enumerate` 的 list input helper 已按 typed backing 生成 `RuntimeListSnapshot`；旧 `one_list` / `list_items` / `maybe_list_items` / `runtime_string_values_into_heap` 批量展开 helper 已删除。long string 元素仍需作为 callback/runtime 参数逐项分配，但现在只在实际进入 callback 或输出 pair 时 materialize，不再在入口展开整张 list。
- stdlib `iter.map` / `iter.filter` / `iter.reduce` / `iter.enumerate` 已继续删除 `into_item_snapshots()` 整表 item-vector 中转，改为直接消费 `RuntimeListSnapshot` 并逐项回调。
- stdlib `iter.flatten` 已对全 nested typed list 输入直接 concat backing；同类型 long string list flatten 不再 materialize heap strings。
- stdlib `iter.unique` 已对 typed int/float/bool/string backing 直接去重，入口改为借用 source list 后构造输出 backing；long string list unique 不再 materialize heap strings，也不再 clone 整个输入 `TypedList`。
- stdlib `iter.unique` 的 list/map equality 已按 `TypedList` / `TypedMap` variant 逐项比较；不再为了比较把 typed list/map 展成 `Vec<RuntimeVal>` 或 entry vector。
- stdlib `stream.from_list` 已直接保存 typed list backing，不再把输入 list 预先展开成 `Vec<RuntimeVal>`。
- stdlib `stream.collect` 对 `FromList` cursor 已直接返回 typed list slice；long string list 不再为了 collect 整表 materialize 成 heap strings。
- stdlib `stream` 打开 cursor 已从 `match self.clone()` 改为按引用匹配，只 clone 当前 cursor 所需字段；map/filter/take/skip/chain 不再因为打开 cursor 先复制整棵 `StreamSpec` 树。
- stdlib `map.keys` / `map.values` 已按 `TypedMap` backing 直读；string-key map 直接产出 typed string key list，typed int/float/bool map values 直接产出对应 typed list；`Mixed` / `StringMixed` values 也改为显式 shape builder，仍保留全 int/float/bool/string values 的 typed list 输出，但不再通过 `TypedList::from_runtime_values(entries.values().cloned().collect())` 通用分类边界；string values 的 mixed originals 只在实际污染时构造。
- stdlib `map.len` / `map.keys` / `map.values` / `map.has` / `map.get` 已直接借用 map backing；`map.set` / `map.delete` 也改为从借用的 `TypedMap` 按 variant 构造返回 map，不再通过 `map_arg(...).clone()` 复制整个 source map。
- stdlib `map.set` / `map.delete` 的 string-key typed map 更新已继续从 whole `entries.clone()` 后 mutating insert/remove 改为按 key 过滤构造输出；被覆盖或删除的 entry 不再复制到临时 map，`StringInt`/`StringFloat`/`StringBool` 仍保留 typed backing。
- stdlib `map.set` 私有 helper 已收窄到 public API 的 string-key 语义，删除不可达的 non-string key materialized mixed-map fallback；typed string-key map 不再保留 `set_materialized_string_map_entry` / `materialized_mixed_map_entries` 兼容面。
- core object builtin `__lk_set_field` 和 `__lk_merge_fields` 已按输出需要构造 map/object 字段；覆盖键不再先复制后覆盖，typed string map 在同类型写入/merge 后继续保留 `StringInt`/`StringFloat`/`StringBool` backing，类型污染降级到 `StringMixed` 时也按 key 过滤构造输出，不保留旧复制后覆盖 fallback。
- stdlib concurrency global `select$block` 已按 typed list handle 按需读取 arm type、channel、send value 和 guard；inactive send value 不再因为 helper 入口整表 clone/materialize。
- 旧 `TypedList::materialize_mixed` consuming helper 和公开 `TypedList::runtime_values_into_heap()` 整表 materialization API 已删除；需要 runtime value vector 的边界按 typed backing 显式构造，不再保留通用 heap materialization wrapper。
- 旧公开 `TypedList::from_runtime_values()` 通用分类入口已删除；exec32 `ToIter` map pair 直接构造 `TypedList::Mixed([key, value])`，stdlib `iter` / `stream` / `task` 的结果 list 只在 crate-local output boundary 用显式 shape builder 构造。
- 旧公开 `TypedMap::from_runtime_entries()` 通用分类入口已删除；VM 内部只保留 crate-private `typed_map_from_entries()`，stdlib/native export 等已知 string-key map 直接构造 `StringMixed` / `StringInt` backing，不再跨 crate 暴露 runtime-entry 分类 API。
- `TypedList` cross-variant equality 已改为按 typed backing 逐项比较；旧 `runtime_values_no_heap()` 临时 `Vec<RuntimeVal>` 展开 helper 已删除，短字符串仍可与 `Mixed(ShortStr)` 比较，长字符串在无 heap 上下文时不误判相等。
- `iter.map` / `iter.filter` / `iter.reduce` 和 `stream.next` / `stream.collect` / blocking cursor 推进路径已迁到 `FullState` native；runtime closure 优先通过 active `RuntimeModuleState32` 调用，避免为普通高阶调用复制 heap/globals。
- stdlib 高阶回调和 task callable 参数已移除无 active state 的 closure 外部化 fallback；`runtime_value_to_callable32_externalized` 已从 VM 外部化边界和测试覆盖中删除。
- stdlib global `spawn` callable 参数已从 shallow clone `RuntimeCallable32` 收窄为持有原 `Arc<RuntimeCallable32>`，async task 调用继续共享同一 module/state/captures，不再复制 callable struct。
- RuntimeCallable32 shared state 提交已改为按值 move 回 `Arc<Mutex<RuntimeModuleState32>>`；`call_runtime_callable32_test` 只作为测试 helper，runtime/native 热路径和错误恢复不再 clone 整个 heap/stack/global state。
- `RuntimeModuleState32` 的 heap/globals/shared stack/inline cache 字段已收窄为 crate 内部可见；外部调用改用 `heap()` / `heap_mut()` / `into_heap()` / `globals()` / `globals_mut()` / `stack()` / `stack_top()` accessor。
- `RuntimeModuleState32`、`Exec32Result` 和 `Program32Result` 不再实现 whole-state `Clone`；测试 callable helper 也改为 move state 回 shared callable，避免保留全 heap/globals/stack snapshot 能力。
- LLVM runtime 的 native import replay 已迁到 `RuntimeExport32` / `import_runtime_export`；file/module/items/namespace import 不再保留 Instr32 migration disabled stub。
- LLVM runtime 的空 `install_artifact_core_vm_builtins` hook 已删除。
- LLVM runtime 已删除旧 direct-lowering helper 表面：string interning/global handle/scalar arithmetic/compare/string contains/floor helper、旧 immediate encoding module 和 `lk_rt_run_module32_json` artifact runner。
- LLVM runtime 不再导出旧 bundled module 注册入口；`lk_rt_register_bundled_module` 已从 FFI surface 删除。
- stdlib LLVM registrar exports 已移动到 `stdlib/src/llvm_bridge.rs`，并由 `lk-stdlib/llvm-bridge` feature gate；`lk-cli` 的 `llvm` feature 显式启用该 bridge。
- 旧 `aot-runtime` staticlib crate、`lk-core/aot-minimal-runtime` 裁剪 feature 和 host executable launcher 已删除；当前 `lk compile exe` 只允许 true native-lowerable subset，不支持的 shape 直接失败。
- `core/src/op` 已整体改名为 `core/src/operator`，只承载 AST/语法层 `BinOp` / `UnaryOp`；旧 runtime `Op` instruction enum 不再存在。
- 旧 prefix optional type 兼容语法 `?T` 已删除；类型注解和 spec 只保留 canonical `T?`。
- type checker 已删除 `Expr` pointer-key `expr_types` cache，不再把 AST 地址作为类型记录 key。
- SSA pipeline 生成的 `PerformanceFacts` 已保留 list/map container value facts；container kind/known len 不再在分析阶段构造后丢弃。
- `Function32` 已携带非序列化 `PerformanceFacts`；compiler lowering 会把 literal、binary result、list/map/range container register facts 写入当前函数。
- compiler lowering 已开始把 `PerformanceFacts` 作为 typed lowering 决策源：`Move` 会传播 register facts，二元 arithmetic 会优先根据 register kind 选择 typed float opcode，facts 缺失时才回退到既有静态推断。
- register copy policy 已开始从结构变成执行事实：compiler 为 `Move` 写入 `PerfRegisterCopyFact`，container materialization 的临时值移动标记为 `move_source`，executor 按该 fact 从源寄存器取值而不是 clone。
- `Move` 的非 move-source clone 分支已接入 copy-policy runtime metrics，并按 register copy / local load / local store facts 分类 heap clone，避免 coverage counters 漏记 register copy 成本。
- local slot/copy facts 已接入 compiler：参数、let/define、模式绑定和临时 call-param 绑定都会标记 `local_slots`，写入 local slot 的 `Move` 会记录 `PerfLocalCopyFact`。
- container move facts 已接入 rewritten `SetIndex` lowering：compiler 为临时 key/value 写 `PerfContainerMoveFact`，executor 按该 fact 在容器写入时 consume 对应 register；local 变量 key 不标记 move，避免改变后续读取语义。
- cell move facts 已接入 `StoreCellVal` lowering：compiler 为 upvalue boxing、cell assignment 和 compound cell assignment 写 `PerfCellMoveFact`，executor 只在该 fact 存在时 consume source register，手写 IR 无 fact 时仍保持 clone 语义。
- global move facts 已接入 `SetGlobal` lowering：direct global assignment 会在 `PerfGlobalFact` 中标记 `move_source` 并让 executor consume 源寄存器；top-level local/global 同步和无 fact artifact 继续 clone，避免破坏后续 local 读取。
- container build facts 已接入 `NewList` / `NewMap` lowering：compiler 为 list/map literal materialization 写 `PerfContainerBuildFact`，executor 只在 fact 存在时 consume 连续临时窗口；`NewList` 的 consume 分支直接构造 typed backing 并清空源 slots，手写 IR / 无 fact artifact 继续 clone 源寄存器。
- container literal 源到临时窗口的 copy policy 已收窄：local/param 源保持 clone，非 local 临时表达式源继续 move，避免 `[x]` / `{key: value}` 构造后把后续仍需读取的 local 清成 `Nil`。
- exec32 dynamic list `+` 已从 by-value `TypedList` concat/push/prepend helper 改为 list operand snapshot；异类型 concat 只有 finalize 阶段才按需 materialize string list，旧 `typed_list_to_runtime_values` / `concat_typed_lists` / `push_typed_list` / `prepend_typed_list` helper 已删除。
- exec32 dynamic list `-` 已同步迁到 list operand snapshot，删除旧 `runtime_value_to_typed_list` / `typed_list_item_to_runtime_value` / `list_snapshot_item_to_runtime_value` owned helper；list 差集和 remove-first 现在按 lhs backing 过滤，跨数字形状比较仍保留 lhs typed list backing。
- exec32 `SetIndex` string-list 污染路径已从 clone 整个 string backing 改为 `mem::take` 原 backing 后 materialize mixed list，不再为了从 `String` 降级到 `Mixed` 复制整张 string list。
- `TypedMap::from_runtime_entries()` 已改为消费式 string-key shape 分类；string-key mixed map 构造不再为了选择 backing 额外 clone value。
- dead write facts 已接入纯 literal expression statement：compiler 只为无副作用、无 heap materialization 的 literal load 标记 `dead_writes`，executor 仍校验 const pool 但跳过目标寄存器写入。
- key-op facts 已接入短字符串 literal `GetIndex`：compiler 写 `PerfKeyFact.const_key`，executor 对 map/object 访问直接使用 const key，动态 key 和长字符串 key 继续走通用寄存器路径。
- key-op facts 已接入短字符串 literal `SetIndex`：rewritten map/object 写入会记录 `PerfKeyFact.const_key`，executor 对 map/object 写入直接使用固化 key，动态 key 和长字符串 key 继续走通用寄存器路径。
- control-flow facts 已在 compiler finish 阶段生成：jump/test/try patch 完成后写入 `block_ids` 和 `branch_targets`，当前作为静态 shape fact 覆盖，不改变分支执行语义。
- call-shape facts 已接入普通 `Call` 和 dynamic `CallNamed` lowering：记录 call window base、positional count、named count 和 direct closure/native target kind；executor 优先用固化 fact 构造 call window 与 callable dispatch hint，无 fact 的 artifact/手写 IR 继续按 Instr32 字段回退。
- closure call window 到 callee frame 的参数传递已改为 consume caller call-window slot；普通 positional 和 dynamic named closure call 都不再通过 `clone_from_slice` 复制参数窗口，返回写回前参数槽会被置为 `Nil`。
- runtime callable receiver/method 路径的 prefixed positional args 已从 heap `Vec` materialization 改为 inline call buffer；typed-map named closure 写 frame 也直接读取 `RuntimePositionalArgs`，不再为了 receiver + args 拼临时 slice。
- core method helper 的 positional args 已从 `Vec<RuntimeVal>` 快照改为 `MethodPositionalArgs` list-handle source；callable 属性、named callable 和 trait method dispatch 直接把 list handle 传入 runtime callable ABI，builtin method 分支才按需用 inline buffer 展开，并新增源码级 `"red,green".split(",").join("|")` 覆盖。
- core method helper 的 builtin inline positional 展开已同步收窄 typed string list 路径：短字符串直接写 `ShortStr`，长字符串逐项 clone `Arc<str>` 并延迟到释放 heap borrow 后分配，不再 clone 整个 string backing、先转 `String`，也不再为全量 string 参数构造中转 `Vec<MethodStringArg>`。
- runtime callable list-handle positional 参数复制已收窄 typed string list 路径：短字符串直接生成 `ShortStr` 写入调用窗口，长字符串只逐项 clone 对应 `Arc<str>` 并在释放 heap borrow 后分配，不再为避开 borrow conflict 先 clone 整个 string backing，也不再为全量 string 参数构造中转 `Vec<RuntimeStringArg>`。
- FullState native call 的 positional 和 named call-window slots 已改为 move 到 inline native args buffer；旧 clone-based `inline_native_args_from_stack` / `inline_native_slots_from_stack` helper 已删除，FullState native 返回前临时参数槽会被置为 `Nil`。
- `Call` / `CallNamed` 返回写回前会统一清空 caller call-window 的 positional 和 named 临时参数槽；plain/context native、Runtime32 callable、closure 和 FullState native 都不再让参数窗口继续作为 GC root 存活。
- `Call` / `CallNamed` dispatch 已从 clone 整个 `CallableValue` 收窄为抽取轻量 `CallableTarget32`；closure/native/Runtime32 callable 分支只 clone 必要的 `Arc` 或 native function handle，不再保留整 callable snapshot dispatch 边界。
- stdlib `iter` 高阶回调调用也已移除 heap callable snapshot clone：iter 回调现在借用 heap object 判别 callable target，只抽取 runtime callable `Arc` 或 native function handle；closure 回调直接用原 heap handle 回到 active `RuntimeModuleState32`。
- stdlib `stream` 高阶回调调用也已移除 heap callable snapshot clone：stream 回调现在借用 heap object 判别 callable target，只抽取 runtime callable `Arc` 或 native function handle；closure 回调直接用原 heap handle 回到 active `RuntimeModuleState32`。
- same-runtime closure callable 和 closure call-window helper 已保证校验/调用错误路径恢复 `RuntimeModuleState32`、caller frame、`stack_top`、`pc`、captures 和 register count，不再在 shared stack ABI 错误早退时遗失 active runtime state。
- same-runtime closure call 现在会保存并恢复 handler stack depth；callee 在 `try` 内提前 `return` 时不再把 stale handler 留给后续 caller/callee raise 路径。
- global slot facts 已接入 `GetGlobal` / `SetGlobal` lowering：executor 优先用固化 slot fact 读写 globals，无 fact 的 artifact/手写 IR 继续按 Instr32 `Bx` 字段回退。
- runtime inline cache 已建立基础结构：`RuntimeModuleState32.inline_caches` 保存 global slot、index target/value shape 和 call shape/target kind；executor 在缺少静态 facts 时会缓存动态边界的 global slot、index shape 与 call shape，不写回 `PerformanceFacts`；普通 `Call` 和 `CallNamed` 动态 cache 测试已断言执行前后对应 `Function32.performance.call_site` 仍为空。
- `HeapStore` 已为 heap slot 维护 shape generation；容器写入和 slot 复用会 bump generation，动态 index inline cache 以 `HeapRef + generation` 为 guard，避免跨对象或跨形状复用陈旧 shape。
- `RuntimeObject` 已维护 field slot shape；对象写入统一通过 `set_field()` 更新 shape，动态 object `GetIndex` 会在 generation guard 后用缓存的 field slot 读取。
- index target shape facts 已接入 `GetIndex` lowering：compiler 记录 list/map/object/string target kind 以及 list/map value kind，executor 有 fact 时直接进入对应 index path 和 typed list/string-map read path，无 fact 时继续按 heap object kind 动态分派。
- writable index target shape facts 已接入 rewritten `SetIndex` lowering：compiler 记录 list/map/object target kind 和 list/map value kind，executor 有 fact 时直接进入对应写入路径和 typed list/string-map update path；未知和 string target 保持旧动态分派。
- 未使用的 `VmContext::snapshot()` 已删除，避免继续保留全上下文 clone 的旧边界。
- `bench/README.md` 和 `cli coverage --runtime` 的 diagnostics 文案/输出已从 BC32 fallback 与 old `Val` clone counters 改为 Instr32、copy-policy 和 heap-value movement counters。
- exec32 dynamic list prepend 的异类型路径已删除 `prepend_runtime_values()` helper，改为按 list snapshot 直接构造 `TypedList::Mixed`，新增 typed string list 前置非字符串值覆盖。
- core exec32、stdlib `list` 和 stdlib `iter` 的 mixed fallback 已删除旧 `append_runtime_values()` / `append_string_list_runtime_values()` helper 表面，改为只在 mixed-output 边界显式 `append_to_mixed_output()`。
- stdlib `map.values` 已删除 decode 风格的 `runtime_values_list()` 整表备用 clone，改为 map-local 渐进 shape builder；typed 输出不再预先 clone mixed fallback，只有实际污染到 mixed 时才构造 mixed values。core JSON/YAML/TOML decode 边界也已改为 `decoded_values_to_typed_list()` 命名，避免继续暴露通用 runtime-values helper 表面。
- stdlib `map.values` 的 numeric/string shape 污染边界已继续收窄：从 typed values 退到 `Mixed` 时改为 final-capacity 手动构造输出，不再通过 `into_iter().map(...).collect::<Vec<_>>()` 先生成中转；新增 numeric-to-mixed 污染覆盖。
- `CallableValue::RuntimeNative32` 的 `name` 字段迁移已补齐到 call target、`LoadNative`、跨 heap/module callable copy 和 runtime display；native callable display name 不再在旧 pattern/initializer 中丢失，也不再阻断全包编译。
- exec32 非 move-source `NewList` 的 register-window typed builder 已改为先扫描最终 shape，再只构造对应 typed backing；不再同时预分配 int/float/bool/string 四套候选 Vec，只有真正 mixed 时才 clone 源 slots。
- stdlib `string.format` 已从 `chars().collect::<Vec<_>>()` 的全格式串 materialization 改为 `Peekable<char>` 单遍扫描；仅为识别 `{}` 占位符而存在的临时 char Vec 已删除。
- compiler for-object pattern binding 已直接按 object field keys 生成 map contains 条件；不再为了复用 map pattern condition 先构造 `(key, Pattern::Wildcard)` 临时 Vec。
- stdlib runtime display 已改为流式拼接 list/map/object 输出；不再先收集 `Vec<String>` / entry Vec 后 join，nested typed containers display 仍保持原格式。

## 最近验证

当前已验证命令摘录：

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
cargo test -p lk-stdlib map -- --nocapture
cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp
cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm
cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture
cargo test -p lk-stdlib string -- --nocapture
cargo test -p lk-core vm::compiler32::tests -- --nocapture
cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture
cargo test -p lk-stdlib runtime_native -- --nocapture
```

`cargo test -p lk-cli` 曾暴露 `cli/src/coverage.rs` 仍打印已删除的 old `Val` clone metrics；该残留已修复并重跑通过。`lk compile exe` 的 CLI 集成测试现在会编译并运行生成的 executable。

本轮补齐 call inline cache 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。后续又补充普通 `Call` / `CallNamed` 动态 cache 不写回静态 `PerformanceFacts` 的断言，并重跑 `cargo test -p lk-core execute_module32_caches_call_shape_without_static_fact -- --nocapture`、`cargo test -p lk-core execute_module32_caches_named_call_shape_without_static_fact -- --nocapture` 和 `cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`。

本轮删除 CLI 旧二进制输入和旧 output target 的专用识别分支；执行入口只特殊处理当前 `.lkm` artifact，compile target 只识别当前支持的 LLVM/native target 或默认 Instr32 artifact。同步移除旧二进制 corrupted-magic 负测和旧命名断言，并把 LLVM CLI direct-call 测试断言更新为当前 constant-folded true-native IR。已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-cli`。

本轮补齐 `SetIndex` key movement 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.next` list materialization 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 exec32 dynamic list arithmetic typed backing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮继续收窄 exec32 dynamic list `+` 和 stdlib `list.concat` 的异类型 fallback，删除 arithmetic/list-local `RuntimeListSnapshot::into_runtime_values()` / `string_list_to_runtime_values()` 批量展开 helper，异类型结果显式构造 `TypedList::Mixed`，同类型 backing 仍保留。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core operator::operator_test::tests::literal_list_operations -- --nocapture`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep 和 unsafe grep。

本轮继续收窄 exec32 dynamic list `-` 的异形 fallback，删除未使用的 `list_snapshot_item_to_runtime_value()` / `list_snapshot_items_equal()`，list/list 差集和 remove-first fallback 改为按 lhs backing 过滤并保留 typed shape；新增跨数字形状 subtraction 覆盖，确认 `Int` lhs 减 `Float` rhs 后仍返回 `TypedList::Int`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_subtracts_cross_numeric_list_without_reclassifying_lhs_backing -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core operator::operator_test::tests::literal_list_operations -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮继续收窄 stdlib `iter.chain` / `iter.flatten` 的异类型 fallback，删除 iter-local `RuntimeListSnapshot::into_runtime_values()` / `string_snapshot_to_runtime_values()` 批量展开 helper，fallback 改为 append-style 构造 Mixed 输出；当前 materialization-helper grep 只剩 `stdlib/src/list.rs` 的 borrowed `one_list()` arity helper。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、helper grep 和 unsafe grep。

本轮收窄 exec32 dynamic map arithmetic typed backing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_adds_and_subtracts_typed_string_int_maps_without_runtime_entry_materialization -- --nocapture`、`cargo test -p lk-core operator::operator_test::tests::literal_map_operations -- --nocapture`、`cargo test -p lk-core --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、map-entry materialization grep 和 unsafe grep。

本轮继续收窄 exec32 dynamic map arithmetic：`map + map` 现在先过滤 RHS 覆盖键再构造输出，不再复制会被覆盖的 lhs entry；`map - key` 和 `map - map` 也按 typed map backing 过滤构造结果，不再先 clone 整个 lhs map 再删除 key；新增 string-int map single-key deletion 覆盖。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core execute32_subtracts_string_key_from_typed_string_int_map_without_cloning_removed_entry -- --nocapture`、`cargo test -p lk-core execute32_adds_and_subtracts_typed_string_int_maps_without_runtime_entry_materialization -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core operator::operator_test::tests::literal_map_operations -- --nocapture` 和 `cargo check -p lk-core`。

本轮收窄 exec32 typed map equality entry materialization 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core operator::operator_test::tests::nested_literal_comparisons -- --nocapture`、`cargo test -p lk-core operator::operator_test::tests::literal_map_operations -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、map-entry/equality grep、unsafe grep 和 `git diff --check`。

本轮收窄 exec32 `MapRest` / map iteration typed map backing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_to_iter_materializes_map_entries_as_pairs -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 `VmContext` core field helpers typed map backing 后已重跑 `cargo test -p lk-core vm::context -- --nocapture`。

本轮继续收窄 `map.values` mixed 污染构造，并补齐 `RuntimeNative32.name` 字段迁移残留和 LLVM feature 下的 runtime callable display accessor。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture`、legacy/materialization grep、unsafe grep、单文件行数检查、`git diff --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`。

本轮收窄非 move-source `NewList` register-window typed builder：先复用 runtime slot shape scan，再只分配最终 typed backing；mixed 路径保留语义所需 clone。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`。

本轮收窄 stdlib `string.format` format-string 扫描：删除整串 `Vec<char>` materialization，改为 `Peekable<char>` 单遍处理 `{}`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib string -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 compiler for-object pattern condition 构造：新增 key-only map condition helper，for-object binding 直接传 field key iterator，不再构造 wildcard pattern Vec。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::tests -- --nocapture`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、unsafe grep 和单文件行数检查。

本轮收窄 stdlib runtime display 输出构造：list/map/object display 改为直接向 `String` 追加内容，删除 map/object entry Vec 和 list value Vec 的中转 join。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib runtime_native -- --nocapture`、`cargo test -p lk-stdlib string -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 `VmContext` trait registration helper 的 whole-list clone 边界，`runtime_list_values()` 不再先 clone 整个 `TypedList`，而是借用 backing 后按 variant 构造返回 values；string backing 只复制 `Arc<str>` 并在释放 heap borrow 后按需 materialize。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core trait -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm` 和 unsafe grep。

本轮收窄 `iter.take` / `iter.skip` typed list slicing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 stdlib `list.set` typed backing 错误路径：同类型 `Mixed`/`Int`/`Float`/`Bool`/`String` set 先校验 index，再复制 backing 构造返回 list；越界时不再先复制整表。已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib list -- --nocapture`。

本轮收窄 runtime callable list-handle positional 参数复制：typed string list 不再 clone 整个 backing，短字符串直接写 inline call frame，长字符串逐项延迟分配；method helper 覆盖已改为 long-string split/join，确保走长字符串参数路径。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core execute_program32_method_helper_uses_list_handle_positional_args -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture` 和 `cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`。

本轮同步收窄 core method helper builtin inline positional 展开：typed string list 不再 clone 整个 backing，也不再先转 `String`；同时 `StreamSpec::open_cursor()` 不再 clone 整棵 stream spec。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core execute_program32_method_helper_uses_list_handle_positional_args -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-stdlib stream -- --nocapture` 和 `cargo test -p lk-stdlib stream_test -- --nocapture`。

本轮收窄 core object/map builtins：`__lk_set_field` 和 `__lk_merge_fields` 对 typed string map 按覆盖键过滤构造输出，不再 clone 整张 map 后覆盖；`__lk_set_field` 的 typed map 污染分支也删除旧 copy-then-set fallback，直接过滤构造 `StringMixed`。新增覆盖键、typed backing 保留和污染分支 len 不增长测试。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core core_set_field_preserves_typed_string_int_map_without_copying_overwritten_entry -- --nocapture`、`cargo test -p lk-core core_set_field_pollutes_typed_map_without_copying_overwritten_entry -- --nocapture`、`cargo test -p lk-core core_merge_fields_filters_base_keys_overwritten_by_overlay -- --nocapture`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 stdlib `list.push` / `list.concat` 同类型 typed backing 构造：输出 `Vec` 按目标容量一次性分配并复制左右/新增元素，不再 clone 后 push/extend。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib test_list_direct_runtime_call_preserves_typed_backing -- --nocapture`、`cargo test -p lk-stdlib test_list_direct_runtime_concat_preserves_typed_backing -- --nocapture`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮同步收窄 stdlib `iter.chain` 同类型 typed backing concat：输出 `Vec` 按目标容量一次性构造，不再 clone 左侧后 extend。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter_direct_runtime_call_preserves_typed_lists -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 `iter.chain` typed list concat 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.chunk` typed list slicing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.zip` typed list indexing 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.collect` typed list copy 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮收窄 `iter.flatten` / `iter.unique` typed backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib iter -- --nocapture`。

本轮收窄 stdlib `iter` runtime list/map equality materialization 与 `select$block` control-list clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy/materialization grep、unsafe grep 和 `git diff --check`。

本轮收窄 `stream.from_list` / `stream.collect` typed list backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib stream -- --nocapture`。

本轮补齐 `StreamValue` / `StreamCursorValue` runtime roots 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core heap_store_gc_marks_stream_and_cursor_roots -- --nocapture`、`cargo test -p lk-stdlib stream -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy/materialization grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend 简单 i64 entry native lowering 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_simple_i64_return_without_instr32_shell -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo test -p lk-cli --features llvm`、`cargo test -p lk-core --lib --features llvm`、legacy/materialization grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend 简单 i64 compare/branch CFG native lowering 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_simple_i64_return_without_instr32_shell -- --nocapture`、legacy/AOT source grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend 简单 i64 global slot 与源码级 `while` loop native lowering 后已重跑 `cargo fmt --all`、`cargo test -p lk-core llvm_backend_lowers_source_while_i64_loop_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_i64_loop_without_instr32_shell -- --nocapture`、legacy/AOT source grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend bool return native print lowering 后已重跑 `cargo fmt --all`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo test -p lk-cli --features llvm test_llvm_compile_lowers_bool_return_without_instr32_shell -- --nocapture`。

本轮补齐 LLVM backend nil return native print lowering 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo test -p lk-cli --features llvm test_llvm_compile_lowers_nil_return_without_instr32_shell -- --nocapture`。

本轮补齐 LLVM backend f64 return / arithmetic / scalar global native lowering 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo test -p lk-cli --features llvm test_llvm_compile_lowers_f64_return_without_instr32_shell -- --nocapture`。

本轮补齐 LLVM backend f64 comparison / branch native lowering，并把 backend scalar-lowering helper 从 `native_i64_*` 命名收口为 `native_scalar_*` 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo test -p lk-cli --features llvm test_llvm_compile_lowers_f64_branch_without_instr32_shell -- --nocapture`。

本轮补齐 LLVM backend 简单 short string return native lowering 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_simple_short_string_return_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_short_string_return_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

历史记录：`lk compile exe` 曾从固定 host launcher 改为 native-first，后来 host launcher fallback 已删除；当前 unsupported runtime value shape 直接失败。

本轮补齐 LLVM backend 简单 long string literal return native lowering，`LoadHeapConst(LongString)` return 不再嵌入 artifact shell，且 `compile exe` 会直接走 native clang 路径。已重跑 `cargo fmt --all`、`cargo test -p lk-core llvm_backend_lowers_simple_long_string_return_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_long_string_return_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend 简单 const list return native lowering，`LoadHeapConst(List)` 中可静态显示的常量 list return 不再嵌入 artifact shell，且 `compile exe` 会直接走 native clang 路径；未覆盖复杂 runtime shape 当前直接报错。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_simple_const_list_return_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_const_list_return_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend 简单 const map return native lowering，`LoadHeapConst(Map)` 中可静态显示的常量 map return 不再嵌入 artifact shell，且 `compile exe` 会直接走 native clang 路径；未覆盖 callable/import/native runtime 等复杂 shape 当前直接报错。已重跑 `cargo fmt --all`、`cargo test -p lk-core llvm_backend_lowers_simple_const_map_return_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_const_map_return_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend 零参数 direct function call native lowering，入口函数中 `LoadFunction` / function binding `SetGlobal` / `Move` / `Call argc=0` 且 callee 为无参数无 capture、straight-line、返回可静态显示值时不再嵌入 artifact shell，`compile exe` 会直接走 native clang 路径。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_zero_arg_direct_function_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend direct-call callee-local i64 arithmetic native lowering，callee straight-line `AddInt` / `SubInt` / `MulInt` / `DivInt` / `ModInt` 会内联生成合法 SSA 指令，不再因为函数体算术退出 native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_i64_arithmetic_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_zero_arg_direct_function_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep 和 unsafe grep。

本轮补齐 LLVM backend direct-call callee-local f64 arithmetic、callee bool return 和 callee nil return native lowering，zero-arg direct call 不再因这些基础标量返回退出 native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_f64_arithmetic_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_bool_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_nil_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_zero_arg_direct_f64_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend direct-call callee-local i64/f64 comparison native lowering，zero-arg direct call callee 中 `CmpInt` / `CmpNeInt` / ordered numeric comparisons 会内联生成 `icmp` / `fcmp` + `zext`，不再因基础比较返回 bool 退出 native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_i64_compare_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_zero_arg_direct_function_call_f64_compare_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_zero_arg_direct_compare_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend simple positional direct-call native lowering，caller call window 中可静态显示 positional args 会 seed 到 callee registers，`fn f(x) { return x + 1; } return f(41);` 不再嵌入 artifact shell，`compile exe` 直接生成 native executable。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_simple_positional_direct_function_call_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_positional_direct_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮补齐 LLVM backend caller-side f64 positional direct-call native lowering，含 `LoadFloat` / caller-side f64 arithmetic/compare 的 entry 会优先走 straight-line direct-call evaluator，`fn f(x) { return x + 2.25; } return f(1.5);` 不再因入口 `LoadFloat` 误进 block lowering 后退出 native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_f64_positional_direct_function_call_without_shell --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm test_llvm_compile_lowers_f64_positional_direct_call_without_instr32_shell -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 `map.keys` / `map.values` typed map backing 后已重跑 `cargo fmt --all -- --check` 和 `cargo test -p lk-stdlib map -- --nocapture`。

本轮继续收窄 stdlib `map.set` / `map.delete` typed backing 更新：typed string map 更新按 key 过滤构造输出，不再 `entries.clone()` 后 insert/remove；direct runtime set 覆盖改为断言 overwrite existing string-int key 后仍保持 `TypedMap::StringInt` 且 len 不增长。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map::tests::test_map_direct_runtime_call_preserves_typed_map -- --nocapture`、`cargo test -p lk-stdlib map -- --nocapture` 和 `cargo check -p lk-stdlib`。

本轮收窄 `select$block` typed control lists 后已重跑 `cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`。

本轮删除 `TypedMap::entries_into_heap()`，收窄 `map.delete`、core positional string args 和 `iter` list input helper 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。后续 `iter.map/filter/reduce/enumerate` 已继续从入口 `Vec<RuntimeVal>` 展开改为 `RuntimeListSnapshot`，并新增 long string callback 失败路径覆盖，确认不会预先 materialize 未访问元素。

本轮补齐 heap shape generation 与 index inline cache generation guard 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::cache32 -- --nocapture`、`cargo test -p lk-core heap_store_shape_generation -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `git diff --check`。

本轮补齐 object field slot shape 与 object access cache 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::cache32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_allocates_object_and_reads_string_field -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy grep、unsafe grep 和 `git diff --check`。

本轮收窄 closure call 参数窗口复制后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`。

本轮收窄 runtime callable receiver/method prefixed args materialization 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::tests::compiler32_dynamic_method_helper_calls_runtime_callable_property -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo test -p lk-core vm::context -- --nocapture`。

本轮收窄 FullState native call 参数窗口复制后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::native::execute_module32_calls_full_state_native_with_named_args -- --nocapture` 和 `cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`。

本轮收窄 RuntimeCallable32 shared state 提交 clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture` 和 `cargo test -p lk-core stmt::import -- --nocapture`。

本轮统一清理 call-window 临时参数槽后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo test -p lk-core vm::compiler32::tests::compiler32_dynamic_method_helper_calls_runtime_callable_property -- --nocapture`。

本轮修复 shared stack/closure callable 错误早退恢复后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy grep、unsafe grep 和 `git diff --check`。

本轮收窄跨 heap copy 的 source container clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy grep、unsafe grep 和 `git diff --check`。

本轮收窄 `RuntimeCallable32` named-map 调用边界后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep、unsafe grep 和 `git diff --check`。

本轮清理剩余 call/capture 复制 helper 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy grep、unsafe grep、`rg -n "clone_from_slice|read_register_range_owned|checked_u8_count" core/src/vm -g '*.rs'` 和 `git diff --check`。

本轮删除旧 `aot-runtime` crate 与 `aot-minimal-runtime` cfg 后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep 和 `git diff --check`。

本轮收窄 `Program32Result` callable 外部化边界后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core stmt::function_test -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep 和 `git diff --check`。

本轮收窄 test callable 测试边界并清理剩余 return/list slice `to_vec()` 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::return_values -- --nocapture`、`cargo test -p lk-core stmt::function_test -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy/snapshot grep、unsafe grep 和 `git diff --check`。

本轮收窄 typed list slice bulk copy 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib stream -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、slice/to_vec grep 和 `git diff --check`。

本轮收窄 `RuntimeCallable32::new` snapshot 构造器后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core val::runtime_model::heap -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib` 和 `git diff --check`。

本轮收窄 `RuntimeExport32` import heap traversal 和 `RuntimeCallable32` 字段可见性后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core val::runtime_model::heap -- --nocapture`、`cargo test -p lk-core vm::gc32 -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib` 和 `git diff --check`。

本轮收窄 `RuntimeExport32` 公开字段和 struct literal 构造面后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo test -p lk-core vm::gc32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`、`cargo test -p lk-stdlib globals_test -- --nocapture`、`cargo test -p lk-core --lib` 和 `cargo test -p lk-stdlib --lib`。

本轮收窄 `RuntimeModuleState32` globals/shared stack/inline cache 字段后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::runtime32 -- --nocapture` 和 `cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`。

本轮补齐 `NewList` / `NewMap` container build movement facts，收窄 container literal local source move，并收窄 `TypedMap::from_runtime_entries()` 构造 clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core vm::compiler32::tests::compiler32_marks_container_materialization_moves_as_source_moves -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep、unsafe grep 和 `git diff --check`。

本轮继续收窄 `NewList` move-source 分支，删除 `take_register_values()` 中间 `Vec<RuntimeVal>` 构造，改为 `take_register_list()` 直接从 mutable register slots 判定 typed shape、consume 源 slots 并构造 `TypedList`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_new_list_build_fact_consumes_source_register -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm` 和 helper grep。

本轮继续收窄 `LoadHeapConst(List)` runtime materialization，删除 const list 先收集 `Vec<RuntimeVal>` 再调用 `TypedList::from_runtime_values()` 的通用分类边界，改为 const-list 专用 shape builder；新增 heap const string list 覆盖，确认 short/long string const list 直接保留 `TypedList::String` backing。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_load_heap_const_list_preserves_typed_string_backing -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 core serde runtime decode 的 JSON/YAML/TOML array typed construction，三条 array 路径不再调用 `TypedList::from_runtime_values()`，改用 decode-local shape builder 显式保留 int/float/bool/string typed backing。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core val::de -- --nocapture`、`cargo test -p lk-stdlib json_parse32_decodes_into_runtime_values -- --nocapture`、`cargo test -p lk-stdlib yaml_parse32_decodes_into_runtime_values -- --nocapture`、`cargo test -p lk-stdlib toml_parse32_decodes_into_runtime_values -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 `HeapStore` GC mark 阶段整对象 clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core val::runtime_model::heap -- --nocapture`、`cargo test -p lk-core vm::gc32 -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep、unsafe grep、GC mark clone grep 和 `git diff --check`。

本轮收窄 closure captures 共享表示和 `RuntimeCallable32::with_state()` captures API 后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core val::runtime_model::heap -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、captures grep、legacy grep、unsafe grep 和 `git diff --check`。

本轮收窄 same-runtime `RuntimeNative32` typed-map named 参数 clone，新增 `NativeArgs32::MapHandle` 后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep、unsafe grep 和 `git diff --check`。

本轮删除 borrowed `NativeArgs32::Map(&TypedMap)` 构造面，并把 exec32 `MapRest` 从整图 clone 后删除改为按 `TypedMap` variant 过滤构造后，已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、legacy grep 和 unsafe grep。

本轮收窄 stdlib `map.set` / `map.delete` 的 source map clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、whole-map clone grep 和 `git diff --check`。

本轮迁移 stdlib 外部 heap 访问并收窄 `RuntimeModuleState32.heap` 后已重跑 `cargo fmt --all`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib` 和 `git diff --check`。

本轮删除剩余 externalized callable snapshot / `RuntimeCallable32::new(heap, globals)` 测试构造器，并把 `Program32Result::exports()` 改为消费式 `into_exports()` 后已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core stmt::function_test -- --nocapture`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core val::runtime_model::heap -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib` 和 `git diff --check`。

本轮收窄 stdlib `map` 只读 helper 的 whole-map clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮继续收窄 `map.values` 的 `Mixed` / `StringMixed` 输出构造，删除 `entries.values().cloned().collect()` 后再交给 `TypedList::from_runtime_values()` 的通用分类边界，改为 map-local 显式 shape builder，并新增 `StringMixed` 全字符串 values 返回 typed string list 覆盖。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 stdlib `list` 只读 helper 的 whole-list clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib list -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮删除 `TypedMap::get_into_heap()` / `get_str_into_heap()` no-op materialization wrapper，并收窄 core map/object access 的 whole-map/object clone 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core val::runtime_model -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli`。

本轮补齐 `Move` clone 分支 copy-policy metrics 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、snapshot grep、unsafe grep 和 `git diff --check`。

本轮补齐 `StoreCellVal` source movement fact 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::gc_cell_error -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、snapshot grep、unsafe grep 和 `git diff --check`。

本轮补齐 `SetGlobal` source movement fact 后已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::compiler32::facts_tests -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo test -p lk-core --lib`、`cargo test -p lk-stdlib --lib`、snapshot grep、unsafe grep 和 `git diff --check`。

本轮删除 LLVM artifact shell / host executable launcher fallback，并把 compiler const list/map lowering 改为直接从 AST slice 构造 typed const heap value 后，已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm -- --nocapture`、`cargo test -p lk-core vm::compiler32 -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮同步 `plan.md`，删除“恢复 LLVM shell / host executable launcher”目标，并移除 `RuntimeModuleState32` / `Exec32Result` / `Program32Result` 的 whole-state `Clone`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core stmt::function_test::tests::test_outer_returns_closure_value -- --nocapture`、`cargo test -p lk-core runtime32_callable_error_keeps_shared_module_state -- --nocapture`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮继续收窄 `RuntimeExport32` / `VmContext` shared-state clone 边界，删除隐式 `Clone` 并补显式 shared-clone 单测。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::runtime32 -- --nocapture`、`cargo test -p lk-core stmt::import -- --nocapture`、`cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`、`cargo test -p lk-stdlib --lib`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和单文件行数检查。

本轮修复 closure call 的 handler stack depth 恢复，防止 callee `try` 内提前 `return` 后 stale handler 捕获后续 raise。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core execute32_callee_return_unwinds_its_try_handlers_before_next_call -- --nocapture`、`cargo test -p lk-core execute32_ -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和单文件行数检查。

本轮补齐 LLVM backend 可静态显示 `ConcatString` 的 true native lowering，straight-line string concat 不再退出到 unsupported shape，也不恢复 artifact shell。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_const_string_concat_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend 静态 `ToString` native lowering，nil/literal bool/literal int/静态 string 可继续参与 straight-line string concat，动态 SSA 数值和复杂对象仍明确 unsupported。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_static_tostring_concat_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_const_string_concat_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮拆分 LLVM direct-call 测试模块并补齐 entry/direct-call 静态 `NewMap` true native lowering。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep 和 unsafe grep。

本轮补齐 LLVM backend 静态 `CallNamed`、静态 closure capture 和 named closure call true native lowering，并同步 CLI/README/website/LSP 中的 removed host launcher 表述。已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-cli --features llvm`、`cd website && bun run build`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮补齐 LLVM backend 静态 handler-local `Raise` true native lowering，entry/direct-call callee 内实际命中 handler 的 `Raise` 不再退出 unsupported shape。已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core --features llvm` 和新增 targeted LLVM raise 测试。

本轮收窄 `Call` / `CallNamed` callable dispatch 边界，删除整 `CallableValue` clone dispatch。已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-core`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture` 和 `cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`。

本轮继续收窄 stdlib `stream` 高阶回调 callable dispatch 边界，删除 stream 调用前整 `HeapValue::Callable` clone。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-stdlib stream -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy/AOT/callable clone grep 和 unsafe grep。

本轮继续收窄 stdlib `iter` 高阶回调 callable dispatch 边界，删除 iter 调用前整 `HeapValue::Callable` clone。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy/AOT/callable clone grep、unsafe grep、行数检查和 `git diff --check`。

本轮收窄 stdlib `map.set` / `map.delete` string-key helper，删除不可达 non-string key materialized mixed-map fallback。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization-helper grep、unsafe grep 和 `git diff --check`。

本轮收窄 stdlib global `spawn` callable 参数边界，`runtime_callable_arg` 返回原 `Arc<RuntimeCallable32>`，不再 shallow clone 整个 callable struct。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib concurrency_task -- --nocapture`、`cargo test -p lk-stdlib concurrency_chan -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/callable grep、unsafe grep 和 `git diff --check`。

本轮收窄 stdlib `list.push` / `list.concat` / `list.set` whole-container clone 边界，改为从借用的 `TypedList` 生成 build plan 后再 finalize。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、list clone grep、unsafe grep 和 `git diff --check`。

本轮收窄 stdlib `iter.chain` / `iter.flatten` whole-list clone 边界，改为从借用的 `TypedList` 生成 snapshot/plan，删除旧 owned nested-list helper。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、iter owned-helper grep、unsafe grep 和 `git diff --check`。

本轮收窄 exec32 dynamic list `+` whole-list clone/materialization 边界，list add operand 改为 snapshot，删除旧 by-value concat/push/prepend/runtime-values helper。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/list-helper grep、unsafe grep 和 `git diff --check`。

本轮收窄 exec32 dynamic list `-` whole-list clone/materialization 边界，list subtract/remove-first 也改为 snapshot，删除旧 owned list read helper。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 exec32 `SetIndex` string-list 污染路径，改为 `std::mem::take` 原 string backing 后 materialize mixed list，删除整 string list clone。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_materializes_typed_string_list_on_non_string_write -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/string-list-clone grep、unsafe grep 和 `git diff --check`。

本轮收窄 exec32 `SliceFrom` / `ToIter` heap object 读取边界，删除两个路径为绕开 borrow 产生的整 `HeapValue` clone。已重跑 `cargo fmt --all`、`cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 `VmContext` core field helpers 的 whole heap clone 边界，`__lk_make_struct` / `__lk_merge_fields` / `__lk_set_field` 都改为借用源 heap object 后只构造返回值所需 backing；`__lk_merge_fields` 的 overlay 合并也改为直接写入目标 typed backing，base 为 `Nil` 时直接保留 overlay typed map shape，不再先构造 context-local `typed_map_entries()` 或 runtime-entry `BTreeMap` 临时 map。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮收窄 core method `list.join()`、stdlib `iter.unique` 和 `iter.chunk` 的 whole-list clone 边界；三者都改为借用 source list 后只构造返回值所需 backing。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮收窄 stdlib `iter.zip` 的 whole-list clone 边界，zip 现在借用两侧 source list 生成 item snapshot，只为实际输出 pair 分配需要的 long string。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter_zip_materializes_only_used_long_string_items -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮删除 exec32 return clone fallback，`Opcode32::Return` 改为 `take_return_values()` 直接从 mutable stack slice 构造 `ReturnValues32`，常见 1-4 个返回值不再分配中间 `Vec`；同时删除 `ReturnValues32::from_slice()` / `from_vec()` 与 `PerformanceFacts.return_moves`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::return_values -- --nocapture`、`cargo test -p lk-core vm::compiler32::facts_tests::executor32_moves_returned_value_from_stack_window -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm` 和 return clone/fact grep。

本轮删除 `TypedList::from_runtime_slice()`、`TypedList::runtime_values_into_heap()`、`TypedMap::entries()` 和 exec32 `read_register_slice()`，`NewList` 的非 move-source 构造改为 `read_register_list()` 从当前 register window 直接识别 typed backing，`TypedMap` equality/display 改为按 variant 直接遍历，保留借用读取但不继续暴露 slice/materialization helper。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-stdlib runtime_registration_tests -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、slice/helper grep、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮补齐 entry/direct-call 静态 `NewRange` true native lowering。已重跑 `cargo fmt --all` 和 `cargo test -p lk-core llvm --features llvm -- --nocapture`。

本轮补齐 entry/direct-call 静态 `MapRest` true native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo fmt --all`。

本轮补齐 entry/direct-call 静态 `NewObject` 和 object `GetIndex` true native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo fmt --all`。

本轮补齐 entry/direct-call 静态 list/map/object `SetIndex` true native lowering，并修复静态 heap-like alias 更新。已重跑 `cargo fmt --all` 和 `cargo test -p lk-core llvm --features llvm -- --nocapture`。

本轮拆出 `core/src/llvm/tests/objects.rs` 并补齐静态 object identity equality / inequality native lowering。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo fmt --all`。

本轮继续拆分 LLVM 基础测试到 `core/src/llvm/tests/basic.rs`，主 `core/src/llvm/tests.rs` 降到 1370 行。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_simple_i64_return_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、legacy/AOT grep、unsafe grep 和 `git diff --check`。

本轮删除 stdlib `iter.collect` / `stream.from_list` 的 owned list arg helper，把 list 副本构造限制在返回值或 stream spec 持久化边界。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib stream -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮修复 LLVM static `IsList` string 语义，使其与 exec32 中 String 作为 iterable/list-like 的行为一致。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_static_string_is_list_like_vm_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 LLVM static `Not` 为 exec32 同语义的 Bool/Nil only；静态 string/list 和 direct-call string 参数的 `Not` 现在明确 unsupported，不再错误按 truthiness 生成 true native。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_rejects --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_source_not_and_is_nil_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 LLVM static `ToString` 为 exec32 同语义的 Nil/Bool/Int/Float/String only；静态 list/map/object 的 `ToString` 现在明确 unsupported，不再错误按 display string 生成 true native。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_rejects_static_list_tostring_to_match_exec32 --features llvm -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_tostring_rejects_list_operand -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮拆出 `core/src/llvm/tests/strings.rs`，并修复 LLVM static `Contains` / `GetIndex` / map equality / `SetIndex` 与 exec32 的 string/string-key map 语义差异：非 string needle 查 string haystack 会 native false，string-key map contains 会按 key-string 规则接受 int/bool/float/string needle，string-key map index/equality 会归一比较 ShortStr/heap string key，mixed map set/index 会保留 ShortStr 与 heap string key 的精确区别。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm::tests::strings --features llvm -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮删除公开 `TypedList::from_runtime_values()`，收窄 exec32 map `ToIter` pair 构造和 stdlib `iter` / `stream` / `task.join_all` 结果 list 的通用分类边界；当前 `TypedList::from_runtime_values` 源码 grep 无命中。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::container -- --nocapture`、`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo test -p lk-stdlib stream -- --nocapture`、`cargo test -p lk-stdlib concurrency_task -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮删除公开 `TypedMap::from_runtime_entries()`，core 内部改为 crate-private `typed_map_from_entries()`，stdlib/native export 已知 string-key map 直接构造 typed backing；当前 `TypedMap::from_runtime_entries` / `from_runtime_entries(` 源码 grep 无命中。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-core val::de -- --nocapture`、`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo test -p lk-stdlib runtime_native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮删除 `TypedList::runtime_values_no_heap()`，cross-variant equality 改为直接按 typed backing 逐项比较，不再为了比较先构造 runtime-value vector；新增短字符串/长字符串 cross-backing equality 覆盖。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core val::runtime_model -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/AOT grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 core method helper positional list materialization，删除 `runtime_positional_args()` 的整表 `Vec<RuntimeVal>` 边界，runtime callable 路径改传 list handle，builtin 分支只用 inline buffer 展开；新增 method helper 源码级覆盖。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::calls -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::native -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep 和单文件行数检查。

本轮收窄 `VmContext` trait registration list parsing，删除 `runtime_list_values()` 返回 `Vec<RuntimeVal>` 的整表展开 helper，`__lk_register_trait` / `__lk_register_trait_impl` 改为按长度和索引逐项读取 methods/entry 字段。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::context -- --nocapture`、`cargo test -p lk-core trait -- --nocapture`、`cargo test -p lk-core vm::compiler32::tests::compiler32_trait_method_dispatch_uses_runtime_callable -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 stdlib `iter.map` / `iter.filter` / `iter.reduce` / `iter.enumerate` 的 item snapshot 中转，删除 `into_item_snapshots()` 整表 `Vec<RuntimeListItemSnapshot>` 构造，改为 `RuntimeListSnapshot::for_each_item()` 逐项消费并回调。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 stdlib `list.push` / `list.set` 的污染分支，删除 `materialize_string_values()` 和 `set_materialized_list()` 通用 helper；typed/string backing 污染时直接构造目标 `Mixed` 输出并返回 old value。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮收窄 exec32 dynamic list prepend 的异类型 fallback，删除 `prepend_runtime_values()` 通用 helper并直接构造 `Mixed` 输出。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic::execute32_prepends_value_to_typed_string_list_without_helper_materialization -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮继续清理 mixed fallback helper surface，core exec32、stdlib `list` 和 stdlib `iter` 中旧 `append_runtime_values()` / `append_string_list_runtime_values()` 命名已无源码命中，保留的转换只在必须构造 `TypedList::Mixed` 输出时发生。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core vm::exec32::exec32_tests::basic -- --nocapture`、`cargo test -p lk-stdlib list -- --nocapture`、`cargo test -p lk-stdlib iter -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`。

本轮收窄 stdlib `map.values` 与 core serde decode 的 runtime-values helper 表面：`map.values` 改为渐进 typed-list builder，typed 输出不再预先克隆 mixed fallback；core decode-local helper 改名为 `decoded_values_to_typed_list()`。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-stdlib map -- --nocapture`、`cargo test -p lk-core val::de -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、legacy/materialization grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮扩 LLVM true-native straight-line 常量算术：entry 和 direct-call callee 中静态 int/float arithmetic 会折叠为 native static value，源码级 template string arithmetic 不再因 `ToString` 看到 SSA numeric register 退出 native lowering；相关测试断言同步改为验证最终 native 常量输出而不是固定中间 arithmetic IR。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_source_template_string_with_static_numeric_arithmetic_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm::tests::strings --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy/materialization grep、AOT/shell grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮继续扩 LLVM true-native static comparison folding：entry evaluator 现在和 direct-call callee 共用 `native_static_compare_bool()`，静态 int/float/string comparison 进入 template string 时可直接折叠为 bool constant，不再因 `ToString` 看到 SSA bool register 退出 native lowering。已重跑 `cargo fmt --all`、`cargo test -p lk-core llvm_backend_lowers_source_template_string_with_static_comparisons_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm::tests::strings --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、legacy/materialization grep、AOT/shell grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮修复 LLVM native division/modulo 与 exec32 的 divisor-zero 语义差异：static float folding 不再把除零折成可 `ToString` 的特殊值；scalar block lowering 对 int/float div/mod 发 divisor-zero guard，命中时 native executable 以非零 exit 结束；官网 runtime 文案也从旧 shell/launcher 改为 true-native LLVM。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_i64_instr32_arithmetic_ops_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_f64_instr32_arithmetic_ops_without_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_rejects_static_float_divisor_zero_tostring_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli` 和 `cd website && bun run build`。

本轮补齐源码级 static optional access 的 LLVM true-native 覆盖，并清理官网/LLVM docs 中旧 `LLVM IR shell output` 文案。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_source_static_object_optional_access_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_source_nil_optional_access_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli`、`cd website && bun run build`、legacy/AOT shell grep、unsafe grep、单文件行数检查和 `git diff --check`。

本轮补齐源码级 static nullish/logical short-circuit 的 LLVM true-native 覆盖；`??`、`&&`、`||` 的 `IsNil` / `Test` / `Jmp` / `Move` lowering 会直接生成 native branch IR，不再因为短路表达式退出到 unsupported shape。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm_backend_lowers_source_static_nullish_coalescing_without_shell --features llvm -- --nocapture` 和 `cargo test -p lk-core llvm_backend_lowers_source_static_logical_short_circuit_without_shell --features llvm -- --nocapture`。

本轮迁移 LSP/README 的 legacy `Val`/module exports 表面，并删除 LSP document symbol 的旧扁平 variable/import 兼容输出：`lk-lsp` completion、import export diagnostic、identifier context validation 和 stdlib definition lookup 改为读取 `RuntimeExport32` shared state 与 `RuntimeVal` heap map，源码中旧 `Module::exports()` / `Expr::Val` / `val::Val` / `extract_variables_from_pattern` 命中已清零。已重跑 `cargo fmt --all -- --check`、`cargo check -p lk-lsp`、`cargo test -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm` 和 legacy grep。

本轮删除 `RuntimeCallable32` 的隐式 `Clone` impl，显式改用 `shallow_clone_shared()` 表达共享 module/captures/state；GC runtime-callable shared-state 测试同步改为显式 shared clone。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core runtime_callable_shared_clone_keeps_module_captures_and_state_shared -- --nocapture`、`cargo test -p lk-core heap_store_gc_collects_runtime32_callable_shared_state_without_marking_dest_heap_captures -- --nocapture` 和 `cargo check -p lk-core`。

本轮补齐 executor 自动 GC 与语言级 `Raise` handler 的组合覆盖：handler catch 写入的 `ErrorVal` 会作为 active stack root 在下一条指令触发 GC 时存活，同时触发前的垃圾 slot 可被回收/复用。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core execute32_gc_keeps_caught_raise_error_value_alive -- --nocapture`、`cargo test -p lk-core vm::exec32::exec32_tests::gc_cell_error -- --nocapture` 和 `cargo check -p lk-core`。

本轮补齐源码级 static conditional / match expression 的 LLVM true-native 覆盖；`return 1 < 2 ? 42 : 7` 和基础 literal `match` 生成的 `Test` / `Jmp` / `Move` 控制流都可直接输出 native i64 return，不再退到 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_static_conditional_expression_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm_backend_lowers_source_static_match_expression_without_artifact_shell --features llvm -- --nocapture`、`cargo test -p lk-core llvm::tests::basic --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、legacy/materialization grep、单文件行数检查和 `git diff --check`。

本轮补齐源码级 range `for` loop 和 static-list indexed `for` loop 的 LLVM true-native 覆盖；`for i in 0..4` 会生成 native loop branch/add IR，`for value in [1, 2, 3, 4]` 可直接输出 native i64 return，不退回 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_for --features llvm -- --nocapture`。

本轮补齐源码级 range `for` loop 中 `break` / `continue` 的 LLVM true-native 覆盖；`for i in 0..7` 内的 continue/break lowering 会保留 native compare/branch/add IR，不退回 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_for_range_break_continue_without_artifact_shell --features llvm -- --nocapture`。

本轮补齐源码级 inclusive negative-step range `for` loop 的 LLVM true-native 覆盖；`for i in 5..=1..0 - 2` 会保留 native compare/branch/add IR，并按静态负步长继续 lowering，不退回 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_for_inclusive_negative_step_range_without_artifact_shell --features llvm -- --nocapture`。

本轮补齐源码级 static-string indexed `for` loop 和 static-map entry tuple-pattern `for` loop 的 LLVM true-native 覆盖；`for ch in "abc"` 和 `for (key, value) in {"a": 1, "b": 2}` 都通过 `ToIter` / `Len` / `GetIndex` lowering 直接输出 native i64 return，不退回 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_for_static_string_loop_without_artifact_shell --features llvm -- --nocapture` 和 `cargo test -p lk-core llvm_backend_lowers_source_for_static_map_entry_loop_without_artifact_shell --features llvm -- --nocapture`。

本轮补齐源码级 static `if let` list/map destructuring 的 LLVM true-native 覆盖；`if let [head, ..tail] = [40, 1, 2]` 和 `if let {"a": a, ..rest} = data` 生成的 `SliceFrom` / `MapRest` / `GetIndex` 路径可直接输出 native i64 return，不退回 artifact shell。已重跑 `cargo test -p lk-core llvm_backend_lowers_source_if_let_static --features llvm -- --nocapture`。

本轮补齐源码级 `if let` / `match` range、guard 和 or-pattern 的 LLVM true-native 覆盖；`if let 18..65` + guard + or-pattern 会保留 native comparison/branch/add IR，`match` range/guard/or-pattern 可直接输出 native i64 return，都不退回 artifact shell。已重跑 `cargo fmt --all -- --check`、`cargo test -p lk-core llvm::tests::basic --features llvm -- --nocapture`、`cargo test -p lk-core llvm --features llvm -- --nocapture`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp --features llvm`、`cargo check -p lk-core -p lk-stdlib -p lk-cli -p lk-lsp`、legacy/materialization grep、单文件行数检查和 `git diff --check`。

## 当前审计结果

当前 grep 期望：

```sh
rg -n "pub enum Val|Val::LongStr|Expr::Val|val::Val|record_val_clone|VAL_CLONES|IMMEDIATE_VAL_CLONES|HEAP_VAL_CLONES|values/(clone|intern)|LiteralLiteralVal|RuntimeLiteralVal|Stringing|Module::exports|\\.exports\\(" core/src stdlib/src cli/src lsp/src bench README.md README.zh-CN.md docs website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# no matches

rg -n '\bOp\b|Vec<Op>|pub enum Op|enum Op|ListFoldAdd|MapValuesFoldAdd|AddRangeCountImm|BC32|bc32|packed|quickening|fallback-reason|legacy|Legacy|LEGACY|crate::op|\bop::|mod op|core/src/op' core/src stdlib/src cli/src bench README.md docs website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# only intentional matches are OptLevel/Opcode32 and plan.md contract text.

rg -n 'Arc::new\(module\.clone\(\)\)|Arc::new\(\(\*module\)\.clone\(\)\)|shared_module\(\).*or_else|let module = runtime\s*\.module\(\)' stdlib/src core/src -g '*.rs'
# no matches

rg -n 'native .*disabled|disabled during the Instr32 artifact migration' core/src stdlib/src docs bench README.md website/src -g '*.rs' -g '*.md' -g '*.ts' -g '*.svelte'
# no matches

rg -n 'named_stack.*to_vec|CallNamed.*to_vec|\.to_vec\(\)' core/src/vm/exec32/named_call.rs core/src/vm/exec32/support.rs core/src/vm/runtime32.rs
# no matches

rg -n 'runtime_value_to_callable32\(' core/src stdlib/src -g '*.rs'
# no matches; only explicit _shared materialization is allowed.

rg -n 'runtime_value_to_callable32_snapshot|snapshot\(&self\)|\.snapshot\(' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'entries_into_heap|materialize_mixed|runtime_value_to_list_values|runtime_value_to_callable32_snapshot|snapshot\(&self\)|\.snapshot\(' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'get_into_heap|get_str_into_heap' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'runtime_values_into_heap' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'runtime_values_no_heap' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'TypedList::from_runtime_values|fn from_runtime_values|from_runtime_values\(' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'TypedMap::from_runtime_entries|from_runtime_entries\(' core/src stdlib/src cli/src -g '*.rs'
# no matches.

rg -n 'values\.to_vec\(\)|\[start\.\.end\]\.to_vec\(\)|get\(start\.\.\).*to_vec\(\)|clone_from_slice|read_register_range_owned|checked_u8_count|first_return_function|runtime_value_to_callable32_externalized|RuntimeCallable32::new\(|pub fn exports\(&self\)|\.exports\(\)' core/src stdlib/src cli/src -g '*.rs'
# only parser token slicing remains outside VM/runtime typed backing paths.

rg -n 'aot-minimal-runtime|lk-aot-runtime|lk_aot_runtime|aot-runtime' . -g '*.rs' -g '*.toml' -g '*.md' -g '!target/**'
# no matches.

rg -n 'compile_host_executable_launcher|host_executable_launcher_source|temp_host_launcher|latest_rlib|current_target_deps_dir|lk_rt_run_module32_json|@lk_module32_json|aot-runtime|aot-minimal-runtime|lk_rt_call|lk_rt_make_aot|lk_rt_register_native|runtime_value_to_callable32_externalized|RuntimeCallable32::new\(' core/src stdlib/src cli/src Cargo.toml core/Cargo.toml README.md docs bench/README.md -g '*.rs' -g '*.toml' -g '*.md'
# only negative test assertions/docs mention the removed shell/launcher symbols.

rg -n 'lk_rt_add|lk_rt_sub|lk_rt_mul|lk_rt_div|lk_rt_mod|lk_rt_intern_string|lk_rt_to_string|lk_rt_load_global|lk_rt_define_global|lk_rt_float|lk_rt_floor|lk_rt_starts_with|lk_rt_contains|lk_rt_cmp|HandleTable|decode_immediate|encode_immediate|NIL_VALUE|BOOL_TRUE_VALUE|BOOL_FALSE_VALUE|mod encoding|llvm::encoding' core/src/llvm -g '*.rs'
# no matches.

rg -n '\bunsafe\b|extern "C"|extern "Rust"|\*mut|\*const|transmute|MaybeUninit|NonNull|lk_rt_call|lk_rt_make_aot|lk_rt_register_native' core/src stdlib/src cli/src -g '*.rs' -g '!core/src/llvm/**' -g '!stdlib/src/llvm_bridge.rs'
# only `stdlib/src/os.rs` user-facing error strings mention unsafe.
```

旧二进制/旧 target 相关标准：

- CLI 不再为旧二进制输入或旧 output target 保留专门分支；这些输入只走普通源码/未知 target 错误。
- docs 和 LLVM module docs 不保留旧 runtime/bridge 的命名兼容面。
- 不允许恢复旧 binary execution、旧 writer 或旧 AOT callable bridge。

## 当前不能宣称完成的部分

- native executable output 已对 LLVM native-lowerable subset 生成 true native executable；unsupported runtime value/container/call/import/native runtime shape 现在直接报错，不再 fallback host artifact launcher 或 `lk_rt_run_module32_json` shell。继续扩 true native AOT 时仍必须基于 `Module32Artifact` / `RuntimeVal` / `HeapStore`，不能恢复旧 AOT callable bridge。
- 性能目标 `VM/Lua geomean <= 1.10x` 未达成；`bench/README.md` 记录的最新 quick comparison 仍明显 behind。
- `core/src/ast/parser.rs` 当前 1499 行，不能通过硬拆方式处理；后续只允许谨慎原位修改或先在其他文件降行数后再评估。
- `plan-progress.md` 已压缩为当前事实快照；后续新增进度前应优先替换旧小节，不再追加长流水账。

## 下一步执行顺序

1. 继续扩 LLVM true native lowering 覆盖面；不支持的形状保持报错，不恢复 launcher/shell。
2. 继续补齐 liveness/ownership movement 等 facts/cache 缺口。
3. 继续收窄跨 runtime/module 边界复制；导出、callable 和 import 路径继续优先 active/shared state，不恢复 heap/globals snapshot materialization。
4. 性能工作在架构迁移闭环后再做；不要新增 benchmark-specific opcode/fusion。

## 文件行数快照

最近检查：

```text
core/src/ast/parser.rs       1499
core/src/expr/expr_impl.rs    896
core/src/vm/analysis.rs       961
core/src/vm/exec32.rs         923
core/src/vm/exec32_tests/basic.rs 1033
core/src/vm/exec32/arithmetic.rs 961
core/src/vm/exec32/cell.rs    45
core/src/vm/exec32/call.rs    328
core/src/vm/exec32/named_call.rs 216
core/src/vm/exec32/container.rs 1260
core/src/vm/runtime32.rs      637
core/src/vm/exec32/imports.rs 227
core/src/vm/exec32/support.rs 280
core/src/vm/exec32/stack.rs   171
core/src/vm/exec32/runtime_callable.rs 1124
stdlib/src/iter.rs            1400
stdlib/src/stream.rs          976
core/src/vm/exec32_tests/native.rs 830
core/src/vm/context.rs        924
core/src/vm/context/core_methods.rs 658
core/src/llvm/runtime.rs      516
core/src/llvm/backend.rs     1355
core/src/llvm/callee_eval.rs  925
core/src/llvm/const_display.rs 67
core/src/llvm/ir_text.rs      91
core/src/llvm/scalar_emit.rs 177
core/src/llvm/scalar_facts.rs 208
core/src/llvm/straightline_value.rs 947
core/src/llvm/mod.rs          24
core/src/llvm/tests.rs       1332
core/src/llvm/tests/basic.rs 323
core/src/llvm/tests/direct_calls.rs 699
core/src/llvm/tests/objects.rs 205
core/src/llvm/tests/strings.rs 391
lsp/src/analyzer/completions.rs 226
lsp/src/analyzer/analysis_impl.rs 1200
lsp/src/analyzer/core_impl.rs 1362
lsp/src/server/analysis.rs 1079
lsp/src/analyzer/tests.rs 363
core/src/vm/compiler32.rs    1392
core/src/vm/compiler32/support.rs 327
core/src/stmt/function_test.rs 633
core/src/vm/compiler32/assign.rs 192
core/src/vm/compiler32/facts_tests.rs 623
core/src/vm/exec32_tests/gc_cell_error.rs 386
core/src/val/runtime_model.rs 687
core/src/val/de.rs            363
core/src/val/runtime_model/heap.rs 444
core/src/val/values/mod.rs     75
cli/src/main.rs               517
cli/tests/compile_cli_test.rs  979
docs/llvm/backend.md           55
stdlib/src/lib.rs            701
stdlib/src/list.rs            552
stdlib/src/map.rs             647
stdlib/src/runtime_native.rs  194
bench/README.md              169
core/src/vm/exec32_tests/container.rs 346
plan-progress.md             648
```

后续改动必须继续保持单文件不超过 1500 行；`core/src/ast/parser.rs` 仍是 1499 行，不能通过硬拆处理。
