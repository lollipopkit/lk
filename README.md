# Lang LK
改编自Lua5.3，[luago](https://github.com/zxh0/luago-book)

## 速览
**详细语法**可以查看[test](test)文件夹的内容
#### 变量
```lua
shy a = {a: "a", "b", 'c'}
```
`shy`表明为局部变量

#### function
```lua
shy func = fn (e) {print(e)}

fn hello() {
    func('hello')
}
```

#### if & for
```lua
if #a >= 0 {
    print("hello")
}
for i = 0, #a {
    print(a[i])
}
for b,c in a {
    print(b,c)
}
```

#### http & json
```lua
shy code, resp = http.req(
    'post', 
    'http://httpbin.org/post', 
    {accept: 'application/json'}, 
    '{"foo": "bar"}'
)
print(code, resp)

print('json.foo:', json.get(resp, 'json.foo'))

shy fn handle(req) {
    shy body = req.body
    rt 200, body
}

if http.listen(':8080', handle) != nil {
    error(err)
}
```

## CLI
```bash
# 编译test/basic.lk，输出到test/basic.lkc
./go-lang-lk -c test/basic.lk
# 运行test/basic.lkc
./go-lang-lk test/basic.lkc
# 也可以运行test/basic.lk（内部会先进行编译）
./go-lang-lk test/basic.lk
```