# 待完成

本轮 review + 修复过程中确认下来的开口项,**按优先级排序**。每条都带复现或证据;
没有复现过的会写明。做完一条就删掉它,不要在这里留"已完成"。

排序依据:正确性 > 性能;普通代码撞得到 > 需要刻意构造;CI 正红 > 潜在;
改动小且能消掉一整类问题的往前放。

---

## P1 · 小,已定性

### P1.1 同质列表字面量对不上 `Tuple` 标注

```lk
fn f() -> Tuple<Int, Int> { return [1, 2]; }   // expected Tuple<Int, Int>, got List<Int>
fn g() -> Tuple<Bool, String> { return [true, "x"]; }   // 通过
```

**同一种语法**按元素同质/异质推成 `List<Int>` 或 `Tuple<Bool, String>`,而只有后者
能赋给 `Tuple` 标注。两条候选:

1. 让 `is_assignable(List<T>, Tuple<T, …, T>)` 成立 —— 小,但丢掉 arity 保证
   (长度 3 的 `List<Int>` 也能通过 `Tuple<Int, Int>`);
2. 列表字面量按**期望类型**推导(双向检查),保留 arity —— 正解,但要把期望类型
   传到字面量处。

### P1.2 两个 anonymous 模块之间的 builtin impl 冲突查不出来

`claim_builtin_impl` 按声明模块的 `TypeScope` 判定归属,而 `TypeScope::anonymous()`
两两相等,所以两个匿名模块给同一个 builtin 实现同一个 trait 时,后者仍然静默覆盖
前者 —— 正是这条检查要消掉的 import 顺序依赖,只是换到了内存/REPL 路径上。

**目前在生产路径上不可达**:一次运行里只有入口程序是匿名的,
`resolve_source_runtime`(唯一另一个匿名来源)没有生产调用方,只有测试用。加一个
eval API 就会变得可达。

要修得让匿名 scope 彼此可区分(现在 `type_info.rs` 的文档明确写着它"只靠是本次运行
唯一的匿名 scope 来区分"),那会牵动 artifact 往返与相等性 —— 不是一处小改。

## P2 · 最大的一件,做完能收回一条硬门禁

### P2 try/catch 的 AOT 保护区外联

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
改回 `NativePath::PureCranelift`、删掉本条。

---

## P3 · 只影响内存/性能

### P3.1 scope drop 的跨块限制

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
