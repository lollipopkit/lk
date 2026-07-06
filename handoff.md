# Handoff

**目标(`/goal`)**:把 `plan.md` 划分为多步、逐个完成、每步 push。**用户:允许短期回归、fix-forward。**
细节台账在 `progress.md`。已推送 **125 commit** 到 `dev`。
**🎯 plan.md v1.0 六项全部达成(2026-07-04)**。**v2 语言面重设计已落地(2026-07-06,用户裁决)**:
**Swift 式错误模型**(删 pcall,try/catch 唯一捕获面 + 后缀 `!` 解包,错误一律 raise)+
**Go 式并发**(删协程/yield/sched,`go` 关键字 + spawn goroutine + 阻塞 channel + select)。

## ✅ 主线状态
- **Phase 0 / M0–M5** 全部达成(v1.0 定义);M2.5 stackless `Vec<CallFrame>` 地基保留。
- **v2 错误模型**:`error(v)` raise 一等错误值 · try/catch(→隐藏 `try$call`)· 后缀 `!`
  (nil → raise "unwrap of nil value";`!` 紧跟 `(`/`[`/`{` 留给宏调用;`x!==1` 需写 `x! == 1`)·
  无 `[ok,value]` 对(非错误的"暂无"用 nil:chan.try_recv 空 / task.try_await 未完成)。
- **v2 并发**(`docs/concurrency.md`):`go f(x);` / `spawn(闭包)`(快照 promote:module Arc +
  捕获/globals 同模块结构深拷贝)· goroutine 内阻塞 send/recv(block_in_place)· chan.close
  Go 语义(缓冲可排空)· select 对 closed channel always-ready(nil binding)· **isolate 深拷贝**
  (裁决:单线程无锁 GC 是底线;通信走 channel,比 Go 更严格的 CSP)。
- 全量 **1499+ tests 0 失败**(核心 953)· `MODULE_ARTIFACT_VERSION` = 9。

## 本轮(v2)五子步,commits `33d3fb9`/`a16da0e`/`cbf0e10`/`a910eb3`/本次
1. 全删协程/yield/sched(-2587 行,artifact v9);select/chan/task/spawn 存活。
2. 修 spawn(闭包):`copy_runtime_value_same_module`(ClosureCopy 模式)+ 快照 promote;
   **Runtime::block_on 多线程 flavor 走 block_in_place**(goroutine 内阻塞收发成立的关键)。
3. `go` 关键字(parse 时糖 → spawn);顺手修 send/recv typecheck 对 fn 参数类型变量误报。
4. 错误模型迁移:pcall→try$call · `!` 解包(与宏调用消歧)· recv/send/try_* raise 语义 ·
   chan.close 改可排空 · 语料/测试全迁(error_unwrap.lk 替代 pcall_error.lk)。
5. 文档:docs/concurrency.md 新写 · semantics.md/stdlib.md · plan.md 4.4/4.5 裁决注记。

**实测踩坑留档**:goroutine 内 block_on panic("runtime within runtime")→ block_in_place;
`!` 与宏调用 `name!(...)` 冲突(宏三种定界符都在用)→ 消歧规则;chan.close 旧行为 remove
导致缓冲丢失 + "Channel not found" → 改标记式关闭;native raise 带 "native ... failed:" 前缀
(LK 层 catch 到的字符串,error() 一等值无此前缀)。

## 本轮追加(用户指示:修小项+文档+LSP 补齐+AOT 排查)
- ✅ spawn 复用 shared Arc<Module>(免每次深克隆)+ `task.stats()` 观测面(commit `3bca22e`)
- ✅ LSP/编辑器补齐 v2 语法(commit `eea81f0`):lk-lsp 语义 token + completion 关键字
  (go/try/catch,后两者是既有缺口);tree-sitter 新增 go_statement/try_statement/
  unwrap_expression(**踩坑**:macro_invocation 静态 prec(21) 会压过 unwrap 且跳过 GLR,
  改 prec.dynamic + conflict 对;语料 9/9);tmLanguage/highlights.scm 同步。
  zed-ext-check 失败是既有工具链问题(futures-core@wasm32-wasip1,基线同样失败)
- ✅ README/README.zh-CN「A Taste/一瞥」可运行示例(commit `5c5ec5f`,实测输出锁定)
- ✅ **M4.2 排查完成**(本 commit):`scripts/aot_coverage.sh` 可复现扫描,14/51,
  阻塞排行+路线图入 progress.md「M4.2 AOT 深覆盖」章节

## 剩余
- **[~] M4.2 AOT 深覆盖(Dyn 实现进行中,计划文件 synthetic-plotting-pony.md 有完整设计)**:
  - ✅ **D1 已落地**(commit `0928b58`):LkDyn{tag,payload} 载体 + abi DynVal 词表 +
    32 个 dyn ABI 条目 + mir Ty::Dyn/ListDyn + codegen llvm_ty;零 lowering 改动,覆盖率 14/51 不变
  - ✅ **D2 混合列表已落地**(commit `2e42ffd`):LoadHeapConst 混合标量列表→ListDyn、
    GetIndex(const/动态统一 dyn_at→Dyn)、display、入口 return Dyn 打印;probe 与 VM
    逐字节一致。**实测钉下 VM 怪癖:混合列表字符串元素裸文显示([1,a b,2]),与
    ListStr 的引号路径不同**(lkrt display_into 已按此实现)
  - ✅ **D2b 已落地**(commit `f41a94e`):MapStrDyn 家族(缺键=Nil tag,无需 Maybe)+
    GetFieldK 臂 + Cmp 全臂(to_dyn 装箱另一侧→dyn.eq/lt…;==nil→dyn.tag==0);probe
    与 VM 逐字节一致;旧限制测试已翻转。map_demo/pattern_matching 推进到 operand
    类型阻塞(D3 领域);template_strings pc23/for_loop_patterns pc34 仍 LoadHeapConst
    (待查常量形状:可能嵌套列表或 LongString 元素——box_const_scalar 现只收 ShortStr)
  - **D3 进行中**:✅ Dyn 算术臂(`1ad4d7a`)+ ✅ NewList 运行时混合/ListPush Dyn 臂/
    **Move 双视图修复**(`8e9b07f`,通用缺陷:带 ArgList ref 的寄存器 Move 丢 SSA handle;
    修复仅对 ArgList 传双半边,其它 ref 会被过期 SSA 掩埋——回归实测踩过)。
    **剩余**:phi 混型合流装箱(Maybe edge_insts 机制,lower:1815)
  - ✅ **嵌套常量全套落地**(commits `a5966ba`/`f3661bb`):嵌套列表([[1,"a"],…])+
    嵌套 map({"address":{"city":…}})递归装箱;DYN_MAP tag + dyn.from_map/field/index;
    GetIndex on Dyn 按键类型分派(I64→index,Str→field);**首例翻转
    template_strings.lk,覆盖率 14→15/51**。两个旧限制测试翻转为 lowers。
    注:VM 前端对混合 map 字段算术 union 推断直接拒绝——native 更宽不构成差分风险
  - ✅ Len/Contains Dyn 臂(commit `dc9922b`):person.len()/"k" in m 原生化,
    str_dyn_has 新 ABI(存 nil ≠ 缺键)
  - ✅ 方法臂/MapStrDyn Str 索引/dyn.len_of(最新 commit)。**裁决留档**:
    map_demo 永久卡 .keys()/.values()(hash 序不可移植,同 display 出子集);
    pattern_matching 卡 MapRest(解构 rest);for_loop_patterns 卡"空 [] push
    Dyn"(需 lazy 物化)
  - ✅ NewRange 落地(i64_from_range ABI,ranges.lk 推进到 iter 模块白名单)。
    **空列表 lazy 物化评估留档**:dyn 猜测会破坏 [] 与 typed 列表的 eq lowering
    (回归风险),需要真正的延迟物化设计(ArgList 机制扩展),不可 rush
  - ✅ iter.range + ListI64 take/skip/chain(**ranges.lk 翻转,16/51**)
  - ✅ 方法臂批次 first/last/get/concat/join(Maybe 模型复用,近零新 ABI);
    list_ops pc49→99。**踩坑**:cargo build -q 2>/dev/null 吞编译错误致 probe
    跑旧二进制——先验证二进制新鲜度再下结论
  - **下一步**:list_ops pc99 定位 · phi 混型合流装箱(lower:1815)·
    sort_search(fn 参数无类型)· NewObject 裁决 · 空列表延迟物化(留档)
  - **D3 待做**:NewList 混合(lower:2883 else 臂)+ phi 混型装箱(照抄 Maybe edge_insts 机制)
    + Dyn 算术全消费点;**D4**:NewRange/方法 ABI 增量/NewObject 裁决
  - 每步必须:aot_coverage.sh 单调不降 + 差分门禁逐字节 + bench 纯噪声
  - GetGlobal 14(try$call/并发/模块白名单)是**另一根因**,独立大项未启
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 feature 拆分。
- **可选后续**:native raise 前缀统一(catch 到的 native 错误带 "native ... failed:" 前缀,
  error() 一等值无)· goroutine 泄漏之外的死锁检测。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 1.021x(基线内)/
差分门禁全过。**下一会话首选:M4.2 Dyn 装箱值地基**(MIR Ty::Dyn + lkrt tagged value,
注意 display/错误信息 VM-exact 逐字节 + semantics.md 已裁决混合 map display 不进子集)。
