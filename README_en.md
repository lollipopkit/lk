<h1 align="center">Lang LK</h1>

<p align="center">
    <img alt="badge-lang" src="https://badgen.net/badge/LK/0.3.0/cyan">
    <img alt="badge-lang" src="https://badgen.net/badge/Go/1.19/purple">
</p>

<div align="center">
💌  <a href="https://www.lua.org">lua</a> - <a href="https://github.com/zxh0/luago-book">luago</a>
</div>

## ⌨️ Experience
#### Get
- One click installation: `go install github.com/lollipopkit/lk@latest`
- Download from [Release](https://github.com/LollipopKit/lang-lk/releases)
- After cloning, uses `go build` to generates

#### CLI
Detailed instructions can be viewed by running `lk --help`
```bash
# Enter the REPL interactive interpreter
lk
# Execute .lk(c) file
lk <file>
# Compile .lk file
lk -c <file>
# Generate syntax tree for .lk file
lk -a <file>
```


## 📄 Grammar
#### Detailed
- **Step by step** ➜ [LANG.md](LANG.md)
- **By examples** ➜ [scripts](scripts) or [test set](test)
#### Example
```js
// Example of http sending request
resp, err := http.post(
    'http://httpbin.org/post', // URL
    {'accept': 'application/json'}, // Headers
    '{"foo": "bar"}' // Body
)
if err != nil {
    error(error) // The built-in error method
}
print(resp.code, resp.body)

// Json parsing
if json.get(resp.body, 'json.foo') != 'bar' {
    error('mismatch result')
}

// The following is the http listener
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

// Parameter of 'print'. If it is not of type 'str', it will be called '__str' metamethod
// Here, the 'Header' class implements the '__str' method
fn Header:__ str() {
    shy s = ''
    for k, v in self.items {
        s = fmt('%s%s: %s\n', s, k, v)
    }
    rt s
}

/*
Processing listening events
'req' contains attributes 'method', 'url', 'body', 'headers'
*/
handler := fn(req) => 200, fmt('%s %s\n\n%s\n%s', req.method, req.url, Header.fromTable(req.headers), req.body)

// Monitoring on 8080
if http.listen(':8080', handler) != nil {
    error(err)
}
```
##  🔖  TODO
- [x] Syntax
    - [x] Comment: `//` `/**/`
    - [x] Remove `repeat`, `until`, `goto`, `..` (`concat`)
    - [x] Raw String, using ``` ` ``` wrap character
    - [x] Object oriented
    - [x] Automatically add 'range' ('paris')
    - [x] Grammar sugar
        - [x] Triple Operator `a ? b : c`
        - [x] `a == nil ?  b : a` -> `a ?? b`
        - [x] `shy a = b` -> `a := b`
        - [x] `shy a = fn(b) {rt c}` -> `shy a = fn(b) => c`
        - [x] Support `a++` `a+=b` etc
    - [x] Table
        - [x] The key is `StringExp`, not `NameExp`
        - [x] Construction method: `=` -> `:`, eg: `{a = 'a'}` -> `{a: 'a'}`
        - [x] Index starts from `0`
        - [x] Change the setting method of `metatable`
- [x] CLI
    - [x] Support incoming parameters (`lk args.lk arg1` -> calling `os.args` to get args)
    - [x] REPL, run directly `lk` to enter
    - [x] Support direction keys
    - [x] Identification code block
    - [x] Resources
- [x] Documentation
  - [x] `LANG.md` 
  - [x] Test set, located in the `test` folder
- [x] IDE
  - [x] VSCode highlights

## 🌳 Ecology
- VSCode plugin: [highlight](https://github.com,/lollipopkit/vscode-lk-highlight)

## 📝 License
```
lollipopkit 2023 GPL v3
```