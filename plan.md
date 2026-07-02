# 正确性优先计划(llvm lower / VM)

> **状态:全部完成(2026-07-02)**,含后续"先补再删"轮:MIR 补齐
> println/print/assert/长字符串/TestEqIntI2 后,legacy text 后端整体退役
> (-4.8 万行,workspace 1460 全绿)。成果见 `handoff.md`,细节见 `progress.md`。
> 计划外发现:`.lkm` facts 不序列化导致 for 循环运行时错误与 while 死循环
> (已修,artifact v4);Miri 抓出 lkrt 测试代码一处 Stacked Borrows UB(已修);
> native abort 丢弃 C stdio 缓冲 stdout(已修,`lkrt_abort` flush)。
> 「非本轮」中的 clang `-O2` 阻碍已清(现在只有 MIR 一个后端)。

目标:不改变架构骨架(bytecode 单一语义锚 + VM/AOT 双独立实现 + 差分验证),
把该架构的正确性保障从"手写用例"升级为"系统性防线"。性能工作(-O2、tiering、
facts 上移)全部排除在本轮之外。

原则:
- 两条执行路径保持**独立推导**(冗余即校验),不引入共享可信输入。
- 失败方向永远是"响亮失败",不允许静默降级到弱测试面。
- 每步完成即跑相关测试;全部完成后跑 `cargo test --workspace --all-features`。

## 步骤

### 1. 生产路径运行 MIR validate()(小)
`llvm/src/llvm/backend.rs`:`lower()` 成功后、`render_module()` 之前无条件调
`lk_aot_mir::validate()`,失败即 bail(内部错误,带 reason)。
验证:`cargo test -p lk-llvm` + CLI AOT 测试。

### 2. bytecode verifier:.lkm 加载期验证(中大)
`.lkm` 是外部输入,但 `artifact.rs::validate` 只查 format/version/entry;
执行器 `stack_index_unchecked` 的 debug_assert 在 release 下消失,损坏 artifact
可静默跨帧读写(内存安全但结果错)。
- 在 `core/src/vm/ir.rs` 为每个 Opcode 增加操作数分类元数据(register / immediate /
  jump offset / 索引),与 opcode 定义同处,作为单一真相。
- 在 artifact 加载(`FunctionData::into_function` / `ModuleData::into_module`)
  逐指令验证:寄存器 < register_count、跳转目标在函数内、函数/全局索引在界内。
- 编译器产物在测试中同样过一遍 verifier(保证 verifier 不误杀合法字节码)。
验证:`cargo test -p lk-core`,加恶意 artifact 拒绝用例。

### 3. legacy fallback 改 opt-in(中)
MIR 拒绝时静默落到 legacy(弱测试面、已知语义分歧)。改为:
- `LlvmBackendOptions::allow_legacy_fallback`(默认 false)+ 环境变量
  `LK_AOT_LEGACY=1` 可开。
- 默认路径:MIR 拒绝 → 直接报错(同时带 MIR Unsupported reason 与 legacy 提示)。
- 现有依赖 fallback 的测试改为显式 opt-in;34 个 pinned legacy 测试不受影响
  (它们已用 `legacy_text_backend_options()`)。
验证:`cargo test -p lk-llvm -p lk-cli`。

### 4. GC stress 模式(小)
`LK_GC_STRESS=1` 时每次 `alloc_heap_value` 立即 collect(而非 pending 标志)。
用于暴露 root 枚举遗漏。
验证:stress 开启下跑 `cargo test -p lk-core`(允许慢)。

### 5. 差分语料扩展:examples/ 纳入差分(中)
新测试:遍历 `examples/{syntax,stdlib,general}` 全部 .lk,能被 MIR lower 的
就 VM vs native 比对 stdout/退出语义;不能 lower 的记录 Unsupported reason
(作为覆盖面快照,不算失败)。
验证:新测试通过;统计当前可 lower 比例写入测试输出。

### 6. 生成式差分 fuzz(大)
新增随机良类型程序生成器(限定 MIR 可下降子集:i64/f64/bool/str 标量、
if/while/for、直接调用、List/Map、模板串):
- 种子化(CI 确定性),默认 N 个用例;`LK_FUZZ_CASES` 环境变量放大。
- 每例:VM 运行 vs MIR native 运行,stdout + 成功/失败逐项比对。
- 生成器另定向 fuzz `lower()` totality:任意程序只允许 Ok/Unsupported,panic 即 bug。
验证:默认规模在 CI 时间预算内全绿;本地放大规模跑一轮。

### 7. native 侧 sanitizer 接入(小中)
`llvm/src/native_executable.rs` 支持 `LK_NATIVE_SANITIZE=address,undefined`
透传 `-fsanitize=`;差分测试(含 fuzz)在 sanitizer 下本地跑一轮。
lkrt 单测跑 Miri 可行性评估(FFI 边界内的纯逻辑部分)。

### 8. 语义 golden vectors(中)
`docs/semantics.md`:逐特性写下已覆盖子集的裁决语义(div/0、缺失键算术、
nil 打印、float 显示、退出语义 VM exit-1 vs native abort-134),配 .lk 片段 +
期望输出,作为 VM/native 分歧时的第三仲裁。

### 9. 收尾验证
- `cargo fmt` + `cargo clippy --workspace --all-features`
- `cargo test --workspace --all-features` 全绿
- `LK_GC_STRESS=1` 下跑 core 测试
- 更新 handoff.md / progress.md

## 非本轮(记录不做)
- clang `-O2`(仅 MIR 管线、需先过差分+sanitizer)—— 性能项
- 函数级 tiering / Cranelift —— 需差分覆盖混合模式后再议
- `LK_DISABLE_FACTS` 旁路 —— 依赖 verifier 落地后评估收益
- native cache key 不含 import 内容 —— AOT 尚不支持 import,在 cache 代码处留注释
