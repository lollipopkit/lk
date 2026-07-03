# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(37 步逐项状态)。本会话已推送 **59+ commit** 到 `dev`。完成度:**✅29 · [~]3 · [ ]4 · [!]1**。

## ✅ 已完成/大幅推进(遍及全部 6 相)
- **Phase 0** 完整;**M3 完整**(嵌入 API + register_fn + 多实例 + 沙箱 builder + C ABI 端到端跑出 42 + `eval_value` 类型化结果)。
- **M0**:🎯 去全局状态 · **lk-values 抽取 + 真 no_std**(wasm32)· **lk-hal**(no_std)· CI no_std 冒烟 + **lk-core 无 async 可构建守卫**。
- **M1**:VM(source)==VM(bytecode) 差分 · conformance 声明 · **`.lkm` 字节码缓存**(`LK_CACHE=1`,坐实其为缓存非分发)。
- **M2**(错误/沙箱模型完整):**pcall/error · 可捕获 assert · 一等基本错误值 · try/catch · fuel+内存+模块白名单三沙箱**;
  **traceback debug-name 地基已落地**(`Function.debug_name` 源码名下沉字节码 + artifact 序列化,往返测试)。
- **M4**:**AOT Tier 0**(`lk bundle`→自包含 ELF)· AOT==VM 差分门禁(CI+ASan/UBSan/fuzz)。
- **M5**:**WASM(wasm32+CI)· lk fmt · M5.4 删中心化注册表(-5000 行,收敛为 git+lockfile 去中心化依赖)· LSP 双轨**。
- **新 crate**:`values/`(L0 no_std)· `hal/`(L0 no_std)· `api/`(L5,ffi+lk.h+eval_value)。
- 端到端验证:C ABI、Tier 0 exe、三沙箱、一等错误值、try/catch、WASM、fmt、git 依赖 fetch、字节码缓存。**全量 1449 tests 0 失败。**

## 剩余(真正的深度架构工作,单会话不可做成 green 连贯单元)
- **[!] callable trait 反转**:`CallableValue::Runtime(Arc<vm::RuntimeCallable>)` @ `val/runtime_model.rs:182`,内嵌
  `Arc<Module>` @ `vm/runtime.rs:16`。改 `dyn` 需同步改 GC 追踪 `val/runtime_model/heap.rs:185-193`、跨模块传递 `heap.rs:430`、
  调用点 `vm/exec/runtime_callable.rs`——**枚举变体一变全部 match 原子断裂**。**注意**:它是 lk-vm-core **内部**依赖(val+vm 同 crate),
  **不是**抽 lk-vm-core 的前置。
- **[!/M0.7/8] 抽 `lk-vm-core`**:分离 VM 核心(token/ast/expr/stmt/typ/vm/val/gc)↔std-heavy(package/net/process/rt/aot)。
  **地基已就绪**:`cargo build -p lk-core --no-default-features` 通过(async 已可选,CI 已守卫)。**下一步阻塞**:core 仍用
  `std::fs/process/env` → 需先把 std-heavy 模块移出(big-bang crate 移动,`crate::` 路径全改,非增量)。解锁 M0.9/M5.1/M5.2。
- **[~] M2.2 traceback 显示端**:地基(debug_name)已完成。显示端两条路都被真实约束卡住:错误展开(anyhow context)改
  `err.to_string()` → 断掉**全仓 111 处**错误字符串断言;ctx 帧栈(`push_call_frame` @ `vm/context.rs:185` 已存在但死)
  每次调用 push/pop 撞 **perf 硬门禁**。须连同错误显示契约(CLI `{:#}` 全链)+ 那批断言一次性重做。
- **[ ] M2.5 stackless**:VM 执行模型重写(trampoline)——多天。
- **[ ] M4.2 Tier 1**:MIR `Unsupported` 改逐函数回退 VM——大改 codegen/lower,多天。
- **[ ] M5.1 三 profile / [~] M5.2 MCU / [~] M0.9 alloc-only CI**:均依赖 lk-vm-core 先抽出。

## 护栏 & 续接
全量 1449 tests 0 失败 / `-D warnings` 0 / fmt+clippy 0 / bench 不受影响(沙箱限额单态化零开销;字节码缓存 opt-in 默认关)。
**下一会话最连贯续接 = 抽 `lk-vm-core`**(已核实无需先做 callable、去 async 已就绪 → 从移 std-heavy 模块出核心开始),解锁 no_std profile 整条线。
