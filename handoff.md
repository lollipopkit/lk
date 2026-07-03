# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(37 步逐项状态)。本会话已推送 **44 commit** 到 `dev`。完成度:**✅26 · [~]6 · [ ]4 · [!]1**。

## ✅ 已完成/大幅推进(遍及全部 6 相)
- **Phase 0** 完整;**M3** 完整(嵌入 API + register_fn + 多实例 + **沙箱 builder** + **C ABI 端到端跑出 42**)。
- **M0**:🎯 去全局状态里程碑 · **lk-values 抽取 + 真 no_std**(wasm32)· **lk-hal**(no_std)· CI no_std 冒烟。
- **M1**:VM(source)==VM(bytecode) 差分 · conformance 声明 · .lkm 内部产物标注。
- **M2**(错误/沙箱模型完整):**pcall/error · 可捕获 assert · 一等基本错误值 · try/catch · fuel + 内存上限 + 模块白名单三沙箱**。
- **M4**:**AOT Tier 0**(`lk bundle`→自包含 ELF,输出与 VM 一致)· AOT==VM 差分门禁(CI+ASan/UBSan/fuzz)。
- **M5**:**WASM(wasm32+CI)· lk fmt · git 去中心化依赖(核心已具备)· LSP 双轨**。
- **新 crate**:`values/`(lk-values L0 no_std)· `hal/`(lk-hal L0 no_std)· `api/`(lk-api L5,ffi + lk.h)。
- 端到端验证:C ABI、Tier 0 自包含 exe、沙箱(fuel/内存/白名单)、一等错误值、try/catch、WASM、fmt。全程 1485 tests 0 失败。

## 剩余(真正的深度架构工作,单会话不可做成 green 连贯单元)
- **callable trait 反转**(M0.1 [!]):改 call **热路径 + GC**——`RuntimeCallable` 内嵌 `Module`/`RuntimeModuleState`,
  executor 直接访问字段;改 trait 对象/索引需**原子改所有访问点**,且触发调用热路径 perf。**这是抽 lk-vm-core 的前置。**
- **M0.7/8 抽 `lk-vm-core`**:从单体 core 分离 VM 核心(token/ast/expr/stmt/typ/vm/val/gc)↔std-heavy(package/net/
  process/rt-tokio/aot),feature-gate rt/net。方法同已验证的 lk-values(解耦→分离→抽 crate→no_std)。**多天。** 解锁 M0.9/M5.1/M5.2。
- **M2.5 stackless**:VM 执行模型重写(trampoline `Sequence::step`)——多天。
- **M4.2 Tier 1**:MIR 后端 `Unsupported` 改逐函数回退 VM——大改 codegen/lower,多天。
- **M2.2 traceback**:需 call-frame 追踪入 call 热路径(perf 敏感);`push_call_frame` 已存在但未接入执行。堆对象一等错误值需 GC rooting。
- **M5.4 删中心 registry**:破坏性删除(删 `registry.rs`/`pkg serve`/publish/keyring + 其测试)——**删工作代码,需用户显式确认**;git+lockfile 去中心化核心已具备。

## 代码层已核实的精确阻塞点(下一会话零摸索)
- **callable 反转**:`CallableValue::Runtime(Arc<vm::RuntimeCallable>)` @ `core/src/val/runtime_model.rs:182`;`RuntimeCallable` 内嵌
  `Arc<Module>`(字节码)@ `core/src/vm/runtime.rs:16`。改 `dyn` 需同步改:GC 追踪 `core/src/val/runtime_model/heap.rs:185-193`、
  跨模块传递 `heap.rs:430`、调用点 `core/src/vm/exec/runtime_callable.rs`。**枚举变体类型一变全部 match 原子断裂,不可增量保绿** → 需专注会话一次性做。
- **traceback(M2.2)**:`Function` @ `core/src/vm/ir.rs:659` **无 name/line 字段**;`Module`@`ir.rs:672` 无函数名表 → **字节码丢弃了名字和行号**。
  已存在 `CallFrameInfo`/`push_call_frame`/`call_stack_report` @ `core/src/vm/context.rs:44-225` 与 `ErrorVal.trace` @ `runtime_model.rs:233`
  **均为死代码,执行器零调用**。先决基础设施:给 `Function` 加 `debug_name: Option<Arc<str>>`,在 `compile_function_body`
  @ `core/src/vm/compiler/entry.rs:146` 下沉时填名,`FunctionData` @ `core/src/vm/artifact.rs:139` 序列化 + 版本号 bump。
  **张力**:error 展开式 traceback 会改 `err.to_string()`(断掉 `.contains("step limit")` 类断言);ctx 帧栈式有热路径 push/pop 成本 →
  须连同错误显示契约(CLI 打 `{:#}` 全链)+ 相关测试**一起重做**。

## 护栏 & 续接
全量 workspace tests 绿(1485)/ `-D warnings` 0 / fmt+clippy 0 / bench 非热路径不受影响(沙箱限额走单态化零开销路径)。
**下一会话最连贯续接**:①给 `Function` 加 `debug_name` + 填名 + 序列化(bounded,可保绿)→ 铺 traceback 地基;
②再 callable trait 反转 → 抽 `lk-vm-core`(解锁 no_std profile 整条线)。两者都需专注会话,已核实的入口点见上。
