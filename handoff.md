# Handoff

**本轮:正确性优先加固(plan.md)已全部完成**。`cargo test --workspace
--all-features` **1701 passed / 0 failed**(基线 1684,+17)。细节见
`progress.md` 顶部章节与 `plan.md`。

## 本轮发现并修复的真实 bug

1. **`.lkm` for 循环运行时错误 + while 死循环(严重)**:`FunctionData.performance`
   曾为 `#[serde(skip)]`,而 `ForLoopI`/`GetIndexStrI`/`SetIndexStrI` 无 fact 即失败,
   compare-test 无 fact 时把下一条指令误读为 Jmp → 死循环。修复:全量序列化
   `PerformanceFacts`,artifact 版本 **3→4**。
2. **lkrt 测试代码 UB**(Miri Stacked Borrows 抓出):`&CStr::as_ptr().cast_mut()`
   传给 `CString::from_raw`;已改用原始 owned 指针(`lkrt/src/host.rs`)。

## 本轮新增防线

- **bytecode verifier**(`core/src/vm/verify.rs`):`.lkm` 加载期逐指令验证寄存器/
  跳转/常量池/函数索引 + facts(不可信输入);`into_module` 无条件跑;
  `compile_module` debug 下自验(全测试套=防误杀语料)。
- **MIR validate() 进生产路径**(`llvm/backend.rs`,原先只在测试跑)。
- **legacy fallback 改 opt-in**:MIR 拒绝默认响亮失败;`LK_AOT_LEGACY=1` 或
  `allow_legacy_fallback` 显式开启。~200 个 llvm 测试已显式 pin
  (`legacy_fallback_options()`)。
- **GC stress**:`LK_GC_STRESS=1` 每安全点强制 collect;core/stdlib/cli 全绿。
- **差分语料扩展**:examples/ 全目录差分(44 例:2 可 lower 且一致,42 记录
  Unsupported 快照);生成式差分 fuzz(`cli/tests/aot_fuzz_differential_test.rs`,
  种子化,600 例两种子全部干净,同时锁定 lower() totality)。
- **sanitizer**:`LK_NATIVE_SANITIZE=address,undefined` 透传 clang;全部差分语料
  ASan/UBSan 零报告;lkrt Miri 全绿。Makefile 新增 `miri-lkrt` /
  `sanitized-differential` / `gc-stress`。
- **语义仲裁文档** `docs/semantics.md`(golden vectors:div/0、缺失键、nil、
  float 显示、退出语义)。

## 剩余 / 后续

1. 阶段 4(闭包/间接调用/可变全局/方法分派)与 legacy 退役 —— 原计划不变。
2. 性能项(clang `-O2` 仅 MIR、AOT 基线重测)—— 见 plan.md「非本轮」。
3. fuzz 生成器可扩形状(for-range、int-key map 写、bool 列表)以提升 lower 覆盖率。
