  # LK 通用 VM 架构性能迁移计划

  ## Summary

  - 优先优化通用 VM，而不是 AOT/LLVM 或特定 benchmark pattern。
  - bench/* 只作为 correctness/performance gate，不作为源码特例来源。
  - 参考实现结论：
      - Luau：寄存器 VM、typed table/list ops、FASTCALL + fallback 结构、call frame window 值得借鉴。
      - CPython：adaptive counters、quickening、指令旁 cache metadata、typed op deopt 值得借鉴。
      - QuickJS：immediate/refcounted object 分界、atom/key identity 值得借鉴；不引入它的 C 对象系统。
      - Ruby/V8/WebKit：call-site feedback、shape/slot cache 分层思想可借鉴；JIT/GC/object-shape 体系不作为当前 VM 依赖。
      - Rune/Rhai：Rust 安全边界、native function ABI、stack reuse 可借鉴；避免 Rhai 那类通用 Dynamic 膨胀热路径。

  ## Architecture

  - 分层执行模型
      - Generic Op：完整语义 fallback，负责动态行为、错误、named/default 参数、复杂 stdlib。
      - Typed Op：通用类型化指令，不绑定 workload 名称或源码常量组合。
      - Quickened Op：运行时根据稳定类型把 generic site 替换/旁路成 typed site，可 deopt。
      - Packed Op：BC32/后续 BC64 的主执行路径，typed op 必须有 packed 覆盖。
  - Quickening 规则
      - 每个函数维护 QuickeningState：执行计数、site 类型观测、失败/backoff 计数、原始 opcode。
      - warmup 后 specialization：Add -> AddInt/AddFloat/字符串拼接融合，Index -> list[int]/string[int] quickening，Call -> CallIc/native fastcall；`CallExact` / `CallClosureExact` / `CallNativeFast` / `CallNamedFallback` 已有独立 typed opcode，可按类型语义命中并在失败时回到 generic/fallback 路径。
      - 失败时恢复 generic 或进入 backoff，不缓存具体 workload 结构。
      - cache 只缓存 type/shape/key/slot/callee identity，不缓存可能因 mutation 过期的 Val 结果。
  - Value 和寄存器协议
      - Val 保持 immediate-first：Nil/Bool/Int/Float/ShortStr cheap clone，heap object 明确走 shared/owned。
      - 新增寄存器访问语义：read_reg、borrow_reg、move_reg、write_reg、take_or_clone。
      - VM 热路径默认单线程所有权；跨线程/API 边界才使用 Arc。
      - 非 LLVM VM/core 不新增 unsafe；现有 raw pointer 边界在拆分时收窄并加 debug assertion。

  ## Key Implementation Changes

  - 先拆大文件
      - opcode.rs 拆成 arithmetic/control/call/container/string/global。
      - packed.rs 拆成 decode/dispatch/hot/cold typed families。
      - values/mod.rs 拆成 layout/strings/containers/functions/conversions。
      - 单文件 <1500 行，>1000 行必须继续拆。
  - Typed opcode 家族
      - Numeric/control：已有 `AddInt` / `AddFloat` / `AddIntImm` / `SubInt` / `SubFloat` / `MulInt` / `MulFloat` / `DivFloat` / `ModInt` / `ModFloat`、int compare quickening、compare+branch fusion、range loop/step fusion、独立 `BoolBranch` / `RangeLoopI` / `CmpI` opcode；`DivInt` 因 LK 除法语义暂不建单一 int op。
      - Call：已做 `Call` + `CallIc` / frame cache / native fastcall / closure fast-span 路径，并新增独立 `CallExact` / `CallClosureExact` / `CallNativeFast` / `CallNamedFallback` typed opcode 家族。
      - List：已有 `ListLen`、`ListIndexI`、`ListSetI`、`IndexK`、通用 `Index` 的 list[int] quickening、`ListPush` unique/CoW typed op；不再单独保留 `ListPushUnique` 命名，唯一/共享由运行时所有权路径处理。
      - Map：已有 `AccessK`、`MapHasK`、`MapHas`、`MapSet`、`MapSetMove`、`MapGetInterned`、`MapSetInterned`、`MapGetDynamic` 和 interned/cached-hash key 语义；`MapHasK` 即 const interned-key contains op，不再额外拆 `MapHasInterned` 别名。
      - String：已有 `StrLen`、`StrIndexI`、`StrConcatKnownCap`、`StartsWithK`、`ContainsK`、通用 `Index` 的 string[int] quickening、`ToStr + Add` 拼接融合和 known-cap concat helper。
      - Stdlib intrinsic 只按公开函数语义建模，不按 workload 写规则；当前已覆盖 `len` / `floor` / `map.get` / `map.set` / `map.has` / `contains` / `starts_with` 等主要 lowering，`push` 已有 VM `ListPush` typed op，`join` 仍按 stdlib/native fastcall 语义处理。
  - Call/frame 重构
      - 位置参数调用传 register window，不创建 Vec<Val>。
      - callee 直接从 caller window 复制或 move 到参数寄存器；返回值直接写 caller return slot。
      - call-site IC 缓存 callee pointer/function id、arity、param/named layout、frame cache；return layout 已进入 `CallIc` entries 并在 hit 时校验。
      - native fastcall ABI 使用 ArgWindow/return slot；旧 RustFunction(args: &[Val]) 已收窄为 legacy callable/fallback adapter，公开 stdlib 导出默认走 fast ABI。
  - Container/string runtime
      - VM list/map unique 时原地 mutation，共享时 clone-on-write；stdlib ListMutation guard 已使用 unique 原地 fast path。
      - map key 引入 interned identity；const key 与 dynamic key 是同一语义下的不同 fast path；长字符串/key cached-hash 路径已补齐。
      - string concat/template 使用 known capacity；短字符串保持 immediate；长字符串拼接后会预热 key/hash cache，减少重复 hash 与重复构造。
      - 不合并 LK List/Map 为 Lua table，不引入 GC。
  - BC32/packed 门禁
      - 新 typed op 必须同步：bytecode enum、encoder、decoder、packed cold exec、必要的 packed hot cache、roundtrip/packed execution test。
      - 新增 coverage 报告：函数是否 packed、哪些 op fallback、quickening 命中/失败、call-site 类型分布。
      - 当前 `Op::bc32_typed_gate_name` 使用 exhaustive match，可强制新增 opcode 做分类；typed gate 样本测试会要求 `Some(...)` op 完成 BC32 encoder/decoder roundtrip 与 packed execution 覆盖。
      - BC32 控制流 offset 在 pack 阶段统一 remap 到 word PC；decoded 表只做 op cache，不再二次改写 offset，`Break` / `Continue` 也纳入该坐标规则。

  ## Implementation Order

  1. Reference-backed audit + coverage
      - [x] 加 VM coverage 输出，不改语义。
      - [x] 输出 opcode 分布、packed 覆盖、fallback 原因、call-site 类型、clone/move 统计。
      - [x] runtime metrics 输出 quickening hit/build/miss/deopt/sentinel 统计。
  2. 文件拆分
      - [x] 拆分 `opcode.rs` 的 call/closure/control 辅助模块，主文件保持在 1500 行以内。
      - [x] 拆分 `values/mod.rs`、`lkb.rs`、`compiler/stmt.rs` 的超限模块。
      - [x] 拆出 `opcode/arithmetic_ops.rs`；`opcode.rs` 已降到 1500 行硬上限以内，但仍处于 1000 行警告区间。
      - [x] 拆出 `opcode/compare_ops.rs`；比较族现在有独立 quickening 入口。
      - [x] 拆出 `opcode/container_ops.rs` 和 `opcode/global_ops.rs`；`opcode.rs` 已低于 1500 行硬上限。
      - [x] 拆出 `opcode/string_ops.rs`，承载 `ToStr`/字符串 intrinsic fast path，避免继续在 `opcode.rs` 警告区堆逻辑。
      - [x] 拆出 `opcode/pattern_ops.rs`；`opcode.rs` 已降到 1000 行警告线以下。
      - [x] 拆出 `packed/stats.rs`、`packed/closure.rs`，并收窄 packed fallback `LoadCapture` 到 closure 辅助模块；`packed.rs` 已降到 1000 行警告线以下。
      - [x] 拆出 `quickening/tests.rs`；`quickening.rs` 新增 float/string quickening 后仍保持在 1000 行警告线以下。
      - [x] 拆出 `bc32/metrics.rs`、`bc32/function_decode.rs`、`bc32/encode_support.rs`；`bc32.rs` 已降到 1000 行警告线以下，后续 typed op 改动可在较小模块里完成。
      - [x] 拆出 `vm/caches/packed.rs`；`caches.rs` 已降到 1000 行警告线以下，packed hot cache 类型与 call/tiny cache 逻辑分离。
      - [x] 拆出 `expr/pattern_impl.rs`、`ast/parser/support.rs`、`lsp/server/handlers/initialization.rs`；当前 `core/cli/lsp/stdlib` Rust 源码没有文件超过 1500 行。
      - [x] 拆出 `compiler/expr_call.rs`、`compiler/expr_list.rs`、`compiler/expr_map.rs`，方法调用/list/map 构造 lowering 已从 `expr.rs` 分离；`expr.rs` 仍处于 1000 行警告区但低于 1500 行硬上限，后续继续拆表达式子族。
      - [x] 拆出 `compiler/stmt/assign.rs`，赋值 lowering 从 `stmt.rs` 分离；`stmt.rs` 已降到 1000 行警告线以下。
      - [x] 新增 fast native 类型后拆出 `val/values/native.rs`，保持 `values/mod.rs` 低于 1500 行硬上限。
      - [x] 拆出 `val/values/strings.rs`，承载字符串构造、intern、拼接和 to-string 热路径；`values/mod.rs` 从硬上限边缘降回 1000 行警告区。
      - [x] 拆分 `container_ops.rs` 内部的 list/map/string/scalar 子族，后续 typed fast path 可在独立小模块里扩展。
  3. typed opcode + BC32 扩展框架
      - [x] BC32 typed-op roundtrip gate 覆盖当前 typed op。
      - [x] `MapHasK` 已同步 enum/encoder/decoder/packed cold execution/test。
      - [x] `Op::bc32_typed_gate_name` 使用 exhaustive match 强制新增 opcode 做 typed-gate 分类，BC32 roundtrip gate 与 coverage 输出共享同一 typed gate 名单。
      - [x] 将 typed gate 测试从“当前样本列表”升级为枚举式门禁：所有 `bc32_typed_gate_name() == Some(...)` 的 op 必须有 encoder/decoder roundtrip、packed cold execution；需要 hot cache 的 op 还必须有 packed hot hit test。
      - [x] 移除或收紧 `encode_support::opcode_name` 的 wildcard fallback，避免新增 opcode 在覆盖/诊断里显示为 `Unknown` 却不失败。
  4. numeric/control quickening
      - [x] 通用 `Op::Add` 已有 per-function site quickening：int+int warmup、hit、deopt、backoff、generic fallback。
      - [x] quickened Add 不绑定 benchmark 源码结构，只观察 RK operand runtime type。
      - [x] 通用 `Op::Add` 已扩展到 `str + primitive` / `primitive + str` 拼接 quickening，只缓存类型形态，不缓存拼接结果。
      - [x] 扩展到 Sub/Mul/Mod 的通用 int quickening；Div 因 LK 当前语义会在 int/int 下按整除性返回 Int 或 Float，暂不使用单一 Int quickening。
      - [x] 扩展 Add/Sub/Mul/Mod 的通用 float numeric quickening，支持 float+float、float+int、int+float 形态并在 int/int 形态变化时 deopt。
      - [x] 扩展 `== != < <= > >=` 的通用 int compare quickening。
      - [x] packed hot cache 已融合动态 `CmpEq/Ne/Lt/Le/Gt/Ge + JmpFalse`，跳过临时 Bool 写入和第二次 dispatch。
      - [x] opcode 解释器已融合动态 `CmpEq/Ne/Lt/Le/Gt/Ge + JmpFalse`，不新增 bytecode 格式即可跳过临时 Bool 写入。
      - [x] opcode 解释器已融合相邻 `ForRangeLoop + ForRangeStep`，与 packed hot range-step fusion 对齐，跳过空/尾部 range loop 的 step dispatch。
      - [x] 将 range tail fusion 下沉到通用 `ForRangeStep` guard 推进：非空 body 也可在尾部直接进入下一轮 body/exit，并删除 packed 中 `AddModulo` / `Tiny*` 局部 body 特例。
      - [x] `CmpI` / `BoolBranch` / `RangeLoopI` 已作为独立 opcode 落地，并同步 BC32/LKB/packed/LLVM/compiler peephole。
  5. call/frame protocol
      - [x] 引入 `ArgWindow` 与 `ReturnSlot` 内部协议；opcode 与 packed 的 native RustFunction/RustFunctionNamed 调用路径已通过 adapter 使用参数窗口和返回槽。
      - [x] 旧 `RustFunction(args: &[Val])` / `RustFunctionNamed(positional, named)` 只作为 fallback adapter；core trait builtins、stdlib method registry、`math.clamp` 与 `string.replace` named native 导出已迁到 fast wrapper / `RustFastFunctionNamed`。
      - [x] 引入 `Val::RustFastFunction` / `NativeArgs`，并迁移 `list.len` 作为首个真实 native fastcall intrinsic。
      - [x] 将 `string.len` / `string.starts_with` / `string.contains` 和 `map.len` / `map.has` / `map.get` 迁移到真正 native fastcall function pointer；旧 method registry 仍走 legacy adapter。
      - [x] stdlib 高阶调用与 mutation 回调路径已识别 `RustFastFunction`，fast intrinsic 可作为普通 callable 传递。
      - [x] method registry 已从 `RustFunction` 指针升级为 callable `Val` cache，并支持 `register_fast_method`；`list/string/map` 的 fast intrinsic method sugar 可直接走 fast ABI。
      - [x] 继续迁移 `string.ends_with` / `string.is_empty` 到 native fastcall，并补充测试防止回退到 legacy `RustFunction`。
      - [x] 迁移 `string` 除 named `replace` 外的 positional 公开函数和方法 sugar 到 native fastcall，减少 slice adapter 中转。
      - [x] 迁移 `list` 公开 positional 函数和方法 sugar 到 native fastcall；mutation guard 内部方法继续保留 legacy adapter。
      - [x] 迁移 `map` 公开 positional 函数和方法 sugar 到 native fastcall；mutation guard 内部方法继续保留 legacy adapter。
      - [x] 迁移 `math` 的纯 positional 公开函数到 native fastcall；`clamp` 因 named 参数语义继续保留 legacy named adapter。
      - [x] 迁移 `datetime` 的纯 positional 公开函数到 native fastcall，继续收窄 legacy native adapter 使用面。
      - [x] 迁移 `stream` 模块导出函数到 native fastcall wrapper；内部 cursor/stream 复用逻辑和 method sugar 暂保留 slice adapter。
      - [x] 迁移 `os` 模块导出函数与 `env` / `dir` 对象方法到 native fastcall，保留原系统调用和参数语义。
      - [x] 迁移 `tcp` 模块导出函数到 native fastcall，完成 `stdlib/src/*` 中 `Self::` 注册函数的 fast ABI 覆盖。
      - [x] 迁移 `iter` 模块导出函数到 native fastcall wrapper，保留内部 list/iterator 复用实现和 Iterator method sugar。
      - [x] 迁移 `json` / `toml` / `yaml` 解析模块导出函数到 native fastcall。
      - [x] 迁移 `time` / `chan` / `task` 并发相关模块导出函数到 native fastcall。
      - [x] 迁移 `io` 模块导出函数与 stdin/stdout/stderr 对象方法到 native fastcall。
      - [x] 迁移全局 builtin（`print` / `println` / `panic` / concurrency lowering helpers）到 native fastcall；`stdlib/src` 剩余 `RustFunction` 只作为 legacy callable 类型识别或 named fallback。
      - [x] 继续迁移更多公开语义 stdlib intrinsic，减少 legacy adapter 返回值中转。
      - [x] `ReturnSlot` 已移除 `*mut FrameState`，native fastcall 返回写槽只保留 base/retc，减少返回协议暴露的裸 frame 边界。
      - [x] 继续收窄 closure exact call 的 frame/return-slot 协议：`CallFrameMeta::inline_return` 统一返回槽 metadata，`invoke_vm_closure_fast` 集中 `self_ptr` 解引用与 fast-span 调用，opcode/packed 不再直接调用 VM raw fast span。
      - [x] 将 call-site return layout 明确纳入 IC/元数据验证；`CallIc` 的 Rust/RustFast/RustFastNamed/RustNamed/closure entries 都携带 return layout 并在 hit 时校验。
      - [x] 完整独立 typed call 家族已落地：`CallExact`、`CallClosureExact`、`CallNativeFast`、`CallNamedFallback` 均同步 bytecode、BC32 ext、LKB、opcode/packed execution、typed gate packed execution test；位置参数 typed call 在 LLVM fallback 中继续发射通用 call helper，named fallback 仍显式保留为 VM fallback opcode。
      - [x] compiler 对已知位置参数 native callable 发射 `CallNativeFast`；native callable 调度先解析为 Copy descriptor，再进入 IC，避免 `CallNativeFast` / generic native branch 为函数指针 callee clone `Val`。
      - [x] packed hot slot 已覆盖 `CallNativeFast` ExtOp，热 native call 不再每次走 ExtOp miss + decode。
      - [x] packed hot slot 已覆盖 generic `Call` / `CallX`，只缓存解码结果与 next PC，执行仍委托统一 `run_call_packed`，避免复制 closure/native/named/default 语义。
      - [x] 方法调用 fallback lowering 已改为预留连续 call-window 后直接填充 obj/method/pos-list/named-map 参数槽，避免先构造临时寄存器再 `Move` 到参数窗口；`bench/workloads_business_algorithms.lk` bytecode `Move` 从 160 降到 65，runtime coverage 保持 checksum 语义并减少约 1.1 万次 register writes。
      - [x] 继续移除或隔离非 LLVM VM/core 新增 raw pointer/unsafe 边界；`runtime/frame/run` 的 `self_ptr` / `frame_raw` / region allocator / cached function pointer 解引用已集中到 `raw_boundary.rs`，opcode/packed/call path 不再散落直接解引用。
  6. value/container/string hot path
      - [x] 通用 `Index` 已支持 list[int] / string[int] quickening，只缓存 site 类型，不缓存元素值。
      - [x] `FrameState` 已建立 `borrow_reg` / `write_reg` / `take_or_clone_reg` 基础协议，并补充单测；后续热点 opcode 可逐步迁移。
      - [x] 增加 `FrameState::move_reg`，并将 return-value 搬运收敛到统一 `move_reg_value` / `move_reg_to_reg` helper，避免在 return path 散落 `mem::replace`。
      - [x] 非调用 RHS 的赋值已直接写入目标局部寄存器，避免“临时寄存器 + StoreLocal clone”路径；compound assignment fallback 也改为把结果直接写回目标槽，仅在 RHS 可能调用时保守复制旧值；`bench/workloads_business_algorithms.lk` packed entry `StoreLocal` 从 99 降到 62，entry ops 从 1022 降到 987。
      - [x] 函数声明与普通 `let` call RHS 已改为直接绑定最终寄存器/返回槽，避免 `MakeClosure/Call result -> StoreLocal` 二次拷贝；位置参数 call window 现在按 `max(argc, retc)` 预留，`argc=0, retc=1` 也有稳定返回槽并通过 global IC 回归测试；`bench/workloads_business_algorithms.lk` packed entry `StoreLocal` 从 62 降到 31，entry ops 从 987 降到 954，runtime `register_writes` 从 45210596 降到 41742177，`heap_clones` 从 1107579 降到 944172。
      - [x] peephole 已消除 `LoadLocal -> Ret/JmpFalse/BoolBranch` 的单消费者临时寄存器链，返回和分支可直接读局部槽；`bench/workloads_business_algorithms.lk` total ops 从 1103 降到 1100，`LoadLocal` 从 69 降到 66，runtime `register_writes` 从 41742177 降到 41457177，`val_clones` 从 14241983 降到 13956983。
      - [x] opcode 解释器已融合通用 `ToStr + Add` 右侧拼接模式，和 packed hot `ToStrAddRhs` 对齐，跳过临时字符串寄存器写入。
      - [x] string concat 已使用 known-cap `String` 并直接进入 `from_string` / `intern_owned`，保留 ShortStr immediate，减少长字符串拼接后的重复构造。
      - [x] 为长字符串拼接/Map key 补齐避免重复 hash 的缓存策略；Map lookup/contains/insert/remove 与 VM/stdlib/LLVM mutation/build path 已切到 `hashbrown` raw lookup，长 `ArcStr` lookup/remove 使用 TLS hash cache，insert 使用 fresh hash 并回填缓存；长字符串 concat 会为生成的 `ArcStr` 预热同一 TLS hash cache，`intern_owned` 对 >64 字节字符串仍不全局 intern，Map 存储仍是 ArcStr key。
      - [x] Map literal key 与 MapSet string key 已收敛到 `Val::primitive_key_arcstr` / `Val::string_key_arcstr`，opcode 与 packed hot/cold 共享同一 interned key 语义。
      - [x] VM `ListPush` / `MapSet` 已通过 `Arc::make_mut` 统一 unique 原地修改、共享时 clone-on-write。
      - [x] stdlib `ListMutation` guard 改为 unique 时原地修改、共享时 clone-on-write；当前 list guard 首次 mutation 会复制到 scratch。
      - [x] `ListLen` / `ListIndexI` / `ListSetI`、`MapGetInterned` / `MapSetInterned` / `MapGetDynamic`、`StrLen` / `StrConcatKnownCap` / `StrIndexI` 独立 opcode 已同步 bytecode、BC32 ext、LKB 编解码、opcode/packed cold execution 和 typed gate；compiler 已在已知 list 局部 + i16 常量索引时发射 `ListIndexI`，在 `list.set` / `data.set` 常量 i16 index 时发射保持 `[updated_list, old_value]` 语义的 `ListSetI`，在已知 map 局部 + 常量字符串 key 时发射 `MapGetInterned` / `MapSetInterned`，在已知 map 局部 + 动态 key 时发射 `MapGetDynamic`，并在 template literal 的已知字符串片段拼接中发射 `StrConcatKnownCap`；主 `opcode.rs` 已先拆出 build/slice/mutation 容器 helper 降到 1000 行警戒线以下，`lkb.rs` 已拆出 opcode codec 降到 1000 行以下，BC32 已补 3-word ext 形态覆盖 `ListSetI` 四操作数字段。
      - [x] packed hot slot 已覆盖 `Access` / `AccessK`、`ListLen` / `MapLen` / `StrLen`、`MapGetInterned` / `MapGetDynamic`、`StrConcatKnownCap`、`BuildList` / `BuildMap`，减少 typed ExtOp 与通用访问/构造 op 的 repeated decode/miss。
      - [x] packed hot slot 继续覆盖 `AddInt` / `AddFloat` / `SubInt` / `SubFloat` / `MulInt` / `MulFloat` / `DivFloat` / `ModInt` / `ModFloat`、`Floor`、`StartsWithK`、通用 `Len` / `Index`、`MapSetInterned`；typed arithmetic 热路径直接读寄存器，类型偏离时才回 fallback。
      - [x] packed hot slot 已覆盖剩余 `ToIter` ExtOp；`coverage --runtime` 中 `quickening_sentinel_skips` 已归零。
      - [x] 非测试 release 构建的 runtime metrics 改为真正 no-op；普通 release VM 执行不再为 clone/write/quickening 统计支付全局 atomic enabled-check 成本，debug/test 的 `coverage --runtime` 仍通过 `vm_runtime_metrics_reset()` 开启统计。
  7. AOT 对齐
      - [x] LLVM runtime native call bridge 已支持 `RustFastFunction`，AOT 路径可消费同一 native fastcall ABI。
      - [x] LLVM 只消费 typed VM IR/opcode；删除 const-map/string-int membership compare 的局部特例，`MapHasK` 改为消费 typed opcode 并走通用 `lk_rt_map_has` helper。
      - [x] LLVM fallback 已覆盖 VM 新增 typed ops：`CallExact` / `CallClosureExact` / `CallNativeFast` 共享 call helper，`StrConcatKnownCap` / `MapGetInterned` / `MapGetDynamic` / `MapSetInterned` / `ListLen` / `MapLen` / `StrLen` 均可 lower；`bench/workloads_business_algorithms.lk` coverage 显示 AOT entry 为 native-lowerable。
  8. 当前验收记录
      - [x] 2026-05-19 短验 `RUNS=3 EXTRA_RUNS=3 bench/run_workload_bench.sh`：checksum 全匹配，VM geomean 3.149x vs Lua，AOT geomean 0.583x vs Lua，AOT backend O2。
      - [x] packed hot slot 扩展后，coverage runtime metrics 中 `quickening_sentinel_skips` 从约 1006 万降至 0，`quickening_misses` 从 269 降至 15。
      - [x] compiler method-call 拆分与 call-window lowering 后，`cargo test -p lk-core compiler --lib` 通过；`coverage --runtime bench/workloads_business_algorithms.lk` 显示 `quickening_sentinel_skips=0`、`quickening_misses=15`、`register_writes=46852156`、`heap_clones=1112052`。
      - [x] 直接赋值 + compound fallback lowering 后，`coverage --runtime bench/workloads_business_algorithms.lk` 显示 `register_writes=45210596`、`val_clones=17710426`、`heap_clones=1107579`，checksum 全匹配。
      - [x] let-call/函数声明直接目标槽 lowering 后，`coverage --runtime bench/workloads_business_algorithms.lk` 显示 entry ops=954、entry `StoreLocal`=31、`register_writes=41742177`、`val_clones=14241983`、`heap_clones=944172`，checksum 全匹配；`RUNS=3 EXTRA_RUNS=3 bench/run_workload_bench.sh` checksum 全匹配，VM geomean 3.175x vs Lua，AOT geomean 0.585x vs Lua，仍有多项 low-confidence 噪声。
      - [x] local-copy peephole 后，`coverage --runtime bench/workloads_business_algorithms.lk` 显示 total ops=1100、entry ops=954、total `LoadLocal`=66、`register_writes=41457177`、`val_clones=13956983`、checksum 全匹配。
      - [x] 后续两次短验均出现全 workload low-confidence 高噪声（VM-only 样本 geomean 3.436x，full AOT 样本 geomean 4.597x），不作为性能回归/收益结论；继续以低噪声样本与 coverage 指标作为当前记录。
      - [ ] VM geomean 尚未达到“通用 VM 大幅提升”验收；下一步应优先继续压缩剩余 generic dispatch / clone / register-write 热点，而不是增加 benchmark-specific lowering。

  ## Test Plan

  - Correctness：
      - cargo test -p lk-core
      - cargo test --workspace
      - cargo check --all-features --workspace --all-targets
  - Coverage：
      - 新增测试保证每个 typed op 有 BC32 roundtrip 和 packed execution。
      - 新增测试保证 quickened op 可 deopt 到 generic op。
  - Performance：
      - 短验：RUNS=3 EXTRA_RUNS=3 bench/run_workload_bench.sh
      - 阶段验收：RUNS=10 EXTRA_RUNS=20 bench/run_workload_bench.sh
      - 必须 checksum 全匹配；VM geomean 应跨类别下降，而不是单一 workload 变快。

  ## Assumptions

  - 不兼容旧 bytecode/runtime ABI；只保留 LK 源码语法和可观察行为。
  - benchmark 是验收，不是优化规则来源。
  - 优先级：通用 VM 主路径 > typed/packed 覆盖 > call/value/container 架构 > AOT 对齐。
