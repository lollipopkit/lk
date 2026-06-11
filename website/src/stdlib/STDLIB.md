# Standard Library Overview

LK standard library modules are imported on demand. Use `use` to bring a module into scope, then call its functions and constants.

```lk
use math;
use { json } from encoding;

math.sqrt(16)         // 4
json.parse("{\"a\":1}")  // { "a": 1 }
```

String, List, Map, and Set meta-methods don't require imports — call them directly with `value.method()`.

## math

Math constants and functions.

**Constants**: `pi`, `e`, `inf`, `nan`, `max_int`, `min_int`, `max_float`, `epsilon`

**Functions**:

| Function | Signature | Description |
|----------|-----------|-------------|
| `abs(x)` | Int/Float → Int/Float | Absolute value |
| `sqrt(x)` | Float → Float | Square root |
| `floor(x)` | Float → Int | Floor |
| `ceil(x)` | Float → Int | Ceiling |
| `round(x)` | Float → Float | Round |
| `min(a, b)` | same type → same | Minimum |
| `max(a, b)` | same type → same | Maximum |
| `pow(base, exp)` | Float, Float → Float | Power |
| `exp(x)` | Float → Float | e^x |
| `sin/cos/tan(x)` | Float → Float | Trigonometric |
| `asin/acos/atan(x)` | Float → Float | Inverse trig |
| `atan2(y, x)` | Float, Float → Float | Four-quadrant arctangent |
| `log/log10/log2(x)` | Float → Float | Logarithms |
| `clamp(val, lo, hi)` | same type → same | Clamp to range |
| `random()` | → Float | Random [0, 1) |
| `hypot(x, y)` | Float, Float → Float | √(x²+y²) |
| `cbrt(x)` | Float → Float | Cube root |
| `sinh/cosh/tanh(x)` | Float → Float | Hyperbolic trig |
| `trunc(x)` | Float → Float | Truncate |
| `fract(x)` | Float → Float | Fractional part |
| `sign(x)` | Float → Float | Sign |
| `to_int(x)` | Float → Int | Cast to Int |
| `to_float(x)` | Int → Float | Cast to Float |
| `is_nan(x)` | Float → Bool | Is NaN |
| `is_inf(x)` | Float → Bool | Is infinity |

```lk
use math;
math.sqrt(2)        // 1.4142135623730951
math.pow(2, 10)     // 1024.0
math.clamp(15, 0, 10) // 10
```

## string

String meta-methods — no import needed, call via `value.method()`.

| Method | Description |
|--------|-------------|
| `len()` | Character count |
| `lower()` | Lowercase |
| `upper()` | Uppercase |
| `trim()` | Trim whitespace |
| `starts_with(prefix)` | Prefix check |
| `ends_with(suffix)` | Suffix check |
| `contains(sub)` | Contains substring |
| `replace(old, new)` | Replace substring |
| `substring(start[, end])` | Extract substring |
| `split(sep)` | Split to list |
| `join(list)` | Join list with this string |
| `reverse()` | Reverse string |
| `repeat(n)` | Repeat n times |
| `chars()` | Split to character list |
| `char_at(index)` | Character at index |
| `byte_at(index)` | Byte at index |
| `find(sub)` | Find substring position, nil if not found |
| `is_empty()` | Is empty |
| `format(args...)` | Format string |

```lk
"Hello, {}!".format("LK")    // "Hello, LK!"
"  hello  ".trim()            // "hello"
"a,b,c".split(",")           // ["a", "b", "c"]
```

## bytes

Binary data operations.

| Function | Description |
|----------|-------------|
| `from_list(list)` | Create from integer list |
| `from_string(str)` | Create from UTF-8 string |
| `len(bytes)` | Byte length |
| `is_empty(bytes)` | Is empty |
| `get(bytes, index)` | Byte at index |
| `slice(bytes, start[, end])` | Slice |
| `to_list(bytes)` | Convert to integer list |
| `to_string_utf8(bytes)` | Convert to UTF-8 string |
| `to_string_lossy(bytes)` | Convert to UTF-8 (replace invalid bytes) |
| `concat(a, b)` | Concatenate |
| `eq(a, b)` | Equality check |

```lk
use bytes;
let raw = bytes.from_string("hello");
bytes.len(raw)                  // 5
bytes.to_string_utf8(raw)       // "hello"
bytes.concat(raw, bytes.from_string("!"))
```

## iter

List utility functions.

| Function | Description |
|----------|-------------|
| `range([start,] end [, step])` | Generate range list |
| `enumerate(list)` | Indexed → [[index, item], ...] |
| `zip(list1, list2)` | Zip pairs |
| `take(list, n)` | Take first n |
| `skip(list, n)` | Skip first n |
| `chain(list1, list2)` | Concatenate lists |
| `flatten(list)` | Flatten one level |
| `unique(list)` | Remove duplicates |
| `chunk(list, size)` | Split into chunks |
| `map(list, fn)` | Map |
| `filter(list, fn)` | Filter |
| `reduce(list, init, fn)` | Reduce |

```lk
use iter;
let nums = iter.range(1, 6);
iter.map(nums, |n| n * 2)           // [2, 4, 6, 8, 10]
iter.filter(nums, |n| n % 2 == 0)  // [2, 4]
iter.reduce(nums, 0, |acc, n| acc + n) // 15
```

## stream

Lazy evaluation stream pipelines. Available when concurrency feature gate is enabled.

| Function | Description |
|----------|-------------|
| `from_list(list)` | Create stream from list |
| `range(start, end)` | Create stream from range |
| `iterate(seed, fn)` | Iterator stream |
| `repeat(val)` | Repeating value stream |
| `from_channel(ch)` | Create stream from channel |
| `map(s, fn)` | Map |
| `filter(s, fn)` | Filter |
| `take(s, n)` | Take first n |
| `skip(s, n)` | Skip first n |
| `chain(a, b)` | Concatenate streams |
| `subscribe(s)` | Create cursor |
| `next(cursor)` | Get next value |
| `collect(stream_or_cursor)` | Collect to list |
| `next_block(cursor[, timeout_ms])` | Get next block |
| `collect_block(stream_or_cursor[, n][, timeout_ms])` | Collect a block |

```lk
use stream;
let s = stream.from_list([1, 2, 3, 4, 5]);
let cursor = stream.subscribe(stream.map(s, |n| n * 10));
stream.collect(cursor)  // [10, 20, 30, 40, 50]
```

## datetime

Date and time helpers.

| Function | Description |
|----------|-------------|
| `now()` | Current timestamp in microseconds |
| `format(secs, fmt)` | Format timestamp |
| `parse(str, fmt)` | Parse time string |
| `add(secs, delta)` | Add time delta |
| `sub(secs, delta)` | Subtract time delta |
| `day_of_week(secs)` | Day of week |
| `day_of_year(secs)` | Day of year |
| `is_weekend(secs)` | Is weekend |

```lk
use datetime;
let now = datetime.now();
datetime.format(now, "%Y-%m-%d %H:%M:%S")
```

## os

Platform information.

| Function | Description |
|----------|-------------|
| `hostname()` | Host name |
| `arch()` | Architecture |
| `os()` | Operating system |
| `clock()` | Process clock |
| `time()` | Current seconds |
| `epoch()` | Unix timestamp |

```lk
use os;
println(os.os());    // e.g. "macos"
println(os.arch());  // e.g. "aarch64"
```

## fs

Filesystem operations (path-based).

| Function | Description |
|----------|-------------|
| `read(path)` | Read as Bytes |
| `read_to_string(path)` | Read as String |
| `write(path, data)` | Write file |
| `append(path, data)` | Append to file |
| `exists(path)` | Path exists |
| `is_file(path)` | Is file |
| `is_dir(path)` | Is directory |
| `metadata(path)` | File metadata |
| `read_dir(path)` | List directory |
| `create_dir(path)` | Create directory |
| `create_dir_all(path)` | Create directory recursively |
| `remove_file(path)` | Remove file |
| `remove_dir(path)` | Remove empty directory |
| `remove_dir_all(path)` | Remove directory recursively |
| `rename(from, to)` | Rename |
| `copy(from, to)` | Copy file |
| `canonicalize(path)` | Canonicalize path |
| `temp_dir()` | Temporary directory |

```lk
use fs;
let content = fs.read_to_string("config.json");
fs.write("output.txt", "hello");
```

## path

Path manipulation.

| Function | Description |
|----------|-------------|
| `join(parts...)` | Join path components |
| `parent(path)` | Parent directory |
| `file_name(path)` | File name |
| `file_stem(path)` | File name without extension |
| `extension(path)` | File extension |
| `with_extension(path, ext)` | Replace extension |
| `is_absolute(path)` | Is absolute path |
| `normalize(path)` | Normalize path |
| `components(path)` | Path components list |
| `sep()` | Path separator |
| `delimiter()` | Environment delimiter |

```lk
use path;
path.join("src", "main.lk")  // "src/main.lk"
path.extension("app.lk")     // "lk"
```

## env

Environment variables (read-only).

| Function | Description |
|----------|-------------|
| `get(key)` | Get environment variable |
| `get_or(key, default)` | Get with default |
| `has(key)` | Key exists |
| `vars()` | All environment variables |

```lk
use env;
let home = env.get_or("HOME", "/tmp");
```

## process

Process operations.

| Function | Description |
|----------|-------------|
| `id()` | Current process ID |
| `cwd()` | Current working directory |
| `set_cwd(path)` | Set working directory |
| `exit(code)` | Exit process |
| `status(cmd[, args])` | Run command, return exit code |
| `output(cmd[, args])` | Run command, return `{status, success, stdout, stderr}` |
| `output_string(cmd[, args])` | Same but stdout/stderr as String |

```lk
use process;
let result = process.output_string("echo", ["hello"]);
println(result.stdout);  // "hello\n"
```

## io

Parent namespace. Import children with `use { std, file } from io` or access via `io.std` and `io.file`.

### io.std

Standard I/O.

| Function | Description |
|----------|-------------|
| `stdin()` | Standard input |
| `stdout()` | Standard output |
| `stderr()` | Standard error |
| `read(reader[, max_bytes])` | Read as Bytes |
| `read_to_string(reader)` | Read as String |
| `read_line(reader)` | Read a line |
| `write(writer, data)` | Write |
| `writeln(writer, data)` | Write with newline |
| `flush(writer)` | Flush buffer |

### io.file

File resource I/O.

| Function | Description |
|----------|-------------|
| `open(path, mode)` | Open file |
| `create(path)` | Create file |
| `read(file[, max_bytes])` | Read |
| `read_to_string(file)` | Read as String |
| `read_line(file)` | Read a line |
| `write(file, data)` | Write |
| `writeln(file, data)` | Write with newline |
| `write_all(file, data)` | Write all |
| `flush(file)` | Flush |
| `close(file)` | Close file |

```lk
use { std, file } from io;
let input = io.std.read_to_string(io.std.stdin());
let f = io.file.open("data.txt", "read");
let content = io.file.read_to_string(f);
io.file.close(f);
```

## net

Parent namespace. `use { socket, tcp, udp } from net` or `net.socket`, `net.tcp`, `net.udp`.

### net.socket

| Function | Description |
|----------|-------------|
| `addr(host, port)` | Create address |
| `close(resource)` | Close |

### net.tcp

| Function | Description |
|----------|-------------|
| `connect(addr)` | TCP connect |
| `bind(addr)` | TCP listen |
| `accept(listener)` | Accept connection |
| `write(stream, data)` | Write |
| `read(stream, len?)` | Read |
| `close(resource)` | Close |
| `connect_task` / `accept_task` / `read_task` / `write_task` | Async variants |

### net.udp

| Function | Description |
|----------|-------------|
| `bind(addr)` | UDP bind |
| `recv_from(socket, len?)` | Receive data |
| `send_to(socket, data, addr)` | Send data |
| `recv_from_task` / `send_to_task` | Async variants |

```lk
use { tcp } from net;
let addr = net.socket.addr("127.0.0.1", 8080);
let stream = net.tcp.connect(addr);
net.tcp.write(stream, "hello");
net.tcp.close(stream);
```

## slice

Slice views.

| Function | Description |
|----------|-------------|
| `from_list(list)` | Create from list |
| `from_string(str)` | Create from string |
| `len(slice)` | Length |
| `is_empty(slice)` | Is empty |
| `get(slice, index)` | Get element |
| `sub(slice, start[, end])` | Slice |
| `to_list(slice)` | Convert to list |
| `to_string(slice)` | Convert to string |

## encoding

Parent namespace. `use { json, yaml, toml, base64, hex, url } from encoding`.

### encoding.json

| Function | Description |
|----------|-------------|
| `parse(string)` | Parse JSON string |

### encoding.yaml

| Function | Description |
|----------|-------------|
| `parse(string)` | Parse YAML string |

### encoding.toml

| Function | Description |
|----------|-------------|
| `parse(string)` | Parse TOML string |

### encoding.base64

| Function | Description |
|----------|-------------|
| `encode(data)` | Encode (Bytes or String) |
| `decode(string)` | Decode to Bytes |

### encoding.hex

| Function | Description |
|----------|-------------|
| `encode(data)` | Encode |
| `decode(string)` | Decode to Bytes |

### encoding.url

| Function | Description |
|----------|-------------|
| `encode_component(string)` | URL-encode |
| `decode_component(string)` | URL-decode |
| `query_parse(string)` | Parse query string |
| `query_stringify(map)` | Serialize to query string |

```lk
use { json, base64 } from encoding;
let data = json.parse("{\"name\": \"LK\"}");
let encoded = base64.encode("hello");
let decoded = base64.decode(encoded);
```

## hash

Hash functions.

| Function | Description |
|----------|-------------|
| `sha256(data)` | SHA-256 |
| `sha1(data)` | SHA-1 |
| `crc32(data)` | CRC-32 |
| `fnv64(data)` | FNV-64 |

`data` accepts `Bytes` or `String`.

```lk
use hash;
hash.sha256("hello")     // SHA-256 hash
hash.fnv64("hello")      // FNV-64 hash
```

## regex

Regular expressions.

| Function | Description |
|----------|-------------|
| `is_match(pattern, text)` | Match check |
| `find(pattern, text)` | Find first match |
| `find_all(pattern, text)` | Find all matches |
| `captures(pattern, text)` | Capture groups |
| `replace(pattern, text, replacement)` | Replace |
| `split(pattern, text)` | Split by regex |

```lk
use regex;
regex.is_match(r"\d+", "abc123")     // true
regex.find(r"\d+", "abc123")          // "123"
regex.split(r"[,;]", "a,b;c")         // ["a", "b", "c"]
```

## random

Random number generation.

| Function | Description |
|----------|-------------|
| `int(min, max)` | Random integer |
| `float()` | Random float [0, 1) |
| `bool([probability])` | Random boolean |
| `bytes(len)` | Random bytes |
| `choice(list)` | Random choice |
| `shuffle(list)` | Random shuffle |

```lk
use random;
random.int(1, 100)    // random integer
random.choice(["a", "b", "c"])
random.shuffle([1, 2, 3, 4, 5])
```

## uuid

UUID generation and validation.

| Function | Description |
|----------|-------------|
| `v4()` | Generate UUID v4 |
| `parse(string)` | Parse UUID |
| `is_valid(string)` | Validate UUID format |

```lk
use uuid;
let id = uuid.v4();
println(id);
```

## http

Synchronous HTTP client.

| Function | Description |
|----------|-------------|
| `request(method, url[, opts])` | General request |
| `get(url[, opts])` | GET request |
| `post(url, body[, opts])` | POST request |

Response is a map with `status`, `headers`, and `body: Bytes`.

```lk
use http;
let resp = http.get("https://httpbin.org/get");
println("status: {}", resp.status);
println("body: {}", bytes.to_string_utf8(resp.body));
```

## time (concurrency)

Time functions. Requires concurrency feature gate.

| Function | Description |
|----------|-------------|
| `now()` | Current timestamp in microseconds |
| `sleep(ms)` | Sleep milliseconds |
| `timeout(ms)` | Create timeout |
| `after(ms)` | Delayed value |
| `since(start, end)` | Compute interval |

## task (concurrency)

Task management. Requires concurrency feature gate.

| Function | Description |
|----------|-------------|
| `spawn(fn_or_closure)` | Create task |
| `await(handle)` | Wait for task result |

## chan (concurrency)

Channel operations. Requires concurrency feature gate.

| Function | Description |
|----------|-------------|
| `chan(capacity?, type?)` | Create channel |
| `send(channel, value)` | Send value |
| `recv(channel)` | Receive value, returns `[ok, value]` |
| `close(channel)` | Close channel |

## Meta-method Quick Reference

No import needed — call via `value.method()`.

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
