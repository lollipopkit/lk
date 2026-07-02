# Handoff

**本轮:"先补再删"完成** —— MIR 补齐 println/print/assert/长字符串/TestEqIntI2 形状后,
**legacy text 后端整体退役**(净 -4.8 万行)。`cargo test --workspace --all-features`
**1460 passed / 0 failed**(约 240 个 legacy 测试随后端退役)。细节见 `progress.md`。

## 现状架构

- **MIR 管线是唯一 AOT 后端**:`ModuleArtifact → lk-aot-lower →
  lk_aot_mir::validate(生产路径强制)→ lk-aot-codegen → clang + liblkrt.a`。
  MIR 拒绝即响亮失败(带 `Unsupported` reason);无 fallback、无 VM shell。
  `use_mir_pipeline`/`allow_legacy_fallback`/`LK_AOT_MIR`/`LK_AOT_LEGACY` 全部移除。
- **本轮新增 MIR 形状**:`LoadHeapConst` 长字符串;`GetGlobal` runtime builtin
  (`println`/`print`/`assert`)——常量格式串在 lower 期按 `format_variadic_runtime`
  精确展开(`{}` 消耗、缺参保字面、多参空格追加;唯一运行时歧义 case 拒绝),
  循环外提的格式串靠只读到达定义回溯(`reg_const_str`,穿 phi 防环)恢复;
  `TestEqIntI2` 双寄存器 fused 比较(MIR 加 `BoolAnd`)。
- **本轮发现并修复的分歧**:native abort 丢弃 C stdio 缓冲的 stdout(assert
  失败/除零后已打印内容消失,VM 保留)→ 所有 abort 路径统一走 `lkrt_abort`
  (`fflush(NULL)` 后 abort),codegen `Term::Abort` 同步。差分用例
  `assert_false_after_output`/`div_zero_after_output` 锁定。
- **覆盖现状**:手写差分 7 组(新增 builtins 16 例);fuzz 92%
  可比较(println 形状入生成器);examples 3/44(长尾在模块 builtin:
  `os.clock`/`math.*`、closures、NewObject 等);bench 全套 AOT 仍 skip
  (第一个卡点 `os.clock`)。
- lkrt:legacy 线性容器 helper(containers.rs ~2000 行)+ abi schema 60 条一并删除;
  新增 `lkrt_abort`/`lkrt_assert`/`lkrt_assert_msg`。

## 剩余(阶段 4 / 后续)

1. **模块 builtin 下降**(`os.clock`/`math.floor` 等 GetGlobal+GetIndex+Call 形状)——
   bench 全套 AOT 与 examples 覆盖的最大头。
2. 阶段 4:闭包/间接调用/可变全局/`__lk_call_method` 方法分派。
3. 混合元素常量容器(`[1, true, "s"]`)。
4. 性能项(plan.md「非本轮」):clang `-O2`(现在只有 MIR 管线,阻碍已清)、AOT 基线重测。

## 上轮(正确性加固)成果仍然有效

bytecode verifier、facts 序列化(artifact v4)、GC stress、三大差分语料、
sanitizer/Miri 接入、docs/semantics.md。Makefile:`miri-lkrt` /
`sanitized-differential` / `gc-stress`。
