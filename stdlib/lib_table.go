package stdlib

import (
	. "git.lolli.tech/lollipopkit/lk/api"
)

var tableLib = map[string]GoFunction{
	"len": tableLen,
	"contains": tableContains,
}

func OpenTableLib(ls LkState) int {
	ls.NewLib(tableLib)
	ls.CreateTable(0, 1)
	ls.PushValue(-1)
	ls.PushValue(-2)
	ls.SetMetatable(-2)
	ls.Pop(1)
	ls.PushValue(-2)
	ls.SetField(-2, "__index")
	ls.Pop(1)
	return 1
}

func tableLen(ls LkState) int {
	t := CheckTable(&ls, 1)
	len := 0
	for range t {
		len++
	}
	ls.PushInteger(int64(len))
	return 1
}

func tableContains(ls LkState) int {
	t := CheckTable(&ls, 1)
	key := ls.CheckString(2)
	_, ok := t[key]
	ls.PushBoolean(ok)
	return 1
}