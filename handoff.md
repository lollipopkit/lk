# Handoff

**目标已达成**:`docs/llvm/aot-redesign.md`(AOT 后端重设计)核心设计 §2-§6 **全部落地**,
MIR 管线为**默认后端**。`cargo test --workspace --all-features` **1684 passed / 0 failed**。
细节见 `progress.md` 与 RFC §9.5。

## 最终架构(已 LIVE)

- 四 crate 管线:`aot/abi`(schema 单一真相,`for_each_abi_fn!` 数据宏)→ `aot/mir`
  (类型化 SSA + `render()` 快照文本 + `validate()`)→ `aot/lower`(总函数
  `lower() -> Result<MirModule, Unsupported>`,含 Braun SSA/类型追踪/单态化)→
  `aot/codegen`(total `render_module` → LLVM 文本)。
- **默认开**:`llvm/src/llvm/backend.rs` gate;`LK_AOT_MIR=0` 或
  `LlvmBackendOptions::use_mir_pipeline=Some(false)` 退回 legacy text 后端
  (fallback,拒绝时错误同时带 legacy + MIR `Unsupported::reason()`)。
- **测试面**:差分 harness `cli/tests/aot_differential_test.rs`(69 例 VM vs native,
  Path::New 断言走新路径);MIR 快照 `aot/lower/tests/mir_snapshots.rs`(6 形状);
  lkrt `abi_conformance_test`(140 条 schema ↔ 实现签名,编译期 arity/extern 强制);
  34 个 legacy-IR 结构断言测试已 pin `legacy_text_backend_options()`(随删 legacy 退役)。
- **所有权**:默认 arena(字符串 + 容器句柄注册,entry 退出调 `lkrt_cleanup`)+
  concat 链死临时串 eager `lkrt_string_free`。
- **覆盖**:标量全量(guarded div/mod、VM-exact 浮点显示)、控制流(if/loop/嵌套/融合分支族/
  BrMod/BrNil)、直接调用(i64/f64/bool 参数/返回按调用点单态化、递归、死函数跳过)、
  容器句柄(List<i64/f64/str> + Map{str,i64}×{i64,f64} 全 4 格;Maybe<{i64,f64,str}>
  present-bit 模型:return→nil、算术→abort=VM halt、==nil→present 位)、字符串
  (eq/concat/插值/join,显示逐字节 =VM)。

## 已知分歧修复记录

`return nil;`:legacy native 打印 `nil`,VM/MIR 打印空 —— 差分测试抓出,CLI 测试已改锁定 VM 行为。

## 剩余(均为 RFC §1 非目标 / §7 约定的后续)

1. **阶段 4**:闭包/间接调用/可变全局 + `__lk_call_method` 方法分派(list `.sort()` 等)——
   RFC 明确非目标;扩展点就位(`Ty` 加变体 + lower 加 arm)。
2. **删 legacy text 后端**:待 MIR 吸收 legacy 独有形状(方法分派/对象/try)后整体退役
   (连同 34 个 pinned 测试 + `dynamic_containers/`,预计 -2~4 万行)。
3. 性能:AOT 侧可跑 `RUN_AOT=1` bench 对比句柄化前基线(VM perf 门禁不受影响,未触碰解释器)。
