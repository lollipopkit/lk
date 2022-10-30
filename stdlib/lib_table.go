package stdlib

import (
	. "git.lolli.tech/lollipopkit/lk/api"
)

var tableLib = map[string]GoFunction{
	"len": tableLen,
	"keys": tableKeys,
	"values": tableValues,
	"have": tableHave,
}

func OpenTableLib(ls LkState) int {
	ls.NewLib(tableLib)
	ls.CreateTable(0, 1)
	ls.CreateTable(0, 1)
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

func tableKeys(ls LkState) int {
	t := CheckTable(&ls, 1)
	keys := make([]interface{}, 0)
	for k := range t {
		keys = append(keys, k)
	}
	pushList(&ls, keys)
	return 1
}

func tableValues(ls LkState) int {
	t := CheckTable(&ls, 1)
	values := make([]interface{}, 0)
	for _, v := range t {
		values = append(values, v)
	}
	pushList(&ls, values)
	return 1
}

func tableHave(ls LkState) int {
	t := CheckTable(&ls, 1)
	key := ls.CheckString(2)
	_, ok := t[key]
	ls.PushBoolean(ok)
	return 1
}