# Lang LK
## 第一行代码
```js
print("Hello World!")
```
以上内容在屏幕上打印出 `Hello World!`。  
`String` 除了可以用 `'` `"` 包裹，还可以用 `` ` `` 包裹（表示这是个 `Raw String` ），这样可以避免转义字符的问题。


## 基本类型
```js
a = 1      // num
a = '1'    // str
a = false  // bool
a = {}     // table
a = nil     // nil
```
`LK`中的基本类型有`num` `str` `bool` `nil` `table`。

```js
print(a == b)  // false <-> num != str
```
如上所示，虽然 `LK` 不需要明确指出变量的类型，但不意味着是弱类型语言。  
不同类型间比较，不会先转化为同类型。

```js
```

## 变量
```js
a = 1
b = '1'
if true {
    shy a = 3
    print(a)  // 3
}
print(a)  // 1
```
`shy` 表示该变量为局部变量，只在当前作用域内有效。  
请**尽量**使用 `shy` 关键字，这会提高程序运行速度（因为不需要全局寻找变量）。  
`LK` 中已经声明的变量可以再次声明，这时会在其作用域内覆盖原来的值。  

```js
a, b = 1
print(a, b)  // 1 nil
```
`LK` 中可以使用 `,` 分隔多个变量，这时会将多个变量的值赋值给左边的变量。  
如果右边的变量个数少于左边的变量个数，那么多余的变量会被赋值为 `nil`。  


## 循环
```js
while condition {
    // ...
}
```
```js
for i = 0, 10 {
    // ...
}
```
等同于: `for i = 0; i <= 10; i++ {}`
```js
for i = 0, 10, 2 {
    // ...
}
```
等同于: `for i = 0; i <= 10; i = i + 2 {}`

## 流程控制
```py
if condition {
    // ...
} elif condition {
    // ...
} else {
    // ...
}
```

## 运算
### 算术运算符
```js
a = 21
b = 10
c = a + b
print("Line 1 - c 的值为 ", c)
c = a - b
print("Line 2 - c 的值为 ", c)
c = a * b
print("Line 3 - c 的值为 ", c)
c = a / b
print("Line 4 - c 的值为 ", c)
c = a % b
print("Line 5 - c 的值为 ", c)
c = a^2
print("Line 6 - c 的值为 ", c)
c = -a
print("Line 7 - c 的值为 ", c)
```
输出：
```
Line 1 - c 的值为     31
Line 2 - c 的值为     11
Line 3 - c 的值为     210
Line 4 - c 的值为     2.1
Line 5 - c 的值为     1
Line 6 - c 的值为     441
Line 7 - c 的值为     -21
```
### 关系运算符
```js
a = 21
b = 10

if (a == b) {
   print("Line 1 - a 等于 b")
else
   print("Line 1 - a 不等于 b")
}

if (a != b) {
   print("Line 2 - a 不等于 b")
else
   print("Line 2 - a 等于 b")
}

if (a < b) {
   print("Line 3 - a 小于 b")
else
   print("Line 3 - a 大于等于 b")
}

if (a > b) {
   print("Line 4 - a 大于 b")
else
   print("Line 5 - a 小于等于 b")
}

// 修改 a 和 b 的值
a = 5
b = 20
if (a <= b) {
   print("Line 5 - a 小于等于 b")
}

if (b >= a) {
   print("Line 6 - b 大于等于 a")
}
```
输出：
```
Line 1 - a 不等于 b
Line 2 - a 不等于 b
Line 3 - a 大于等于 b
Line 4 - a 大于 b
Line 5 - a 小于等于 b
Line 6 - b 大于等于 a
```

### 逻辑运算符
```js
print(false and true)  // false
print(false or true)   // true
print(not false)       // true
```

### 其他运算符
```js
a = "Hello "
b = "World"

// 连接字符串 a 和 b
print(a..b)  // Hello World
// b 字符串长度
print(#b)   // 5
```

### 运算符优先级
```js
a = 20
b = 10
c = 15
d = 5

e = (a + b) * c / d  // (30 * 15) / 5
print(e)  // 90.0

e = ((a + b) * c) / d  // (30 * 15) / 5
print(e)  // 90.0

e = (a + b) * (c / d)  // (30) * (15 / 5)
print(e)  // 90.0

e = a + (b * c) / d  // 20 + (150 / 5)
print(e)  // 50.0
```

## 迭代器
### 默认迭代
```js
shy a = {'num': 1, 'str': '1', 'bool': false, 'nil': nil}
for k, v in a {
    print(k, v)
}
```
其中 `for k, v in a` 就创建了一个迭代器， 
当 `a` 是 `table` 时，编译器会使用内置的迭代器，在每次迭代时为 `k` 和 `v` 分别赋值为 `a` 的键和值。 

### 自定义迭代器
#### 无状态迭代器
```js
fn square(iteratorMaxCount, currentNumber) {
    if currentNumber < iteratorMaxCount {
        currentNumber = currentNumber + 1
        rt currentNumber, currentNumber * currentNumber
    }
}

for i, n in square, 3, 0 {
   print(i, n)
}
```
这样就实现了一个简单的平方迭代器，输出：
```
1    1
2    4
3    9
```

#### 有状态迭代器
```js
fn iter (a, i)
    i = i + 1
    shy v = a[i]
    if v {
       rt i, v
    }
}
 
fn ipairs (a) {
    rt iter, a, 0
}
```
如上，实现了虚拟机内置的默认迭代器


## 函数
```js
shy fn (a, b) {
    return a + b
}
print(add(1, 2))  // 3
```
函数同样可以使用 `shy` 关键字表明为局部函数，仅在当前文件内有效。

```js
shy add = fn (a, b) {
    return a + b
}
```
除去常规的函数声明，`LK` 还可以将函数赋值给变量，这样可以实现匿名函数。


## 包
```js
// 文件名为 module.lua
// 定义一个名为 module 的模块
module = {}
 
// 定义一个常量
module.constant = "这是一个常量"
 
// 定义一个函数
fn module.func1() {
    print("这是一个公有函数！\n")
}
 
shy fn func2() {
    print("这是一个私有函数！")
}
 
fn module.func3() {
    func2()
}
 
rt module
```
如上定义了一个包，然后在另一个文件中导入：
```js
import "test"
```
可以通过 `import` 关键字导入包，导入的包会在当前文件作用域中有效。  
导入路径 `test` 为当前文件的相对路径。  
例如`import "a/b/c"`，会尝试导入：`./a/b/c.lk` `./a/b/c/init.lk`。  

导入后如下使用：
```js
test.func1()
// test.func2() 不可直接使用，因为时局部函数，但可以通过 module.func3() 调用
test.func3()
```


## 标准库
### `string`
### `utf8`
### `os`
### `math`
### `re`
### `http`
### `json`
### `sync`