# 待完成

本轮 review + 修复过程中确认下来的开口项。每条都带复现或证据;没有复现过的会写明。
做完一条就删掉它,不要在这里留"已完成"。

## 需要先拍板的

### try/catch 变成真语句 —— AOT 侧欠账

`try`/`catch` 已经从解析期去糖(`try$call(|| body)` + 解构 `let`)改成真语句
`Stmt::Try`,VM 编译器发射 `TryBegin`/`TryEnd`。VM 侧完成并全绿。

**AOT lowering 没有这两个 opcode 的处理**。它优雅降级成 `Unsupported`(不会崩),
后果是:

- `AOT_COVERAGE_REQUIRE_FULL=1 scripts/aot_coverage.sh` 从 51/51 掉到 **48/51**;
- `cli/tests/clif_differential_test.rs::clif_differential_try_catch` 红(它用
  `LK_AOT_NO_FALLBACK=1` 强制纯原生)。

**正确性不受影响,已验证**:`examples/syntax/{try_catch,error_unwrap,error_model_edges}.lk`
降级后输出与 VM 逐字节一致。但降级是**整程序** Tier 0
(`self-contained; embeds the VM`)而不是部分降级 —— 这三个例子的 try 在入口函数
里,而 hybrid 桥不桥接入口。try 在非入口函数里的程序才会走 hybrid 部分降级。

两条路,选一条:

1. **先落 VM 侧,把回归写响**(建议):`AOT_COVERAGE_ALLOW` 加那 3 个路径 + 理由 +
   写清 51/51 → 48/51;`clif_differential_try_catch` **不要删**,改成允许 fallback
   的 VM/native 等价测试并改名(它真正保的是等价,不是"走过 Cranelift");
   `docs/aot/` 记下外联设计。
2. **先补 AOT,两边一起落**:代价是整件事里最大的一块,且在它做完之前 try/catch
   一直是坏的(见下面"编译器拒绝普通代码")。

补 AOT 要做的:在 MIR lowering 里把保护区**外联**成函数(Cranelift 没有异常、
lkrt 靠 longjmp、setjmp 必须待在不会返回的帧里),外联函数返回三态(正常值 /
raise / 外层要 return),live-in 变参数、region 内写且 region 后仍活的值传回来。

## 开着的 bug

### 类型检查器不看带标注的局部做返回类型推导

```lk
fn f() -> Int { let r: Int = 0; r = 7; return r; }   // expected Int, got 'T0
```

不带 try/catch 也复现,`lk check` / VM / AOT 三条路都中。用户直接撞得到,且和上面
的分支决策无关 —— 建议优先做这条。

(注:之前一度把它记成 try/catch 的 bug,是错的。)

### `try { return x / 0; }` 在函数里过不了 Cranelift verifier

`Error: Cranelift codegen failed: Module("Compilation error: Verifier errors")`。
既存 —— `LK_AOT_NO_OPT=1` 与改动前的 baseline 都复现。从 `LK_AOT_DUMP_MIR=1` 看得
很清楚:外联出来的 `f1` 签名是 `-> i64`,但 `bb9` 上有一条裸 `ret`,去糖丢掉了
"body 返回了"这个情况。

`feat/try-catch-statement` 上**未复验**(那条分支上 try 不进 AOT)。真语句化 + AOT
外联做完之后应该一起消失,做完要回来确认。

### scope drop 的跨块限制

实测 `for i in 0..200000 { let parts = s.split("-"); if parts[0] == "alpha" {…} }`:
native 43.6 MB vs VM 22.7 MB,而 `LK_AOT_OPT_STATS=1` 报 `scope drops = 0` ——
`if` 把块切开,句柄跨块,连容器都没释放。

该做**跨块 liveness**。`aot/mir/src/opt.rs` 文档里"语料上 18 个循环分配中 3 个块
内、0 个跨块但不逃逸"那句没覆盖这个形状,先用 `count_loop_allocations` 的第三个
计数器在这个形状上复测。

### `patch_branch` 用 `as i16` 截断跳转偏移

`core/src/vm/compiler/builder.rs`。超出 signed-bx 范围的分支目标会静默回绕成跳到
别处,而不是编译失败。同文件新加的 `patch_try_begin` 已经用 checked 转换,旧的那个
没动。

### builtin 类型的 impl 跨模块撞车

`impl D for Int` 这类 impl 全部落在 `TypeScope::builtin()` 一个 scope 里,最后注册
的赢。代码里有 `TODO(coherence)`。要一条 orphan rule 才能拒绝重叠 —— 是语言决策,
不是分派修复。

### `resolve_file_path` 的 `..` 不做真包含检查

`core/src/vm/resolver.rs`,代码里有 `TODO(security)`。`starts_with(root)` 只是归一化
偏好,逃出 root 的候选照样返回。要先定"`..` import 允许逃到哪个 root"。

### native `println` 打印 `List<str>` 不带引号

与 VM 输出不一致(`[a-b,a-b]` vs `["a-b","a-b"]`)。既存,main 上同样。新加的严格
native 差分 CI 的语料没覆盖到。**本人未复验**,来自第一轮 review 报告。

### ASan 下 hybrid 程序的 fuzz 差分失败

`aot_fuzz_differential_test` 在 `LK_NATIVE_SANITIZE` 下报 "AOT compile failed
without a graceful Unsupported reason"。既存(stash 掉改动同样复现):ASan 版 lkrt
与未插桩的 lk-api staticlib 混链,`scripts/build_lkrt_asan.sh` 自己的注释警告过这种
ABI 混用。不是 PR 门禁,优先级低。

## 已记档的覆盖上限(不是 bug)

这些是有意的保守取舍,写在这里只为免得被重新"发现"。

- **跨模块 impl 分派**在方法写 module global、或子树里有
  `Call`/`CallNamed`/`CallMethodK`(静态解析不了的间接调用)时拒绝。收窄要用现成的
  `PerfCallTargetKind` 调用点事实:证明只到 native 的调用不可能 `SetGlobal`。见
  `docs/vm-cross-module-dispatch.md`。
- **`handle_release_deep` 只覆盖"元素从未被读"的容器**;读过元素的要元素级
  liveness。per-iteration region 走不通的原因:`lkrt` 的 `owned_strings` /
  `owned_containers` 无序,mark/reclaim 要先把它改成有序日志 + tombstone。
