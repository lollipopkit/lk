package stdlib

import (
	"strconv"

	. "github.com/lollipopkit/lk/api"
)

var numLib = map[string]GoFunction{
	"abs":  numAbs,
	"len":  numLen,
	"char": numChar,
}

func OpenNumLib(ls LkState) int {
	ls.NewLib(numLib)
	ls.CreateTable(0, 1)
	ls.PushInteger(0)
	ls.PushValue(-2)
	ls.SetMetatable(-2)
	ls.Pop(1)
	ls.PushValue(-2)
	ls.SetField(-2, "__index")
	ls.Pop(1)
	return 1
}

func numAbs(ls LkState) int {
	n := ls.CheckNumber(1)
	if n < 0 {
		n = -n
	}
	ls.PushNumber(n)
	return 1
}

func numLen(ls LkState) int {
	n := ls.CheckNumber(1)
	ls.PushInteger(int64(len(strconv.FormatFloat(n, 'f', -1, 64))))
	return 1
}

func numChar(ls LkState) int {
	n := ls.CheckInteger(1)
	ls.PushString(string(rune(n)))
	return 1
}
