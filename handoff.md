# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。细节台账在 `progress.md`。

## ✅ 主线状态(2026-07-09,全部收官)

- **plan.md v1.0 六项全部达成**(2026-07-04);Phase 0 / M0–M5 完成。
- **v2 语言面重设计已落地**(2026-07-06,用户裁决):Swift 式错误模型(try/catch 唯一捕获面 +
  后缀 `!` 解包,删 pcall)+ Go 式并发(`go` 关键字 + goroutine + 阻塞 channel + select,
  删协程/yield/sched)。语义细节见 `docs/concurrency.md` / `docs/semantics.md`。
- **M4.2 AOT 深覆盖终局:51/51 满额**(14 → 25 → 50 → 51)。途中顺手修复 8+ 个 VM bug、
  两套 fixpoint 重猜机制(loop phi / 空[])、fuzz Dyn 面 120/120。语义裁决全部入
  `docs/semantics.md`(三套 eq/句柄语义/map 迭代序 Fx 镜像/错误文本)。
- **PR #17 已合并**(2026-07-07,v2 错误模型/并发 + 深覆盖主线)。
- **PR #20 已合并**(2026-07-09,打磨轮 P1-P10):clippy --all-targets 清零 + CI gate ·
  raise 前缀统一 · docs/macros.md · select Condvar 唤醒 · lower 三张声明表 ·
  fixpoint 省 clone · unsupported.lk 全翻转。细节 progress.md「打磨轮」。

## 进行中:Tier 1 收尾三阶段(2026-07-09 起,计划文件 playful-meandering-sloth.md)

**更正**:Tier 1 hybrid v1 早已全套落地(opt-in `LK_AOT_HYBRID=1`,五子步完成,
docs/llvm/tier1-hybrid.md)——此前 handoff 把它列为"待做大项"是误读。真实剩余:
- **A ✅ 修 correctness 夜跑三回归**(07-07 起连红 5 轮):A0 harness 补 stderr ·
  A1 fuzz 超时=staticlib 冷构建吃进 per-case 60s(预热+CI prebuild)· A2 sanitized
  分歧=LSan 杀缓冲 stdout(JmpBuf 备用槽复用修真泄漏 + 差分 detect_leaks=0)·
  A3 GC stress VM 真 bug=宿主 HOF 累积值无 root(**host_roots 机制**,
  map/filter/reduce/stream 全钉 + 确定性 stress 回归基建)。细节 progress.md
  「Tier 1 收尾轮」。分支 fix/correctness-nightly-regressions,PR 待开/合并。
- **B 待做:翻默认**(前置=PR-A 合并 + workflow_dispatch 触发 correctness ≥3 轮全绿):
  lower :1144 is_none_or · hybrid_off_by_default 翻转 · fuzz 删显式 env ·
  aot_coverage.sh 钉 LK_AOT_HYBRID=0 · tier1-hybrid.md 状态行(现仍写 not
  implemented,过时)+ CLAUDE.md。
- **C 进行中:v2 桥接返回值**(分支 feat/hybrid-v2-return-bridge,stacked 于 PR-A):
  - ✅ **C1-C5 完成并全验证**:C1 core keep-state(ModuleFunctionOutcome,修
    v1 悬空)· C2 lk_hybrid_call_r 标量档(LkDyn 镜像 + lk-api dev-dep lkrt
    conformance 钉 tag/布局)· C3+C4 MIR CallVm{dst}+lower dst=Dyn+codegen
    call_r(dst 未读降级 call_v 保 v1 零 marshal)· C5 容器深转换(wrapper 注入
    lkrt 构造表 lk_hybrid_register_rt;map 按 VM 迭代序重放;String 类型列表 die
    留档=引号 display 无 Dyn 载体)+ **lower 标记→rerun 改不动点迭代**(caller
    因 callee 过期 ret 假设失败/早退缺 param_obs → 每轮暴露新可标函数)+
    **lkrt display_into 补 DYN_MAP 臂**(VM 格式;此前 raise→abort 是更差分歧;
    静态 map display 的 lowering 拒绝裁决不动)。e2e:标量五档+容器全链路
    (字段读/Dyn 算术/嵌套索引/整 map 显示)与 VM 逐字节一致;coverage 51/51;
    三差分全绿。
  - ✅ **C6 完成(raise 跨桥)**:ModuleFunctionCall::{Return,Raise}(LkRaisedValue
    downcast 保一等值;LanguageRaise→String 镜像 VM catch 语义);bridge_call 统一
    v/r,Raise 先 drop guard/state 再经注入 lkrt_rt_raise_dyn 进最近 native try;
    lkrt no-handler 打印 uncaught 到 stderr。typeof(Dyn) 留子集外(既有边界)。
  - ✅ **C7 完成(fuzz+文档)**:帮手换动态格式串 + '全原生编译即报警'断言 +
    四类形状(丢弃/标量消费/容器消费/raise 被 try 捕);tier1-hybrid.md 状态行
    + v2 章节。
  - **C 全套验证**:workspace -D warnings 0 失败 · 三差分+sanitized · GC stress
    1053/0 · miri lkrt · coverage 51/51 · bench 1.009x · fuzz 40+200。
    **PR-C 待 PR-A 合并后 rebase 开出**(分支 stacked)。

## 打磨轮:文件拆分 + macOS 链接修复(2026-07-16)

- **P0 8 个 >1500 行文件全部拆到子模块,零回归**(`aot/lower/lib.rs` 11778→21 文件
  含 `lower_inst` fallthrough 委托链;compiler/exec/parser/core_methods/cli-main/
  runtime_callable 等)。全仓最大源文件降至 1483。细节见 progress.md「P0 拆分」。
- **修既存 bug**:macOS native AOT 链接补 `-framework CoreFoundation`
  (`iana_time_zone`←`chrono` 的 `_CF*` 符号)→ 12 个 differential 门禁 macOS
  本地首次全通。清 core 一个 clippy `needless_return`。
- **验证**:`cargo test --workspace --all-features` 1536/0 · clippy
  `--all-features --all-targets -D warnings` 0 · fmt 0 · coverage 51/51。
- **P1 架构收敛评估结论=不做**:①core 已整体 no_std 可编译(强于分 crate),
  拆 crate 低 ROI 高风险;②lkrt thread_local/longjmp 是 C-ABI native runtime
  的固有模型 + 刚发布并 Miri 验证的 hybrid 承重件,非缺陷。均属受控设计。

## 剩余(可选/留档)

- goroutine 死锁检测(泄漏检测之外)。
- lkrt 静态库 sanitizer 重编(-Zbuild-std 触发 E0152 留档;现靠混合配置差分兜底)。
- FFI 增强:ergonomic Value 转换层 · register_module 命名空间 · rooted handle。
- M2.6 内存上限(fuel 已有)。
- typed 列表方法长尾对 ListDyn receiver(按需补,无驱动语料不动)。
- full-VM-on-MCU(留待 lk-vm-core;WASM/MCU 冒烟已过)。

**✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 feature 拆分。

## 护栏 & 门禁等价性

- 全量 tests 0 失败 / clippy --all-targets 0 / fmt 0 / 差分门禁逐字节 / bench 噪声带(0.99-1.02x)。
- **本地与 CI 等价必须**:`cargo test --workspace --all-features` + `RUSTFLAGS=-D warnings`;
  clippy 带 `--all-targets`(测试代码也在 gate 内)。
- AOT 改动:`scripts/aot_coverage.sh` 单调不降 + 差分门禁 + bench 纯噪声。
