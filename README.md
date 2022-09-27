# Lang LK
改编自Lua5.3，[luago](https://github.com/zxh0/luago-book)

## 速览
**详细语法**可以查看[test](test)文件夹的内容
#### 变量
```lua
shy a = {'a': "a", "b", 'c'}
```
`shy`表明为局部变量

#### comment
```go
// 单行注释
/*
多行注释
*/
```

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

#### http & json & metatable
```lua
shy code, resp = http.req(
    'post', 
    'http://httpbin.org/post', 
    {'accept': 'application/json'}, 
    '{"foo": "bar"}'
)
print(code, resp)

print('json.foo:', json.get(resp, 'json.foo'))



shy headers = {}
headers.__str = fn(a) {
    shy s = ''
    for k, v in a {
        shy ss = ''
        for _, vv in v {
            ss = ss .. vv .. ';'
        }
        s = s .. k .. ': ' .. ss .. '\n'
    }
    rt s
}

shy fn handle(req) {
    setmetatable(req.headers, headers)
    rt 200, string.format('%s %s\n\n%s\n%s', req.method, req.url, req.headers, req.body)
}

if http.listen(':8080', handle) != nil {
    error(err)
}
```
`req`包含属性`method`, `url`, `body`, `headers`

## CLI
```bash
# 编译test/basic.lk，输出到test/basic.lkc
./go-lang-lk -c test/basic.lk
# 运行test/basic.lkc
./go-lang-lk test/basic.lkc
# 也可以运行test/basic.lk（内部会先进行编译）
./go-lang-lk test/basic.lk
```