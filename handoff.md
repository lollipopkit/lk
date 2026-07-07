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
  - ✅ typed 列表↔Dyn 跨型比较(i64/f64/str_to_dyn 冷路径转换)
  - ✅ **chunk/enumerate/zip/unique/flatten 批次(list_ops 翻转,16→18/51)**
    (commit `ee57511`)。顺带钉三个深怪癖:(1) unique 走 core_methods 版
    runtime_values_equal = **句柄语义**(数值 to_bits、≤7B 字符串按内容、
    列表/map/长串按 handle)→ lkrt unique_eq 专用实现,长串永不去重裁决入
    semantics.md;(2) NewList 混合臂不收列表元素([l,l] 静默不物化→SSA read
    挂)→ 过滤集合扩展 + **窗口内装箱 memo 保句柄同一性**;(3) Cmp Dyn 臂并入
    ListDyn 操作数
  - ✅ **两个真 bug 修复**(commit `610623a`):VM core_methods 列表方法遇
    >7B 字符串 double-unwrap 必 panic(into_iter_owned String 臂)→ 全换
    list_runtime_items 并删病灶;AOT F64 常量 `fadd 0.0,x` 丢 -0.0 符号
    (IEEE 754 加法恒等元是 -0.0)
  - ✅ **phi 混型合流装箱**(commit `59f02db`,match.lk 翻转,19/51):
    add_phi_operands 收集全边后决策,混型全 dyn-boxable → phi 宽化 Ty::Dyn +
    edge_insts 装箱;仅前向 join phi(loop header/自引用 reject)
  - ✅ **'in' 操作符 VM-exact**(commit `dd013c4`,operators.lk 翻转,20/51):
    VM 的 in 是第三套 eq(typed 严格同型无数值 coercion、Mixed=derive
    PartialEq)。**修既有 bug**:(ListF64,I64) coerce 臂 2 in [1.0,2.0] 与
    VM(false)分歧。dyn_contains 换 contains_eq;semantics.md 新增裁决
  - ✅ **range 切片**(commits `7918884`/`e69b007`):**两个 VM bug**——
    字符串切片按字节(多字节 panic,与 s[i]/len 的 char 语义不一致)+
    len<=3 heuristic(s[8..20] 全坏);修后 native 以 range_def side-table
    (全常量 step==1)发射真切片(str slice_chars/i64_slice ABI)
  - ✅ **iter 模块转发 + NewObject=MapStrDyn**(commit `d0f562d`,3 例翻转,
    23/51):iter.map/filter/…/chunk 转发到方法 lowering(HOF 复用
    lower_list_hof_k);struct 实例以 str_dyn map 承载(type_name 不存,
    整对象 display/typeof 不进子集);typed float 算术 + IsNil 补 Dyn 臂
  - ✅ **Str 方法批次**(commit `1e959ee`,string_methods 翻转,**24/51**):
    lower/upper/trim/reverse/repeat/ends_with/find(字节)/substring(字节+
    边界 abort)/replace/chars(→ListDyn 保 Mixed 裸文 display)/is_empty
  - **剩余 27 例全部是已裁决留档或独立大项**:13 GetGlobal(try$call/并发/
    trait 分发——用户明示单独立项)· 9 operand(跨函数 Dyn 流动:可空参数/
    无类型参数/mutable 全局——首版不做)+ MapRest + 空[]延迟物化(留档)·
    4 Call(comprehensive 需 Set 内建类型 + word_count 需 str-lambda HOF/
    动态 map,均留档)· 2 use 文件依赖(compile llvm 不支持多文件)
  - ✅ **迭代/空列表/字符索引批次**(commit `3a0eac3`,control_flow 翻转,
    **25/51**):GetIndex(Str,I64) 单字符读(str_char_at→Dyn)· Str+Dyn
    拆箱 concat(loop 累加器 phi 保型)· ToIter 臂(列表 identity/Str→
    chars/Dyn→as_list)· 空 [] 三态猜测(str/索引读→ListDyn/默认 i64)·
    ListPush 全臂 Maybe unwrap
  - **M4.2 Dyn 深覆盖收官**:14/51 → **25/51**(+11 例)。剩余 26 全为
    留档/大项:13 GetGlobal(单独立项)· 8 跨函数 Dyn 流动 + MapRest ·
    comprehensive(Set 类型)· word_count(sort_words 无类型参数,其内
    str-lambda HOF/动态 map 链路已部分就绪)· 2 use 多文件
  - ✅ **Dyn 折叠点安全审计**(commits `2c586c2`/本次):IsList/IsMap 对
    Dyn 编译期折叠 false → rest 解构守卫错误 Raise;NilBranch 折叠'恒非
    nil' → struct 缺省字段 if 判断**静默走错分支**。四处折叠点(IsNil/
    IsList/IsMap/NilBranch)全部改运行时 tag 判断。SliceFrom 补 Dyn/
    ListDyn 臂(rest 解构 tail 全链路);空 [] 猜测源扩展 SliceFrom/ToIter
  - ✅ **fixpoint 重猜机制落地**(commit `45bd1ce`):loop phi 混型
    (total += p.tags over Dyn 字段)→ retriable DynLoopPhi 错误 →
    fixpoint 重跑时 phi 从创建起就是 Dyn。空[]猜测 lookahead 精化
    (Move 传播/NewList/NewObject 源/LoadHeapConst 按常量种类分流)。
    **sanitizer 欠账补齐**:6 例 + 全特性压力 probe 过 ASan/UBSan 干净
  - ✅ **空[]重猜落地**(commit 本次):EmptyListGuessWrong{pcs} retriable
    ——lookahead 猜错由消费点证伪后 fixpoint 强制 ListDyn 重物化;混合
    push(顺序/循环内 if-else)两形状真原生一致。**Dyn 化重猜机制全套完备**
    (loop phi + 空[]两版)
  - ✅ **fuzz 生成器扩 Dyn 面**(commit 本次):混合列表/混合 push 重猜/
    HOF 嵌套族/struct 四类新 case,LK_FUZZ_CASES=120 全部真原生对比通过
    ——M4.2 语义面(三套 eq/句柄/重猜)自此有 fuzz 回归保护
  - **留档小项**:typed 列表方法长尾对 ListDyn receiver 按需补;
    for_loop_patterns 永久卡 map 迭代(hash 序,与 map_demo 同类);
    lkrt 静态库 sanitizer instrument(-Zsanitizer 重编)
  - **循环任务已收尾**(2026-07-07):清单(D1-D4+机制项+验证面)全部完成,
    连续三轮回归全绿且零变化后 cron f5db44fa 已删除。最终战果:覆盖率
    14/51 → 25/51、6 个 VM bug 顺手修复、两套 fixpoint 重猜机制、
    fuzz Dyn 面 120/120。
  - 每步必须:aot_coverage.sh 单调不降 + 差分门禁逐字节 + bench 纯噪声
- **[~] 深覆盖收尾大计划进行中(2026-07-07 起,用户裁决全部推进)**:
  计划文件 `~/.claude/plans/silly-noodling-river.md`(先修①-⑥ + 再修
  GetGlobal a-d,目标 47-50/51)。**两项用户裁决**:map 迭代序走
  native 复刻 Fx 序(不改 VM);并发走 OS 线程+深拷贝 channel(不引
  tokio 进 lkrt)。
  - ✅ **阶段①完成**(commits `e34841e`/`ed284d1`/`7925690`/`56db77f`):
    跨函数 Dyn 流动全套——A1 参数格点 join→Dyn(from_maybe_* ×4 +
    dyn.truthy + 全类型真值表 + Nil-phi 走 Dyn 宽化)· A2 返回类型
    join→Dyn(dyn_rets retriable + **自递归 ret 即时发布修 stale-I64
    伪异型**)· A3 Dyn 全局(混型/容器槽位装箱,zeroinit=nil tag)·
    A4 MapRest→MapStrDyn。**覆盖率 25→29/51**(error_handling/
    null_coalescing/recursive/pattern_matching)。
    **顺手修 VM bug**:函数内对顶层 let 全局的方法调用被编译成模块
    属性读(list 崩/map 调 nil)→ user_let_globals 集合修路由;
    Maybe 实参此前 unwrap-abort(VM 传 nil)也一并修正。
  - ✅ **阶段②完成**(commits `27ce681`/`0ea88ca`):B1 Set 内建
    (Ty::Set + lkrt lkset + SetCtor;**修 lowering bug**:值返回型
    builtin 需 early-return 跳过统一 nil 尾)· B2 模块小批(string/path
    白名单 + math 长尾 + assert_eq 万能 dyn.eq 兜底)· B3 HOF 三 ABI
    家族(i64 快路径/str/dyn 万能——**dyn_rets 强制 lambda 返回装箱
    是关键机制**;dyn_lt 补字符串字典序)。**覆盖率 29→31/51**
    (comprehensive/sort_search)。
  - ✅ **阶段③完成**(commits `a32b530`/`5e99f07`):C1 宏导入扫描放行
    `..`(**修 bug**:use.lk 此前连 VM 都跑不了)· C2 编译期 bundling
    (CLI 合并依赖函数表+fidx/全局槽重写;lower_bundled + ImportEnv
    解析全部五种 use 形式;GlobalRef::UserModule;str.byte_len)。
    **覆盖率 31→33/51**(use/use_forms 真原生)。
  - ✅ **阶段④完成**(commits `ad39276`/`43aab93`):map 迭代序 native
    复刻全套——lkrt 载体换 hashbrown+Fx(与 core fast_map 同款)+
    vm_mirror.rs(RtKey/ShortStr hash 恒等镜像 + lit 两段协议,
    **order-conformance 直连 lk-core 首跑即过**)+ 迭代面 ABI ×16
    (iter_pairs/keys/values/delete)。for_loop_patterns(打印迭代
    输出)逐字节一致=镜像端到端实证。**又修一个 VM 死循环
    miscompile**:循环字面量缓存别名作变量 home,嵌套循环内重赋值
    rebind 后内层回边读旧寄存器(word_count 在 VM 挂死,差分语料
    一直当 timeout skipped 漏网)。**覆盖率 33→36/51**
    (for_loop_patterns/map_demo/word_count)。
  - ✅ **先修①-⑥全部完成**(⑤ E1 长尾已在途中被覆盖,无独立工作;
    ⑥ F1 commit `ad38f0a`:build_lkrt_asan.sh + LKRT_STATICLIB 注入
    + make asan-lkrt + CI 非阻塞 job;-Zbuild-std 触发 E0152 留档,
    混合配置实测差分全绿)。**覆盖率 25→36/51**。
  - ✅ **G 完成(try$call 原生化,commits `4fd02d7`/`9b9aebe`)**:
    sjlj(MIR TryCall 单指令,codegen 文本展开 diamond,免块手术)+
    运行时 cell 跨界(SSA cell 模型保留,try 边界物化+写回)+ 守卫
    全面改道 raise(panic 保持 fatal;无 handler 落回 abort,try 外
    行为不变)。**覆盖率 36→39/51**(try 三例真原生)。
  - ✅ **H 完成(并发原生化,commits `8c364cb`/`2b0fa77`)**:OS 线程
    +深拷贝 channel(OwnedVal;map 条目按迭代序捕获重放=Fx 布局跨线程
    保序)· spawn0..4 arity trampoline(免 MIR wrapper:捕获全 join→
    Dyn + dyn_rets)· isolate 虚拟槽(goroutine cell 写线程私有)·
    select spin-poll(closed recv=nil binding/closed send=raise)。
    **覆盖率 39→41/51**(concurrency_demo/select)。
  - ✅ **I 完成(模块白名单增量,commit `b90c6c1`)**:lkrt encoding.rs
    (json/yaml/toml 与 VM de.rs 同 crate 同规则,对象过 vm_mirror 两段
    重放保键序;解析错误→raise 可 catch)· stream 转发(全纯语料 eager
    ≡ lazy,复用 iter 通路)· tcp/socket/bytes 白名单绑定(ABI 早已在
    net.rs)。**覆盖率 41→47/51**(json_demo/json_process/yaml_toml/
    config_parser/stream_demo/tcp_demo,两项留档担忧均解除)。
  - **再修进行中:J(trait 静态 devirtualize)**。剩余 4 例:
    struct_trait/trait_impl(J1)· macros.lk(整对象插值 display,
    J 后评估)· unsupported.lk(留档合集)
- GetGlobal 13(try$call/并发/模块白名单/trait)= 再修阶段 a-d,未启
- **✅ 裁决不做**:callable trait 反转 · 真机/QEMU demo · 细粒度 feature 拆分。
- **可选后续**:native raise 前缀统一(catch 到的 native 错误带 "native ... failed:" 前缀,
  error() 一等值无)· goroutine 泄漏之外的死锁检测。

## 护栏 & 续接
全量 tests 0 失败 / clippy 0 / fmt 0 / no_std 0/0 / GC-stress 全绿 / bench 1.021x(基线内)/
差分门禁全过。**下一会话首选:M4.2 Dyn 装箱值地基**(MIR Ty::Dyn + lkrt tagged value,
注意 display/错误信息 VM-exact 逐字节 + semantics.md 已裁决混合 map display 不进子集)。
