# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(37 步逐项状态)。本会话已推送 **66+ commit** 到 `dev`。完成度:**✅31 · [~]3 · [ ]2 · [!]1**。

## ✅ 已完成/大幅推进(遍及全部 6 相)
- **Phase 0** 完整;**M3 完整**(嵌入 API + register_fn + 多实例 + 沙箱 builder + C ABI 端到端跑出 42 + `eval_value` 类型化结果)。
- **M0**:🎯 去全局状态 · **lk-values 抽取 + 真 no_std**(wasm32)· **lk-hal**(no_std)· CI no_std 冒烟 + **lk-core 无 async 可构建守卫**。
- **M1**:VM(source)==VM(bytecode) 差分 · conformance 声明 · **`.lkm` 字节码缓存**(`LK_CACHE=1`,坐实其为缓存非分发)。
- **M2**(错误/沙箱模型完整):**pcall/error · 可捕获 assert · 一等基本错误值 · try/catch · fuel+内存+模块白名单三沙箱**;
  **traceback 完整**(`Function.debug_name` 下沉字节码 + 错误传播分支 push ctx 调用栈 + pcall 捕获清空,CLI 打印命名调用链;
  仅 Err 路径零热成本、不碰 to_string 断言)。**唯一遗留**:堆对象(String/List)一等错误值(需 GC rooting 跨展开)。
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
- **[ ] M2.5 stackless**:VM 执行模型重写(trampoline)——多天。
- **[ ] M4.2 Tier 1**:MIR `Unsupported` 改逐函数回退 VM——大改 codegen/lower,多天。
- **[ ] M5.1 三 profile / [~] M5.2 MCU / [~] M0.9 alloc-only CI**:均依赖 lk-vm-core 先抽出。
- **M2.2 堆对象一等错误值**(唯一小遗留):`error("str")`/`error([..])` 目前 native 包装;首类化需 GC rooting 跨错误展开(把堆值 root 住直到 pcall 取回)。

**剩余全部要么是 big-bang crate 移动(lk-vm-core → 解锁 3 步)、原子热路径改动(callable)、执行模型/codegen 重写(M2.5/M4.2)。**

## 护栏 & 续接
全量 1451 tests 0 失败 / `-D warnings` 0 / fmt+clippy 0 / bench 不受影响(沙箱限额+traceback 均仅 Err/单态化冷路径;字节码缓存 opt-in 默认关)。
**下一会话最连贯续接 = 抽 `lk-vm-core`**(已核实无需先做 callable、去 async 已就绪 → 从移 std-heavy 模块出核心开始),解锁 no_std profile 整条线。
