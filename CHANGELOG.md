# CHANGELOG
**仅包含语言LK的变化**

## 0.1.3
- 添加`prefixexp : Name args`的语法
- 支持面向对象
- `REPL`支持自动添加`print`

## 0.1.2
- `Table`索引从`0`开始
- 去除以`'''` `"""`构造长String，当前仅支持``` `` ```构造
- 修改`stdlib_http`库部分接口
- 包名改为`lk`
- `stdlib_os`新增`os.args`接口，可用于获取命令行参数
- 支持`ID++` `ID1 += ID2`等语法（通过编译器匹配，自动转换为`ID = ID + 1`
- `REPL`优化，支持方向键等（eg：上句存在语法错误，快捷修改上一行）
- `stdlib_os`支持`mkdir`

## 0.1.1
- 支持任意对象的`concat`
- 新增`stdlib_re`
- 支持`REPL`
- 支持`32`位系统
- `string.format` -> `fmt`

## 0.1.0
- `table`构造方式：`=` -> `:`, eg: `{a = 'a'}` -> `{a: 'a'}`
- 使用`json`作为编译后格式
- 使用`//` `/* */`用作注释
- 新增`stdlib_http` `stdlib_json`
- `stdlib_os`支持`write` `read` `rm` `mv` `exec`
- `stdlib_base`新增`kv`函数，返回`key`列表和`value`列表
- 支持将`table`作为参数传递
- 去除`repeat` `until` `goto`
- `do block end` -> `{ block }`
- `tostring` -> `str`, `tonumber` -> `num`
- 编译器自动为`for ID{, ID} in ID {`添加`range` ( 原`paris` )
- 将`stdlib_table`的内容移入`stdlib_base`
- 去除`prefixexp [‘:’ Name] args`支持，不允许`function ID:ID(args)`