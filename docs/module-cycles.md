# `core` 里的依赖环

`core` 是一个 crate,装着整个前端加 VM(~57k 行,`vm/` 占 53%)。**拆不开的
原因是依赖环,不是体积** —— 每一个环都是下层反过来伸手够上层。

先修环,再谈拆 crate:先拆只会把环搬到 crate 层,而 Cargo 在那一层直接拒绝。

## 还剩的环

| 环 | 窄边 | 是什么 |
| --- | --- | --- |
| `val` ↔ `vm` | `val/runtime_model.rs` | `CallableValue` 内嵌 `vm::NativeFunction` 与 `Arc<vm::RuntimeCallable>` |
| `rt` ↔ `vm` | `rt/runtime.rs` | `RuntimePayload` 里三处 `vm::copy_runtime_value` |

**这两个是同一个问题。** `RuntimePayload` 伸进 VM 的唯一理由是
`copy_runtime_value` —— 在两个 `HeapStore` 之间做深拷贝,一个**值**操作;它之所以
住在 `vm/exec/runtime_callable.rs`,只因为拷贝 `CallableValue::Runtime` 需要
`Arc<Module>` 和 `Arc<Mutex<RuntimeModuleState>>`。修好 `val` ↔ `vm`,
`rt` ↔ `vm` 自己就掉下来了;单独去修 `rt`,那个函数没有地方可放。

值类型本身**不是**问题:`RuntimeVal` 是 16 字节 `Copy` 枚举,标量加 `HeapRef`,
不提任何 VM 类型。环完全走 `CallableValue` 这个堆值。要断它,得在 `val` 一侧
定义一个"VM 能调用的东西"的 trait —— 而这个 trait 要带的不止 `call`:回收器要
走一个 runtime callable 的捕获和跨模块状态,`copy_runtime_value` 还要问它属于
哪个模块(决定 `Reject` 还是 `SameModule`)。那是把值/VM 边界重新设计一遍,
不是搬一下代码。

## 已经修掉的(5 → 2)

- `token_lexeme` 移进 `token`:它是 `Token` 的属性,住在 `macro_system` 里害得
  `stmt` 仅仅为了打印一个 token 就依赖它。
- `Program::execute*` 变成 `vm::ProgramExec` 扩展 trait:挂在 AST 上的固有方法
  纯为调用点方便,却逼出 `stmt` → `vm`。
- `ModuleResolver` + `execute_imports` 移进 `vm::resolver`:加载一个模块意味着
  解析、执行、绑定导出 —— 那是执行,不是语法。
- `macro_system` ↔ `package`(2026-07-30):宏导入解析改成从
  `MacroExpandOptions` 取一个 `PackageMacroModuleResolver` 函数指针,不再直接调
  `PackageGraph::discover`;`syntax::ParseOptions::default()` 装入
  `package::macro_module_root`。边现在是 `syntax → package → macro_system`,单向。

  依赖注入的典型失败是"忘了装默认值,于是功能静默消失"。这里由
  `package_named_macro_import_expands_and_is_compile_time_only` 兜底:把默认值
  去掉,它就红。

`stmt` 现在只依赖 `compat` 和 `token`。
