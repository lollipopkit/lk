package stdlib

import (
	"regexp"
	"strconv"
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
	"match":    strMatch,
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

// strMatch returns a Map of matches: {"GROUP": "MATCH"}
func strMatch(ls LkState) int {
	s := ls.CheckString(1)
	pattern := ls.CheckString(2)
	exp, err := regexp.Compile(pattern)
	if err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
	} else {
		matches := map[string]string{}
		names := exp.SubexpNames()
		for idx, name := range names {
			if len(name) == 0 {
				names[idx] = strconv.Itoa(idx)
			}
		}
		for idx, match := range exp.FindStringSubmatch(s) {
			matches[names[idx]] = match
		}
		if len(matches) == 0 {
			ls.PushNil()
			ls.PushString("no matches")
		} else {
			pushTable(ls, matches)
			ls.PushNil()
		}
	}
	return 2
}

func strJoin(ls LkState) int {
	sep := ls.CheckString(1)
	list := CheckList(ls, 2)
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

func strLen(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushInteger(int64(len(s)))
	return 1
}

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

func strLower(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushString(strings.ToLower(s))
	return 1
}

func strUpper(ls LkState) int {
	s := ls.CheckString(1)
	ls.PushString(strings.ToUpper(s))
	return 1
}

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
