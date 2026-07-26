# 待完成

本轮 review + 修复过程中确认下来的开口项,**按优先级排序**。每条都带复现或证据;
没有复现过的会写明。做完一条就删掉它,不要在这里留"已完成"。

排序依据:正确性 > 性能;普通代码撞得到 > 需要刻意构造;CI 正红 > 潜在;
改动小且能消掉一整类问题的往前放。

---

## P0 · 先花几分钟核实,结论会改变后面的排序

三条都是"验证成本远低于修复成本",而且结论决定它们自己的优先级。

### P0.1 native `println` 打印 `List<str>` 不带引号

与 VM 输出不一致(`[a-b,a-b]` vs `["a-b","a-b"]`)。**若为真,这是 VM/native 的错
答案分歧**,不是排版问题 —— 整套差分设施就是为了防这个,而新加的严格 native 差分
语料没覆盖到它。

既存,main 上同样。**本人未复验**,来自第一轮 review 报告。先复现:为真则升到 P1
之前并补一条差分用例;为假则删掉本条。

### P0.2 `try { return x / 0; }` 是否还过不了 Cranelift verifier

`Error: Cranelift codegen failed: Module("Compilation error: Verifier errors")`。
既存 —— `LK_AOT_NO_OPT=1` 与改动前的 baseline 都复现。`LK_AOT_DUMP_MIR=1` 看得很
清楚:外联出来的 `f1` 签名是 `-> i64`,但 `bb9` 上有一条裸 `ret`,去糖丢掉了
"body 返回了"这个情况。

真语句化之后**未复验**(现在 try 不进 AOT)。跑一次就知道:已消失就删掉本条,
否则它是 P3 的一部分。

### P0.3 ASan 下 hybrid 程序的 fuzz 差分是否已修

`aot_fuzz_differential_test` 在 `LK_NATIVE_SANITIZE` 下报 "AOT compile failed
without a graceful Unsupported reason"。这条观察**早于** `fix: hybrid 标记之后重新
收敛签名`,而那个提交修的正是"codegen 硬失败而非优雅降级",所以可能已一并消失。

若仍在:原因大概是 ASan 版 lkrt 与未插桩的 lk-api staticlib 混链,
`scripts/build_lkrt_asan.sh` 的注释警告过这种 ABI 混用;不是 PR 门禁,降到 P5。

---

## P1 · 正确性,可复现,互不依赖

### P1.1 类型检查器不看带标注的局部做返回类型推导

```lk
fn f() -> Int { let r: Int = 0; r = 7; return r; }   // expected Int, got 'T0
```

不带 try/catch 也复现,`lk check` / VM / AOT 三条路都中。**列表里唯一"写正常代码就
撞得到、且 `lk check` 直接失败"的一条**,且和 AOT 那堆决策完全独立。

(注:之前一度把它记成 try/catch 的 bug,是错的。)

### P1.2 `patch_branch` 用 `as i16` 截断跳转偏移

`core/src/vm/compiler/builder.rs`。超出 signed-bx 范围的分支目标会静默回绕成跳到别
处,而不是编译失败。潜在 miscompile,但**改动是三行**(换 checked 转换 + 一条明确
错误),能消掉一整类问题 —— 同文件新加的 `patch_try_begin` 已经这么做了,照抄。

---

## P2 · CI 正红,但先要一个能复现的环境

### P2.1 ASan 下 `lkrt_lklist_i64_filter_fn` 报 stack-use-after-scope

main 的 CI("differential corpora with an ASan-instrumented lkrt" job)在
`aot_differential_test::differential_builtins` /
`builtins/list_hof_map_filter_reduce` 上失败:

```
AddressSanitizer: stack-use-after-scope
  #0 <Copied<Iter<*const i8>> as Iterator>::size_hint
  #1 <Vec<i64> as SpecFromIterNested<…, lkrt_lklist_i64_filter_fn::{closure#0}>>::from_iter
  #2 lkrt_lklist_i64_filter_fn
```

`lkrt_lklist_i64_filter_fn` 在 `values.iter().copied().filter(|&v| p(v)).collect()`
里跨着**回调进生成代码**持有源列表的 `&[i64]` 借用。两种可能,先分清再定改法:

1. 回调改动了同一个列表(Vec 重分配 → 迭代器悬空)—— **真的内存不安全**,是本列表
   里可能最严重的一条;
2. 回调 raise 时 longjmp 越过 Rust 作用域,ASan 的 poison 没被
   `__asan_handle_no_return` 清掉 —— **误报**,但同样红。

CLAUDE.md 里 lkrt 那条纪律("绝不在持有 lock guard / RefCell borrow 时调用可 raise
的函数")对**切片借用**同样适用,只是没写进去 —— 很可能就是根因。

**本地用仓库自己的 `scripts/build_lkrt_asan.sh` 构出 ASan lkrt 后不复现**,所以排在
P1 之后:得先能复现。

---

## P3 · 最大的一件,做完能收回一条硬门禁

### P3.1 try/catch 的 AOT 保护区外联

`try`/`catch` 已经是真语句(`Stmt::Try` → `TryBegin`/`TryEnd`),VM 侧完成。
**AOT lowering 没有这两个 opcode 的处理**,优雅降级成 `Unsupported`。

已经按"先落 VM 侧、把回归写响"处理:

- `AOT_COVERAGE_REQUIRE_FULL=1` **48/51**,三个 `examples/syntax/{try_catch,
  error_unwrap,error_model_edges}.lk` 列在 `.github/workflows/check.yml` 的
  `AOT_COVERAGE_ALLOW` 里,带理由;
- `clif_differential_try_catch` 改名 `try_catch_differential`,允许降级、仍然断言
  VM/native 输出一致 —— 保住的是等价,失去的是"走过 Cranelift";
- 设计与欠账记在 `docs/aot/tier1-hybrid.md` 末尾。

降级不影响正确性(三个例子输出与 VM 逐字节一致),但降级是**整程序** Tier 0 而不是
部分降级:它们的 try 在入口函数里,hybrid 桥不桥接入口。

要做的:在 MIR lowering 里把保护区**外联**成函数(Cranelift 没有异常、lkrt 靠
longjmp、setjmp 必须待在不会返回的帧里),外联函数返回三态(正常值 / raise /
**外层函数要 return**),live-in 变参数、region 内写且 region 后仍活的值传回来。

**补齐之后要做的清理**:删掉 check.yml 里那三条 allow、把 `try_catch_differential`
改回 `NativePath::PureCranelift`、复验 P0.2、删掉本条。

---

## P4 · 先要一个语言决策,决定完实现很小

### P4.1 `resolve_file_path` 的 `..` 不做真包含检查

`core/src/vm/resolver.rs`,代码里有 `TODO(security)`。`starts_with(root)` 只是归一化
偏好,逃出 root 的候选照样返回。要先定"`..` import 允许逃到哪个 root"—— 定完之后
实现是几行。

### P4.2 builtin 类型的 impl 跨模块撞车

`impl D for Int` 这类 impl 全部落在 `TypeScope::builtin()` 一个 scope 里,最后注册
的赢(静默的错答案)。代码里有 `TODO(coherence)`。要一条 orphan rule 才能拒绝重叠
—— 是语言决策,不是分派修复。

---

## P5 · 只影响内存/性能

### P5.1 scope drop 的跨块限制

实测 `for i in 0..200000 { let parts = s.split("-"); if parts[0] == "alpha" {…} }`:
native 43.6 MB vs VM 22.7 MB,而 `LK_AOT_OPT_STATS=1` 报 `scope drops = 0` ——
`if` 把块切开,句柄跨块,连容器都没释放。

该做**跨块 liveness**。`aot/mir/src/opt.rs` 文档里"语料上 18 个循环分配中 3 个块
内、0 个跨块但不逃逸"那句没覆盖这个形状,先用 `count_loop_allocations` 的第三个
计数器在这个形状上复测。

---

## 已记档的覆盖上限(不是 bug,不排优先级)

这些是有意的保守取舍,写在这里只为免得被重新"发现"。

- **跨模块 impl 分派**在方法写 module global、或子树里有
  `Call`/`CallNamed`/`CallMethodK`(静态解析不了的间接调用)时拒绝。收窄要用现成的
  `PerfCallTargetKind` 调用点事实:证明只到 native 的调用不可能 `SetGlobal`。见
  `docs/vm-cross-module-dispatch.md`。
- **`handle_release_deep` 只覆盖"元素从未被读"的容器**;读过元素的要元素级
  liveness。per-iteration region 走不通的原因:`lkrt` 的 `owned_strings` /
  `owned_containers` 无序,mark/reclaim 要先把它改成有序日志 + tombstone。
