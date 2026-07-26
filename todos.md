# 待完成

本轮 review + 修复过程中确认下来的开口项,**按优先级排序**。每条都带复现或证据;
没有复现过的会写明。做完一条就删掉它,不要在这里留"已完成"。

排序依据:正确性 > 性能;普通代码撞得到 > 需要刻意构造;CI 正红 > 潜在;
改动小且能消掉一整类问题的往前放。

---

## P1 · 正确性,可复现

### P1.1 解构 `let` 的元素类型没有分发

`let [ok, v] = f()` 现在把 `v` 绑成 `Any`,因为原来的"每个名字都绑整个右值的类型"
明显是错的(`v` 会被绑成整个 tuple)。**正解是把模式分发到类型上**:tuple 按位置、
`List<T>` 按元素、union 逐成员分发。

`Any` 是诚实的占位(不会凭空拒绝),但也就检查不出解构元素的类型错误。做完之后
`core/src/stmt/stmt_impl/type_check.rs` 里那段注释要一并删掉。

### P1.2 tuple 返回类型和自身的标注对不上

```lk
fn pick(m: Map<String, String>, k: String) -> Tuple<Bool, String> {
  if (m.get(k) == nil) { return [false, "missing"]; }
  return [true, "found"];
}
// Return type mismatch in function 'pick': expected Tuple<Bool, String>, got Tuple<Bool, String>
```

**显示完全一致却 unify 失败**,所以问题在 `Tuple` 的结构比较而不是显示。既存(和
P1.1 的修复无关,是写它的回归测试时撞到的)。带标注的 tuple 返回目前写不出来。

---

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

## P3 · 先要一个语言决策,决定完实现很小

### P3.1 `resolve_file_path` 的 `..` 不做真包含检查

`core/src/vm/resolver.rs`,代码里有 `TODO(security)`。`starts_with(root)` 只是归一化
偏好,逃出 root 的候选照样返回。要先定"`..` import 允许逃到哪个 root"—— 定完之后
实现是几行。

### P3.2 builtin 类型的 impl 跨模块撞车

`impl D for Int` 这类 impl 全部落在 `TypeScope::builtin()` 一个 scope 里,最后注册
的赢(静默的错答案)。代码里有 `TODO(coherence)`。要一条 orphan rule 才能拒绝重叠
—— 是语言决策,不是分派修复。

---

## P4 · 只影响内存/性能

### P4.1 scope drop 的跨块限制

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
