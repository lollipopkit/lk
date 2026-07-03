# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`(37 步逐项状态)。已推送 **70+ commit** 到 `dev`。完成度:**✅33 · [~]1 · [ ]3 · [!]1**(M2.2 收尾完成);当前主线=M0.7/8 no_std flip 收尾。

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

## 本轮进展(M0.7/8 no_std 大推进)
- **VM 核心 no_std 就绪地基**(commit `0b2b284`):新 `core/src/compat.rs`(collections/sync::Mutex/path/prelude
  兼容层,std 默认逐字节不变);**140+ 文件**机械转换 std→core/alloc/compat + prelude;gate 掉 no_std 无意义叶子
  (ResourceHandle OS 变体、gc_stress env)。**no_std 编译错误 2481→~20**(仅剩 std-only 叶子)。
- 验证:default std + **1451 tests** / fmt / clippy(-D warnings)全绿;`--no-default-features` 0 警告
  (hashbrown/spin/compat 路径已被该 config 实际编译)。**`#![no_std]` flip 暂缓**(见下)。

## 剩余(真正的深度架构工作)
- **[!/M0.7/8] `#![no_std]` flip 收尾**(最连贯续接,~20 错误全在 2 处 std-only 叶子):
  ① gate `stmt::import` 的 `ImportResolver`(DashMap/PathBuf/fs 文件导入)+ 消费者 `vm/exec/program.rs` 的
     `execute_imports`/`collect_program_imports`;保留纯数据类型 `ImportStmt/ImportSource/ImportItem`。
  ② gate macro_system 3 文件的 fs/process 函数:`imports.rs`(fs 加载宏文件)/`procedural.rs`(std::process 跑外部
     proc-macro)/`proc_deps.rs`(fs 指纹)——**保留纯数据类型**(ProcMacroRequest 等),只 gate fs/process 叶子;
     syntax.rs 用的宏类型随之保留、`base_dir`/`expand_*` 走 compat::path + std-gate。
  ③ 翻 `#![cfg_attr(not(feature="std"), no_std)]`(lib.rs 已有注释占位)→ CI `--no-default-features` 变真 no_std 检查。
  解锁 M0.9(`lk-vm-core --features alloc` 冒烟)/M5.1(三 profile 单 crate)/M5.2(full-VM-on-MCU)。
  **本轮深挖发现(flip 还需依赖级 no_std 改造,非纯源码 gate)**:
  - `anyhow`(VM 核心 `execute()->anyhow::Result` 遍布)需改 `default-features=false` + lk-core `std` feature 转发
    `anyhow/std`(1.81+ 用 `core::error::Error`,已把 5 处 `std::error::Error`→`core::error::Error`)。
  - `serde_json`(artifact.rs 字节码序列化 / val/de.rs / import.rs)需 `default-features=false,features=["alloc"]`+std 转发。
  - `serde_yaml`/`toml`(val/de.rs 反序列化)std-only → no_std 下 gate。
  - `dashmap`(import.rs ModuleResolver 的 file/package 缓存)std-only → 字段+方法 `#[cfg(std)]` gate;
    ModuleResolver 无需整体 gate(VmContext 不动),内部 no_std 化=保留 stdlib_registry 解析、gate 掉
    search_paths/runtime_file_modules/package_modules + 所有 Path/fs 方法(components/canonicalize/exists/join/…全 std)。
  - `serialize_imports`/`deserialize_imports`(serde_json)+ `resolve_source_runtime_with_base` 的 `base_dir:Option<PathBuf>`
    随之 gate。→ **flip 是跨 4+ 依赖 + 热路径(artifact 序列化)的多会话单元**,须整体绿再翻,勿半成品提交。
- **[!] callable trait 反转**:`CallableValue::Runtime(Arc<vm::RuntimeCallable>)` @ `val/runtime_model.rs`,内嵌
  `Arc<Module>`。改 `dyn` 需同步改 GC 追踪/跨模块传递/调用点——枚举变体一变全部 match 原子断裂。lk-vm-core **内部**事,非 flip 前置。
- **[ ] M2.5 stackless**:VM 执行模型重写(trampoline)——多天。
- **[ ] M4.2 Tier 1**:MIR `Unsupported` 改逐函数回退 VM——大改 codegen/lower,多天。

## 本轮另完成:M2.2 堆对象一等错误值(遗留清除)
`error(String/List/…)` 现一等携带(GC-root pin 跨展开),pcall 原样取回;uncaught 用 `LkRaisedValue.rendered`
出消息。commit `a3533a4`。**M2.2 无遗留**。GC-stress 1095 tests 验证 rooting。

## 护栏 & 续接
全量 **1451 tests** 0 失败 / **GC-stress 1095** / `-D warnings` 0 / fmt+clippy 0 / bench 不受影响
(compat 在 std 下路由到 std HashMap/Mutex 零行为变化;M2.2 rooting 仅 Err 冷路径)。
**下一会话最连贯续接 = no_std flip 收尾**(gate `stmt::import` resolver + macro_system 3 文件 fs/process + 依赖级
anyhow/serde_json no_std → 翻 `#![no_std]`),解锁 no_std profile 整条线。剩余深度项:M2.5 stackless、M4.2 Tier 1、callable trait 反转。
