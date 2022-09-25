# Lang LK
改编自Lua5.3，[luago](https://github.com/zxh0/luago-book)

## CLI
```bash
# 编译test/basic.lk，输出到test/basic.lkc
./go-lang-lk -c test/basic.lk
# 运行test/basic.lkc
./go-lang-lk test/basic.lkc
# 也可以运行test/basic.lk（内部会先进行编译）
./go-lang-lk test/basic.lk
```

## 速览
**详细语法**可以查看[test](test)文件夹的内容
#### 变量
```lua
shy a = {a="a", "b", 'c'}
```
`shy`表明为局部变量

#### 函数
```lua
shy func = fn (e) {print(e)}
func("hello")

fn hello() {
    print("hello")
}
```

#### if
```lua
if #a >= 0 {
    print("hello")
}
```

#### for
```lua
for i = 0, #a {
    print(a[i])
}
for b,c in a {
    print(b,c)
}
```

#### 网络请求
```lua
shy url = 'http://httpbin.org/post'
shy headers = 'accept: application/json'
shy body = '{"foo": "bar"}'
shy code, resp = http.req('post', url, headers, body)
print(code, resp)
```

#### json
```lua
shy ok, data = json.get(resp, 'data')
print(ok, data)
```
用法请参考`gjson`库
