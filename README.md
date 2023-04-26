<h1 align="center">Lang LK</h1>

<p align="center">
    <img alt="badge-lang" src="https://badgen.net/badge/LK/0.3.0/cyan">
    <img alt="badge-lang" src="https://badgen.net/badge/Go/1.19/purple">
</p>

<div align="center">
💌 致谢 - <a href="https://www.lua.org">lua</a> - <a href="https://github.com/zxh0/luago-book">luago</a>

简体中文 | [English](README_en.md)
</div>



## ⌨️ 体验
#### 获取 
- 一键安装：`go install github.com/lollipopkit/lk@latest`
- [Release](https://github.com/LollipopKit/lang-lk/releases) 下载
- 克隆后，`go build` 生成


#### CLI
详细说明可以运行 `lk --help` 查看
```bash
# 进入REPL交互式解释器
lk
# 执行.lk(c)文件
lk <file>
# 编译.lk文件
lk -c <file>
# 为.lk文件，生成语法树
lk -a <file>
```

## 📄 语法
#### 详细
- **Step by step** ➜ [LANG.md](LANG.md)
- **By examples** ➜ [脚本](scripts) or [测试集](test)

#### 示例
```js
// http发送请求示例
resp, err := http.post(
    'http://httpbin.org/post', // URL
    {'accept': 'application/json'}, // Headers
    '{"foo": "bar"}' // Body
)
if err != nil {
    error(err) // 内置的error方法
}
print(resp.code, resp.body)

// json解析
if json.get(resp.body, 'json.foo') != 'bar' {
    error('mismatch result')
}

// 以下是http监听部分
class Header {
    'items': {}
}

fn Header.fromTable(h) {
    self := new(Header)
    for k, v in h {
        self.items[k] = v
    }
    rt self
}

// `print`的参数，如果非`str`类型，会调用`__str`方法
// 这里`Header`类实现了`__str`方法
fn Header:__str() {
    shy s = ''
    for k, v in self.items {
        s = fmt('%s%s: %s\n', s, k, v)
    }
    rt s
}

/*
处理监听事件
`req`包含属性`method`, `url`, `body`, `headers`
*/
handler := fn(req) => 200, fmt('%s %s\n\n%s\n%s', req.method, req.url, Header.fromTable(req.headers), req.body)

// 监听
if http.listen(':8080', handler) != nil {
    error(err)
}
```

## 🔖 TODO
- [x] 语法
  - [x] 注释：`//` `/* */`
  - [x] 去除 `repeat`, `until`, `goto`, `..` (`concat`)
  - [x] Raw String, 使用 ``` ` ``` 包裹字符
  - [x] 面向对象
  - [x] 自动添加 `range` ( `paris` )
  - [x] 语法糖
    - [x] 三元操作符 `a ? b : c`
    - [x] `a == nil ? b : a` -> `a ?? b`
    - [x] `shy a = b` -> `a := b`
    - [x] `shy a = fn(b) {rt c}` -> `shy a = fn(b) => c`
    - [x] 支持 `a++` `a+=b` 等
  - [x] Table
    - [x] key为StringExp，而不是NameExp
    - [x] 构造方式：`=` -> `:`, eg: `{a = 'a'}` -> `{a: 'a'}`
    - [x] 索引从 `0` 开始
    - [x] 改变 `metatable` 设置方式
- [x] CLI
  - [x] 支持传入参数 ( `lk args.lk arg1` -> `.lk`内调用`os.args` )
  - [x] REPL，直接运行 `./lk` 即可进入
    - [x] 支持方向键
    - [x] 识别代码块
- [x] 资源
    - [x] 文档
      - [x] `LANG.md` 
      - [x] 测试集，位于 `test` 文件夹
    - [x] IDE
      - [x] VSCode高亮  

## 🌳 生态
- Vscode插件：[高亮](https://github.com,/lollipopkit/vscode-lk-highlight)

## 📝 License
```
lollipopkit 2023 GPL v3
```