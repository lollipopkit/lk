# LK Standard Library

LK stdlib modules are Rust crates registered through `ModuleProvider`. Each
top-level module has its own crate under `stdlib/crates/`; parent namespaces
such as `io`, `net`, and `encoding` expose child namespaces through runtime
exports.

## Module Boundaries

- `fs` owns path-level filesystem operations such as `read`, `write`,
  `metadata`, `read_dir`, and removal/rename/copy helpers.
- `io.file` owns opened `File` resources: `open`, `create`, `read`,
  `read_to_string`, `write`, `flush`, and `close`.
- `os` is intentionally narrow: platform and clock helpers only.
- `env`, `path`, and `process` split out environment lookup, path manipulation,
  and process execution/state.
- `encoding` is a parent namespace for data formats and byte/text encodings.
  Every codec under it is a **pair**: `json`/`yaml`/`toml` have `parse` and
  `stringify`, `base64`/`hex` have `encode` and `decode`, `url` has
  `encode_component` and `decode_component`. A parser without its serializer is
  half an operation — `stringify` was missing, so a script could read a config
  and change it but not write it back.
- Concurrency is Go-shaped (see `docs/concurrency.md`): the `go` statement /
  `spawn` global start goroutines, `chan` owns channel operations — the whole
  surface, blocking (`send`/`recv`) as well as polling (`try_send`/`try_recv`),
  because `use chan;` shadows the `chan` global and a module that is only half
  there sends the reader back to unqualified globals — and
  `task` owns task management (`await`, `try_await`, `join_all` — which takes
  either the tasks or one list of them — and `sleep`).
  Failures raise (v2 error model) — there are no `[ok, value]` pairs.

## Method Naming

One operation, one name, across every container. The rules, and the reason each
exists — they are what a new container type should be checked against:

| 操作 | 名字 | 谁有 |
|---|---|---|
| 成员 | `contains(value)` | List / Slice / Bytes / Str / Set |
| 键成员 | `has(key)` | Map |
| 位置 | `index_of(needle)`,找不到给 nil | List / Slice / Bytes / Str |
| 读一个 | `get(index)`,越界给 nil | List / Slice / Bytes / Str / Map |
| 窗口 | `slice(start[, end])`,**起止**不是起点+长度 | List / Slice / Bytes / Str |
| 前/后 n 个 | `take(n)` / `skip(n)` | List / Slice / Bytes / Str |
| 两端 | `first()` / `last()` | List / Slice / Bytes / Str |
| 删一个 | `delete(key)` | Map / Set |
| 清空 | `clear()`,原地,答容器本身 | List / Map / Set |

**列表的可变方法一律原地改**,答复只有两种:改完的**列表本身**(所以
`xs.push(1).push(2)` 能链),或者**被取出来的那个元素**。

| 方法 | 答复 |
|---|---|
| `push(v)` / `insert(i, v)` | 列表本身 |
| `pop()` / `remove_at(i)` | 被取出的元素(空列表 `pop()` 给 nil) |
| `set(i, v)` / `map.set(k, v)` / `clear()` | 容器本身 |
| `last()` / `first()` / `get(i)` | 只读,不改列表 |

`Set` 的 `add` / `delete` 是**有理由的例外**:集合没有"另一个值"可以
交回(你交进去的就是那个值),能说的只有"是不是新的 / 在不在",所以
它们答 Bool。

曾经这里是三套约定:`push`/`set` 原地改,`insert` 复制一份返回新列表,
`remove_at` 复制一份返回 `[新列表, 旧值]` 二元组(全语言唯一这个形状,
而那个"新列表"没人持有),`pop` 则是 `last` 的逐字重复、根本不弹出。
于是"加一个元素会不会改变这个列表"有两个相反的答案。

`has` 不是 `contains` 的同义词:对 map 来说 "contains" 说不清问的是键
还是值,所以键成员单独一个名字。这是有理由的区分,不是历史遗留。

`slice` 取**起止**是硬规则。曾经 `String` 只有 `substring(start, length)`,
于是 `xs.slice(1, 3)` 和 `s.substring(1, 3)` 从同样的数字里切出不同的
窗口 —— 同形的调用,不同的语义,是陷阱不是特性。`substring` 与 `find` 两
种拼写(方法与模块函数)都已删除,统一为 `slice` 与 `index_of`;模块的
`string.index_of` 多一个可选的起始位置,那是方法形式没地方放的东西。

`string` 模块的每个成员都是**方法的拼写**,而不是第二份实现:模块函数体
就一句 `forward("name", …)`,把第一个实参当 receiver 交给
`core_methods` 的同名臂。这条规则从 2026-07-31 起是完整的 —— 在那之前
`capitalize`/`title`/`count`/`strip`/`strip_prefix`/`strip_suffix`/
`pad_left`/`pad_right`/`format` 九个只有模块拼写,`get`/`first`/`last`/
`take`/`skip`/`bytes` 六个只有方法拼写,两边各写各的地方就是漂移的来源
(`count("")` 一边按字节数一边按字符数,差了两倍)。

`string.char_at` 因此改叫 `string.get`:元素访问在每个序列载体上都拼作
`get`(`xs.get(i)`、`bytes.get(b, i)`、`s.get(i)`),第三个名字也意味着
第三套规矩 —— `char_at` 拒绝负数,而 `s[-1]`、`s.get(-1)` 和原生的
`str.char_at` 符号都从末尾往回数。`byte_at` 保留原名,因为它答的是**字节**,
不是元素。

`bytes` 模块同样是纯转发(2026-07-31 补齐):`len`/`is_empty`/`get`/`slice`/
`to_list` 曾经模块和方法各一份实现,而 `slice` 已经漂了 —— `bytes.slice(b, 2, 1)`
报错,`b.slice(2, 1)` 答 `Bytes([])`。现在只有方法侧那一份,答案是截断的那个
(和 `"abcde".slice(-1, -3)`、`xs.slice(-1, -3)` 一致)。`to_string_utf8`/
`to_string_lossy`/`concat` 补上了方法拼写,`contains`/`index_of`/`first`/`last`/
`sum`/`min`/`max`/`take`/`skip` 补上了模块拼写;带回调的 `map`/`filter`/`reduce`
不进 `bytes` 模块,它们的模块拼写在 `iter` 里(`iter.map(b, f)` 本来就能用)。

两个构造函数是"方法名和成员名不一样"的仅有情况,写下来免得被当成疏漏:
`bytes.from_string(s)` **就是** `s.bytes()`,`bytes.from_list(xs)` 就是新加的
`xs.to_bytes()` —— receiver 是 String / List,方法自然长在那边。

`string.to_int` / `string.to_float` 是这条规则的例外,且是有意的:它们收
`String | Number | Bool`,第一个参数不是 String,所以它们是**转换函数**而
不是字符串方法,没有 receiver-first 的方法拼写。

`Set.has` 也已删:它是 `contains` 的纯别名。`Map.has` 留着 —— 见上面
那条,它问的是键,不是同义词。`bytes.eq(a, b)` 同样已删:它逐字节就是
`a == b`,而运算符不需要一个模块函数替身。

方法的**元数以 `core/src/typ/builtin_method_sig.rs` 的声明为准**:
分发前按它校验,所以实现里再写一份 arity 守卫是够不到的。三次漂移
(`bytes.slice`、`map.get`、`str.slice`)都是因为声明和实现各写各的。

## 文本 → 数字

`string.to_int(value[, base])` 与 `string.to_float(value)` 是把 **String** 读成
数字的地方,也顺带做数字之间的转换。

在这之前语言里**没有**这条路:两个函数都只收 `Number | Bool`,给个 `"42"`
直接类型报错;`"42".to_int()` 不存在;全局也没有 `int()`/`float()`。也就是说
读一行配置、切一段 CSV、取一个命令行参数,到"变成数字"这步全是死路 —— 而
唯一看起来像答案的名字明确拒绝字符串。

两种失败,分得很清楚:

- **文本不是数字 → `nil`。**「这行是不是数字」问的是输入,不是程序错误,
  所以用值回答,配 `??` 或 `!` 用,和 `index_of` 一个形状。
- **Float 没有对应的 Int → raise。** NaN、无穷、超出 `i64` 范围都是程序错误。
  Rust 的 `as` 会给 `0` 或 `i64::MAX` —— 一个装成正确答案的错误答案。

首尾空白会被 trim:从文件读的一行带着换行,`"42\n"` 和 `"42"` 在任何读者
眼里是同一个答案。`base` 取 2–36,符号写在前面(`to_int("-ff", 16)`)。
`to_float` 认 `"nan"` / `"inf"` / `"-inf"`,那是 Float 有而 Int 没有的值。

## `datetime.format` / `datetime.parse` 是一对

`parse` 接受 **`format` 能写出来的一切**:完整日期时间、只有日期、只有时间。
少的那一半按 `format` 丢掉时的默认补 —— 只有日期 = 当天 UTC 零点,只有时间 =
epoch 那天的那个时刻。

此前 `parse` 只试 `NaiveDateTime`(必须同时有日期和时间),于是这一对**不能
往返**:`format(t, "%Y-%m-%d")` 给出 `1970-01-02`,拿同一个 format 串 parse 回去
报 "input is not enough for unique date and time" —— 一句在讲 chrono 自己解析器
的内部要求,而程序从没提过 chrono。format 串是调用方对**两侧文本**的描述,两个
方向必须对它的含义达成一致。

不匹配时的报错现在说 `` `zz` does not match the format `%Y-%m-%d` ``。

## 数值哈希是 i64 位模式

`hash.crc32` / `hash.fnv64` 返回 `Int`,而 `Int` 是 i64:`fnv64("")` 的 FNV
offset basis 是 `0xcbf29ce484222325`,超过 `i64::MAX`,所以看到的是负数。这与
语言"Int 溢出回绕"的规则一致(`docs/semantics.md`),不是缺陷 —— 但拿它做桶
下标要先取绝对值或按位掩码。要文本形式的哈希用 `sha256` / `sha1`,它们返回
十六进制串。

## Common Modules

- `hash`: `sha256`, `sha1`, `crc32`, `fnv64`.
- `regex`: match, find, captures, replace, and split helpers.
- `random`: integers, floats, booleans, bytes, list choice, and list shuffle.
- `uuid`: UUID v4 generation and validation.
- `http`: synchronous client returning `{status, headers, body}` maps.

## Rust Module Exports

Stdlib modules use the derive/attribute macro pair from `lk_stdlib_common`.
Put `#[stdlib_export]` on each exported function so runtime exports, catalog
metadata, and LSP hover stay next to the implementation:

```rust
#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "env", docs = "Environment variable helpers")]
pub struct EnvModule;

#[lk_stdlib_common::stdlib_exports]
impl EnvModule {
    #[stdlib_export(
        name = "get",
        params(key: String),
        returns = String?,
        docs = "Returns an environment variable, or nil if it is not set."
    )]
    fn get(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        /* native implementation */
    }
}
```

The macro generates `ModuleProvider`, `register`, `metadata`, runtime exports,
and LSP hover metadata from the exported methods. For runtime builtins such as
`task.spawn` or `time.sleep`, add `runtime_builtins = true`:

```rust
#[lk_stdlib_common::stdlib_exports(module = "time", runtime_builtins = true)]
impl TimeModule {
    #[stdlib_export(params(ms: Int | Float), returns = Nil)]
    fn sleep(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        /* native implementation */
    }
}
```

Simple fixed-arity functions can use the ergonomic ABI. The macro emits a
zero-allocation wrapper that checks arity once and passes indexed values:

```rust
#[stdlib_export(params(source: String), returns = Slice)]
fn from_string(source: RuntimeVal, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    /* native implementation */
}
```

Variadic, named-argument, or full-state functions should keep the raw
`NativeArgs` ABI. Nested namespace modules such as `io.std`, `net.tcp`, and
`encoding.json` use the same per-function exports. Parent namespaces declare
children on `#[stdlib_exports]` so the macro wires the runtime map and registers
child metadata:

```rust
#[lk_stdlib_common::stdlib_exports(
    children(json = JsonModule, yaml = YamlModule, toml = TomlModule)
)]
impl EncodingModule {}
```

For named parameters, list accepted names explicitly and keep semantic
validation in the native implementation:

```rust
#[stdlib_export(
    params(value: Int, min?: Int = 0, max?: Int = 100),
    named(min, max),
    returns = Int
)]
fn clamp(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    /* native implementation */
}
```

## Named Parameters

A parameter is declared `named(...)` when its **position is not enough to say
what it means**. Three shapes qualify, and the rule is mechanical enough that
`every_ambiguous_parameter_is_named` enforces the first one:

1. **Two or more parameters of the same type**, past the first. Nothing at the
   call site distinguishes them, so swapping them is silent — the program keeps
   running and answers something else.
2. **A boolean switch.** `true` at a call site says nothing about which
   behaviour it selects.
3. **An optional parameter whose absence changes what happens**, rather than
   merely supplying a default.

The first parameter — the thing being operated on — is never named.
`string.len(text: s)` is noise; the subject of a call is what a call is about,
and its position says so.

Named parameters are **optional at the call site**: `math.clamp(5, 1, 3)` and
`math.clamp(5, min: 1, max: 3)` are both accepted, so declaring them breaks
nothing and only gives the caller a way to be explicit.

### Why the rule earns its keep

These operations are siblings, and the same `(2, 3)` used to mean two things:

```lk
"abcdef".substring(2, 3)        // "cde"  — the third argument was a *length*
[1,2,3,4,5,6].slice(2, 3)       // [3]    — the third argument is an *end*
bytes.slice(b, 2, 3)            // "c"    — the third argument is an *end*
```

No amount of care at the call site distinguished `substring(s, 2, 3)` from
`slice(xs, 2, 3)`; only the declaration knew. `substring` is gone — every
window in the language now takes a start and an *end* — but the rule stands for
the arguments that remain unlike each other: `string.slice(s, start: 2, end: 5)`
and `bytes.slice(b, 0, end: 2)` cannot be misread, and the names cost nothing.

## Examples

```lk
use fs;
use bytes;
use { file } from io;
use { json, base64 } from encoding;

let raw = fs.read("config.json");
let text = bytes.to_string_utf8(raw);
let cfg = json.parse(text);

let out = file.open("out.txt", "write");
file.write(out, base64.encode(text));
file.close(out);
```

Top-level `json`, `yaml`, and `toml` modules are removed. Use
`use { json, yaml, toml } from encoding;` instead.
