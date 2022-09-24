# Lang LK
改编自Lua5.3，[luago](https://github.com/zxh0/luago-book)

## 速览
#### 定义变量
```lua
shy a = {"a", "b", 'c'}
```
`shy` -> `local`

#### 定义函数
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
for b,c in pairs(a) {
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
使用`gjson`库

## 改变
原始|更改后
-|-
`elseif`|`elif`
`function`|`fn`
`local`|`shy`
`return`|`rt`
`local`|`shy`
`do block end`|`{ block }`
`~=`|`!=`

## 测试
`go run . test.lk`