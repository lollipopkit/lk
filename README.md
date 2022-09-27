# Lang LK
改编自Lua5.3，[luago](https://github.com/zxh0/luago-book)

## 速览
**详细语法**，可以查看[test](test)文件夹的内容

```js
// 发送请求
shy _, resp = http.post(
    'http://httpbin.org/post', 
    {'accept': 'application/json'}, 
    '{"foo": "bar"}'
)
print(resp)

// json解析
if json.get(resp, 'json.foo') != 'bar' {
    error('mismatch result')
}

// 设置metatable
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

// 处理监听
shy fn handle(req) {
    setmetatable(req.headers, headers)
    rt 200, string.format('%s %s\n\n%s\n%s', req.method, req.url, req.headers, req.body)
}

// 监听
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