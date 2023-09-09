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
resp, code, err := http.req(
    'POST', // Method
    'https://http.lolli.tech/post', // URL
    {'accept': 'application/json'}, // Headers
    {'foo': 'bar'} // Body
)
if err != nil {
    errorf('http req: %s', err) // Internal error(f) func
}
printf('code: %d, body: %s', code, resp)

// Json parse
obj, err := json(resp)
if err != nil {
    errorf('json parse: %s', err)
}
foo := obj['json']['foo']
// Regular matching
if foo != nil and foo:match('[bar]{3}') {
    printf('match: %s', foo)
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
        - [x] Support `a.0` (equals to `a[0]`) 
- [x] CLI
    - [x] Support incoming parameters (`lk args.lk arg1` -> calling `os.args` to get args)
    - [x] Display call stack when error
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