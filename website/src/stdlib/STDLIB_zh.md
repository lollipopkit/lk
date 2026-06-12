# 标准库概览

LK 标准库按需导入。用 `use` 引入模块后即可调用其函数和常量。

```lk
use math;
use { json } from encoding;

math.sqrt(16)         // 4
json.parse("{\"a\":1}")  // { "a": 1 }
```

String、List、Map、Set 的元方法无需导入，直接通过 `value.method()` 调用。

## math

数学常量与函数。

**常量**：`pi`、`e`、`inf`、`nan`、`max_int`、`min_int`、`max_float`、`epsilon`

**函数**：

| 函数 | 签名 | 说明 |
|------|------|------|
| `abs(x)` | Int/Float → Int/Float | 绝对值 |
| `sqrt(x)` | Float → Float | 平方根 |
| `floor(x)` | Float → Int | 向下取整 |
| `ceil(x)` | Float → Int | 向上取整 |
| `round(x)` | Float → Float | 四舍五入 |
| `min(a, b)` | 同类型 → 同类型 | 最小值 |
| `max(a, b)` | 同类型 → 同类型 | 最大值 |
| `pow(base, exp)` | Float, Float → Float | 幂运算 |
| `exp(x)` | Float → Float | e^x |
| `sin/cos/tan(x)` | Float → Float | 三角函数 |
| `asin/acos/atan(x)` | Float → Float | 反三角函数 |
| `atan2(y, x)` | Float, Float → Float | 四象限反正切 |
| `log/log10/log2(x)` | Float → Float | 对数 |
| `clamp(val, lo, hi)` | 同类型 → 同类型 | 截断到范围 |
| `random()` | → Float | [0, 1) 随机数 |
| `hypot(x, y)` | Float, Float → Float | √(x²+y²) |
| `cbrt(x)` | Float → Float | 立方根 |
| `sinh/cosh/tanh(x)` | Float → Float | 双曲三角函数 |
| `trunc(x)` | Float → Float | 截断小数 |
| `fract(x)` | Float → Float | 小数部分 |
| `sign(x)` | Float → Float | 符号 |
| `to_int(x)` | Float → Int | 转整数 |
| `to_float(x)` | Int → Float | 转浮点 |
| `is_nan(x)` | Float → Bool | 是否 NaN |
| `is_inf(x)` | Float → Bool | 是否无穷 |

```lk
use math;
math.sqrt(2)        // 1.4142135623730951
math.pow(2, 10)     // 1024.0
math.clamp(15, 0, 10) // 10
```

## string

String 元方法，无需导入，直接通过 `value.method()` 调用。

| 方法 | 说明 |
|------|------|
| `len()` | 字符数 |
| `lower()` | 转小写 |
| `upper()` | 转大写 |
| `trim()` | 去除首尾空白 |
| `starts_with(prefix)` | 前缀匹配 |
| `ends_with(suffix)` | 后缀匹配 |
| `contains(sub)` | 包含子串 |
| `replace(old, new)` | 替换子串 |
| `substring(start[, end])` | 截取子串 |
| `split(sep)` | 按分隔符拆分为列表 |
| `join(list)` | 用此字符串连接列表元素 |
| `reverse()` | 反转字符串 |
| `repeat(n)` | 重复 n 次 |
| `chars()` | 拆分为字符列表 |
| `char_at(index)` | 指定位置字符 |
| `byte_at(index)` | 指定位置字节 |
| `find(sub)` | 查找子串位置，未找到返回 nil |
| `is_empty()` | 是否为空 |
| `format(args...)` | 格式化 |

```lk
"Hello, {}!".format("LK")    // "Hello, LK!"
"  hello  ".trim()            // "hello"
"a,b,c".split(",")           // ["a", "b", "c"]
```

## bytes

二进制数据处理。

| 函数 | 说明 |
|------|------|
| `from_list(list)` | 从整数列表创建 |
| `from_string(str)` | 从 UTF-8 字符串创建 |
| `len(bytes)` | 字节长度 |
| `is_empty(bytes)` | 是否为空 |
| `get(bytes, index)` | 指定位置字节 |
| `slice(bytes, start[, end])` | 截取子段 |
| `to_list(bytes)` | 转整数列表 |
| `to_string_utf8(bytes)` | 转 UTF-8 字符串 |
| `to_string_lossy(bytes)` | 转 UTF-8（替换非法字节） |
| `concat(a, b)` | 拼接 |
| `eq(a, b)` | 比较相等 |

```lk
use bytes;
let raw = bytes.from_string("hello");
bytes.len(raw)                  // 5
bytes.to_string_utf8(raw)       // "hello"
bytes.concat(raw, bytes.from_string("!"))
```

## iter

列表工具函数。

| 函数 | 说明 |
|------|------|
| `range([start,] end [, step])` | 生成范围列表 |
| `enumerate(list)` | 带索引遍历 → [[index, item], ...] |
| `zip(list1, list2)` | 拉链配对 |
| `take(list, n)` | 取前 n 个 |
| `skip(list, n)` | 跳过前 n 个 |
| `chain(list1, list2)` | 拼接列表 |
| `flatten(list)` | 展平一层 |
| `unique(list)` | 去重 |
| `chunk(list, size)` | 按大小分块 |
| `map(list, fn)` | 映射 |
| `filter(list, fn)` | 过滤 |
| `reduce(list, init, fn)` | 归约 |

```lk
use iter;
let nums = iter.range(1, 6);
iter.map(nums, |n| n * 2)           // [2, 4, 6, 8, 10]
iter.filter(nums, |n| n % 2 == 0)  // [2, 4]
iter.reduce(nums, 0, |acc, n| acc + n) // 15
```

## stream

懒执行流管道。并发 feature gate 启用后可用。

| 函数 | 说明 |
|------|------|
| `from_list(list)` | 从列表创建流 |
| `range(start, end)` | 从范围创建流 |
| `iterate(seed, fn)` | 迭代器流 |
| `repeat(val)` | 重复值流 |
| `from_channel(ch)` | 从通道创建流 |
| `map(s, fn)` | 映射 |
| `filter(s, fn)` | 过滤 |
| `take(s, n)` | 取前 n 个 |
| `skip(s, n)` | 跳过前 n 个 |
| `chain(a, b)` | 拼接流 |
| `subscribe(s)` | 创建游标 |
| `next(cursor)` | 取下一个值 |
| `collect(stream_or_cursor)` | 收集为列表 |
| `next_block(cursor[, timeout_ms])` | 取下一块 |
| `collect_block(stream_or_cursor[, n][, timeout_ms])` | 收集一块 |

```lk
use stream;
let s = stream.from_list([1, 2, 3, 4, 5]);
let cursor = stream.subscribe(stream.map(s, |n| n * 10));
stream.collect(cursor)  // [10, 20, 30, 40, 50]
```

## datetime

日期时间辅助。

| 函数 | 说明 |
|------|------|
| `now()` | 当前微秒时间戳 |
| `format(secs, fmt)` | 格式化时间戳 |
| `parse(str, fmt)` | 解析时间字符串 |
| `add(secs, delta)` | 时间加法 |
| `sub(secs, delta)` | 时间减法 |
| `day_of_week(secs)` | 星期几 |
| `day_of_year(secs)` | 一年中第几天 |
| `is_weekend(secs)` | 是否周末 |

```lk
use datetime;
let now = datetime.now();
datetime.format(now, "%Y-%m-%d %H:%M:%S")
```

## os

平台信息。

| 函数 | 说明 |
|------|------|
| `hostname()` | 主机名 |
| `arch()` | 架构 |
| `os()` | 操作系统 |
| `clock()` | 进程时钟 |
| `time()` | 当前秒 |
| `epoch()` | Unix 时间戳 |

```lk
use os;
println(os.os());    // e.g. "macos"
println(os.arch());  // e.g. "aarch64"
```

## fs

文件系统操作（基于路径）。

| 函数 | 说明 |
|------|------|
| `read(path)` | 读取为 Bytes |
| `read_to_string(path)` | 读取为 String |
| `write(path, data)` | 写入文件 |
| `append(path, data)` | 追加写入 |
| `exists(path)` | 是否存在 |
| `is_file(path)` | 是否为文件 |
| `is_dir(path)` | 是否为目录 |
| `metadata(path)` | 元数据 |
| `read_dir(path)` | 列出目录内容 |
| `create_dir(path)` | 创建目录 |
| `create_dir_all(path)` | 递归创建目录 |
| `remove_file(path)` | 删除文件 |
| `remove_dir(path)` | 删除空目录 |
| `remove_dir_all(path)` | 递归删除目录 |
| `rename(from, to)` | 重命名 |
| `copy(from, to)` | 复制文件 |
| `canonicalize(path)` | 规范化路径 |
| `temp_dir()` | 临时目录 |

```lk
use fs;
let content = fs.read_to_string("config.json");
fs.write("output.txt", "hello");
```

## path

路径操作。

| 函数 | 说明 |
|------|------|
| `join(parts...)` | 拼接路径 |
| `parent(path)` | 父目录 |
| `file_name(path)` | 文件名 |
| `file_stem(path)` | 文件名（不含扩展名） |
| `extension(path)` | 扩展名 |
| `with_extension(path, ext)` | 替换扩展名 |
| `is_absolute(path)` | 是否绝对路径 |
| `normalize(path)` | 规范化路径 |
| `components(path)` | 路径组件列表 |
| `sep()` | 路径分隔符 |
| `delimiter()` | 环境变量分隔符 |

```lk
use path;
path.join("src", "main.lk")  // "src/main.lk"
path.extension("app.lk")     // "lk"
```

## env

环境变量（只读）。

| 函数 | 说明 |
|------|------|
| `get(key)` | 获取环境变量 |
| `get_or(key, default)` | 获取或默认值 |
| `has(key)` | 是否存在 |
| `vars()` | 全部环境变量 |

```lk
use env;
let home = env.get_or("HOME", "/tmp");
```

## process

进程操作。

| 函数 | 说明 |
|------|------|
| `id()` | 当前进程 ID |
| `cwd()` | 当前工作目录 |
| `set_cwd(path)` | 设置工作目录 |
| `exit(code)` | 退出进程 |
| `status(cmd[, args])` | 运行命令返回退出码 |
| `output(cmd[, args])` | 运行命令返回 `{status, success, stdout, stderr}` |
| `output_string(cmd[, args])` | 同上但 stdout/stderr 为 String |

```lk
use process;
let result = process.output_string("echo", ["hello"]);
println(result.stdout);  // "hello\n"
```

## io

父命名空间。用 `use { std, file } from io` 导入子模块，或通过 `io.std`、`io.file` 访问。

### io.std

标准 I/O。

| 函数 | 说明 |
|------|------|
| `stdin()` | 标准输入 |
| `stdout()` | 标准输出 |
| `stderr()` | 标准错误 |
| `read(reader[, max_bytes])` | 读取为 Bytes |
| `read_to_string(reader)` | 读取为 String |
| `read_line(reader)` | 读取一行 |
| `write(writer, data)` | 写入 |
| `writeln(writer, data)` | 写入并换行 |
| `flush(writer)` | 刷新缓冲 |

### io.file

文件资源 I/O。

| 函数 | 说明 |
|------|------|
| `open(path, mode)` | 打开文件 |
| `create(path)` | 创建文件 |
| `read(file[, max_bytes])` | 读取 |
| `read_to_string(file)` | 读取为 String |
| `read_line(file)` | 读取一行 |
| `write(file, data)` | 写入 |
| `writeln(file, data)` | 写入并换行 |
| `write_all(file, data)` | 全量写入 |
| `flush(file)` | 刷新 |
| `close(file)` | 关闭 |

```lk
use { std, file } from io;
let input = io.std.read_to_string(io.std.stdin());
let f = io.file.open("data.txt", "read");
let content = io.file.read_to_string(f);
io.file.close(f);
```

## net

父命名空间。`use { socket, tcp, udp } from net` 或 `net.socket`、`net.tcp`、`net.udp`。

### net.socket

| 函数 | 说明 |
|------|------|
| `addr(host, port)` | 创建地址 |
| `close(resource)` | 关闭 |

### net.tcp

| 函数 | 说明 |
|------|------|
| `connect(addr)` | TCP 连接 |
| `bind(addr)` | TCP 监听 |
| `accept(listener)` | 接受连接 |
| `write(stream, data)` | 写入 |
| `read(stream, len?)` | 读取 |
| `close(resource)` | 关闭 |
| `connect_task` / `accept_task` / `read_task` / `write_task` | 异步版本 |

### net.udp

| 函数 | 说明 |
|------|------|
| `bind(addr)` | UDP 绑定 |
| `recv_from(socket, len?)` | 接收数据 |
| `send_to(socket, data, addr)` | 发送数据 |
| `recv_from_task` / `send_to_task` | 异步版本 |

```lk
use { tcp } from net;
let addr = net.socket.addr("127.0.0.1", 8080);
let stream = net.tcp.connect(addr);
net.tcp.write(stream, "hello");
net.tcp.close(stream);
```

## slice

切片视图。

| 函数 | 说明 |
|------|------|
| `from_list(list)` | 从列表创建 |
| `from_string(str)` | 从字符串创建 |
| `len(slice)` | 长度 |
| `is_empty(slice)` | 是否为空 |
| `get(slice, index)` | 获取元素 |
| `sub(slice, start[, end])` | 截取子段 |
| `to_list(slice)` | 转列表 |
| `to_string(slice)` | 转字符串 |

## encoding

父命名空间。`use { json, yaml, toml, base64, hex, url } from encoding`。

### encoding.json

| 函数 | 说明 |
|------|------|
| `parse(string)` | 解析 JSON 字符串 |

### encoding.yaml

| 函数 | 说明 |
|------|------|
| `parse(string)` | 解析 YAML 字符串 |

### encoding.toml

| 函数 | 说明 |
|------|------|
| `parse(string)` | 解析 TOML 字符串 |

### encoding.base64

| 函数 | 说明 |
|------|------|
| `encode(data)` | 编码（接受 Bytes 或 String） |
| `decode(string)` | 解码为 Bytes |

### encoding.hex

| 函数 | 说明 |
|------|------|
| `encode(data)` | 编码 |
| `decode(string)` | 解码为 Bytes |

### encoding.url

| 函数 | 说明 |
|------|------|
| `encode_component(string)` | URL 编码 |
| `decode_component(string)` | URL 解码 |
| `query_parse(string)` | 解析查询字符串 |
| `query_stringify(map)` | 序列化为查询字符串 |

```lk
use { json, base64 } from encoding;
let data = json.parse("{\"name\": \"LK\"}");
let encoded = base64.encode("hello");
let decoded = base64.decode(encoded);
```

## hash

哈希函数。

| 函数 | 说明 |
|------|------|
| `sha256(data)` | SHA-256 |
| `sha1(data)` | SHA-1 |
| `crc32(data)` | CRC-32 |
| `fnv64(data)` | FNV-64 |

`data` 接受 `Bytes` 或 `String`。

```lk
use hash;
hash.sha256("hello")     // SHA-256 哈希
hash.fnv64("hello")      // FNV-64 哈希
```

## regex

正则表达式。

| 函数 | 说明 |
|------|------|
| `is_match(pattern, text)` | 是否匹配 |
| `find(pattern, text)` | 查找第一个 |
| `find_all(pattern, text)` | 查找所有 |
| `captures(pattern, text)` | 捕获分组 |
| `replace(pattern, text, replacement)` | 替换 |
| `split(pattern, text)` | 按正则拆分 |

```lk
use regex;
regex.is_match(r"\d+", "abc123")     // true
regex.find(r"\d+", "abc123")          // "123"
regex.split(r"[,;]", "a,b;c")         // ["a", "b", "c"]
```

## random

随机数。

| 函数 | 说明 |
|------|------|
| `int(min, max)` | 随机整数 |
| `float()` | 随机浮点数 [0, 1) |
| `bool([probability])` | 随机布尔 |
| `bytes(len)` | 随机字节 |
| `choice(list)` | 随机选取 |
| `shuffle(list)` | 随机打乱 |

```lk
use random;
random.int(1, 100)    // 随机整数
random.choice(["a", "b", "c"])
random.shuffle([1, 2, 3, 4, 5])
```

## uuid

UUID 生成与验证。

| 函数 | 说明 |
|------|------|
| `v4()` | 生成 UUID v4 |
| `parse(string)` | 解析 UUID |
| `is_valid(string)` | 验证 UUID 格式 |

```lk
use uuid;
let id = uuid.v4();
println(id);
```

## http

同步 HTTP 客户端。

| 函数 | 说明 |
|------|------|
| `request(method, url[, opts])` | 通用请求 |
| `get(url[, opts])` | GET 请求 |
| `post(url, body[, opts])` | POST 请求 |

响应为包含 `status`、`headers`、`body: Bytes` 的映射。

```lk
use http;
let resp = http.get("https://httpbin.org/get");
println("status: {}", resp.status);
println("body: {}", bytes.to_string_utf8(resp.body));
```

## time（并发）

时间函数，需要启用并发 feature gate。

| 函数 | 说明 |
|------|------|
| `now()` | 当前微秒时间戳 |
| `sleep(ms)` | 休眠毫秒 |
| `timeout(ms)` | 创建超时 |
| `after(ms)` | 延迟值 |
| `since(start, end)` | 计算间隔 |

## task（并发）

任务管理，需要启用并发 feature gate。

| 函数 | 说明 |
|------|------|
| `spawn(fn_or_closure)` | 创建任务 |
| `await(handle)` | 等待任务结果 |

## chan（并发）

通道操作，需要启用并发 feature gate。

| 函数 | 说明 |
|------|------|
| `chan(capacity?, type?)` | 创建通道 |
| `send(channel, value)` | 发送值 |
| `recv(channel)` | 接收值，返回 `[ok, value]` |
| `close(channel)` | 关闭通道 |

## 元方法速查

无需导入，通过 `value.method()` 直接调用。

### List

`len` `push` `set` `concat` `join` `get` `first` `last` `map` `filter` `reduce` `take` `skip` `chain` `flatten` `unique` `chunk` `enumerate` `zip` `to_stream` `sort` `reverse` `pop` `insert` `remove_at` `contains` `index_of` `slice` `is_empty`

### Map

`len` `is_empty` `keys` `values` `has` `get` `set` `delete` `clear`

### Set

`len` `is_empty` `has` `contains` `add` `delete` `remove` `values` `clear`

### Stream

`map` `filter` `take` `skip` `chain` `subscribe` `collect` `collect_block`

### StreamCursor

`next` `collect` `next_block` `collect_block`

### Channel

`to_stream`
