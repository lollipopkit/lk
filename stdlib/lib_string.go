package stdlib

import (
	"fmt"
	"strings"

	. "github.com/lollipopkit/lk/api"
)

var strLib = map[string]GoFunction{
	"len":      strLen,
	"repeat":   strRep,
	"reverse":  strReverse,
	"lower":    strLower,
	"upper":    strUpper,
	"sub":      strSub,
	"bytes":    strByte,
	"char":     strChar,
	"split":    strSplit,
	"join":     strJoin,
	"contains": strContains,
	"replace":  strReplace,
}

func OpenStringLib(ls LkState) int {
	ls.NewLib(strLib)
	ls.CreateTable(0, 1)       /* table to be metatable for strings */
	ls.PushString("dummy")     /* dummy string */
	ls.PushValue(-2)           /* copy table */
	ls.SetMetatable(-2)        /* set table as metatable for strings */
	ls.Pop(1)                  /* pop dummy string */
	ls.PushValue(-2)           /* get string library */
	ls.SetField(-2, "__index") /* metatable.__index = string */
	ls.Pop(1)
	return 1
}

func strReplace(ls LkState) int {
	s := ls.CheckString(1)
	old := ls.CheckString(2)
	new := ls.CheckString(3)
	times := ls.OptInteger(4, -1)
	ls.PushString(strings.Replace(s, old, new, int(times)))
	return 1
}

func strContains(ls LkState) int {
	s := ls.CheckString(1)
	sub := ls.CheckString(2)
	ls.PushBoolean(strings.Contains(s, sub))
	return 1
}

func strJoin(ls LkState) int {
	list := CheckList(ls, 1)
	sep := ls.CheckString(2)
	l := make([]string, len(list))
	for i := range list {
		l[i] = list[i].(string)
	}
	ls.PushString(strings.Join(l, sep))
	return 1
}

func strSplit(ls LkState) int {
	s := ls.CheckString(1)
	sep := ls.CheckString(2)
	pushList(ls, strings.Split(s, sep))
	return 1
}

// string.len (s)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.len
// lua-5.3.4/src/lstrlib.c#str_len()
func strLen(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushInteger(int64(len(s)))
	return 1
}

// string.rep (s, n [, sep])
// http://www.lua.org/manual/5.3/manual.html#pdf-string.rep
// lua-5.3.4/src/lstrlib.c#str_rep()
func strRep(ls LkState) int {
	s := ls.CheckString(1)
	n := ls.CheckInteger(2)
	sep := ls.OptString(3, "")

	if n <= 0 {
		ls.PushString("")
	} else if n == 1 {
		ls.PushString(s)
	} else {
		a := make([]string, n)
		for i := 0; i < int(n); i++ {
			a[i] = s
		}
		ls.PushString(strings.Join(a, sep))
	}

	return 1
}

// string.reverse (s)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.reverse
// lua-5.3.4/src/lstrlib.c#str_reverse()
func strReverse(ls LkState) int {
	s := ls.CheckString(1)

	if strLen := len(s); strLen > 1 {
		a := make([]byte, strLen)
		for i := 0; i < strLen; i++ {
			a[i] = s[strLen-1-i]
		}
		ls.PushString(string(a))
	}

	return 1
}

// string.lower (s)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.lower
// lua-5.3.4/src/lstrlib.c#str_lower()
func strLower(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushString(strings.ToLower(s))
	return 1
}

// string.upper (s)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.upper
// lua-5.3.4/src/lstrlib.c#str_upper()
func strUpper(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushString(strings.ToUpper(s))
	return 1
}

// string.sub (s, i [, j])
// http://www.lua.org/manual/5.3/manual.html#pdf-string.sub
// lua-5.3.4/src/lstrlib.c#str_sub()
func strSub(ls LkState) int {
	s := ls.CheckString(1)
	sLen := len(s)
	i := posRelat(ls.CheckInteger(2), sLen)
	j := posRelat(ls.OptInteger(3, -1), sLen)

	if i < 1 {
		i = 1
	}
	if j > sLen {
		j = sLen
	}

	if i <= j {
		ls.PushString(s[i-1 : j])
	} else {
		ls.PushString("")
	}

	return 1
}

// string.byte (s [, i [, j]])
// http://www.lua.org/manual/5.3/manual.html#pdf-string.byte
// lua-5.3.4/src/lstrlib.c#str_byte()
func strByte(ls LkState) int {
	s := ls.CheckString(1)
	sLen := len(s)

	list := make([]int64, sLen)
	for k := 0; k < sLen; k++ {
		list[k] = int64(s[k])
	}
	pushList(ls, list)
	return 1
}

// string.char (···)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.char
// lua-5.3.4/src/lstrlib.c#str_char()
func strChar(ls LkState) int {
	nArgs := ls.GetTop()

	s := make([]byte, nArgs)
	for i := 1; i <= nArgs; i++ {
		c := ls.CheckInteger(i)
		ls.ArgCheck(int64(byte(c)) == c, i, "value out of range")
		s[i-1] = byte(c)
	}

	ls.PushString(string(s))
	return 1
}

func _fmtArg(tag string, ls LkState, argIdx int) string {
	switch tag[len(tag)-1] { // specifier
	case 'c': // character
		return string([]byte{byte(ls.ToInteger(argIdx))})
	case 'i':
		tag = tag[:len(tag)-1] + "d" // %i -> %d
		return fmt.Sprintf(tag, ls.ToInteger(argIdx))
	case 'd', 'o': // integer, octal
		return fmt.Sprintf(tag, ls.ToInteger(argIdx))
	case 'u': // unsigned integer
		tag = tag[:len(tag)-1] + "d" // %u -> %d
		return fmt.Sprintf(tag, uint(ls.ToInteger(argIdx)))
	case 'x', 'X': // hex integer
		return fmt.Sprintf(tag, uint(ls.ToInteger(argIdx)))
	case 'f': // float
		return fmt.Sprintf(tag, ls.ToNumber(argIdx))
	case 's', 'q': // string
		return fmt.Sprintf(tag, ls.ToString2(argIdx))
	default:
		panic("todo! tag=" + tag)
	}
}
