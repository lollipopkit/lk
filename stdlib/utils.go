package stdlib

import (
	"fmt"
	"reflect"

	. "github.com/lollipopkit/lk/api"
)

// type lkMapKey interface {
// 	string | int | int64
// }

type lkMap map[string]any

func pushValue(ls LkState, item any) {
	switch i := item.(type) {
	case string:
		ls.PushString(i)
	case int64:
		ls.PushInteger(i)
	case int:
		ls.PushInteger(int64(i))
	case float64:
		ls.PushNumber(i)
	case bool:
		ls.PushBoolean(i)
	case GoFunction:
		ls.PushGoFunction(i)
	case nil:
		ls.PushNil()
	default:
		v := reflect.ValueOf(i)
		switch v.Kind() {
		case reflect.Slice:
			items := make([]any, v.Len())
			for i := 0; i < v.Len(); i++ {
				items[i] = v.Index(i).Interface()
			}
			pushList(ls, items)
			return
		case reflect.Map:
			items := make(lkMap)
			keys := v.MapKeys()
			for idx := range keys {
				key := &keys[idx]
				items[(*key).String()] = v.MapIndex(*key).Interface()
			}
			pushTable(ls, items)
			return
		}
		panic(fmt.Sprintf("unsupported type: %T", item))
	}
}

func pushList[T any](ls LkState, items []T) {
	ls.CreateTable(len(items), 0)
	for i := range items {
		pushValue(ls, items[i])
		ls.SetI(-2, int64(i))
	}
}

func pushTable[T any](ls LkState, items map[string]T) {
	ls.CreateTable(0, len(items)+1)
	for k := range items {
		pushValue(ls, items[k])
		ls.SetField(-2, k)
	}
}

func getTable(ls LkState, idx int) lkMap {
	ls.CheckType(idx, LK_TLIST)
	table := make(lkMap)
	ls.PushNil()
	for ls.Next(idx) {
		key := ls.ToString(-2)
		val := ls.ToPointer(-1)
		table[key] = val
		ls.Pop(1)
	}
	return table
}

func getList(ls LkState, idx int) []any {
	ls.CheckType(idx, LK_TLIST)
	list := make([]any, 0)
	ls.PushNil()
	for ls.Next(idx) {
		list = append(list, ls.ToPointer(-1))
		ls.Pop(1)
	}
	return list
}

func CheckTable(ls LkState, idx int) lkMap {
	ls.CheckType(idx, LK_TMAP)
	return getTable(ls, idx)
}

func CheckList(ls LkState, idx int) []any {
	ls.CheckType(idx, LK_TLIST)
	return getList(ls, idx)
}

func OptList(ls LkState, idx int, dft []any) []any {
	if ls.IsNoneOrNil(idx) {
		return dft
	}
	return getList(ls, idx)
}

func OptTable(ls LkState, idx int, dft lkMap) lkMap {
	if ls.IsNoneOrNil(idx) {
		return dft
	}
	return getTable(ls, idx)
}

// lua-5.3.4/src/loslib.c#getfield()
func _getField(ls LkState, key string, dft int64) int {
	t := ls.GetField(-1, key) /* get field and its type */
	res, isNum := ls.ToIntegerX(-1)
	if !isNum { /* field is not an integer? */
		if t != LK_TNIL { /* some other value? */
			return ls.Error2("field '%s' is not an integer", key)
		} else if dft < 0 { /* absent field; no default? */
			return ls.Error2("field '%s' missing in date table", key)
		}
		res = dft
	}
	ls.Pop(1)
	return int(res)
}

func getFunc(ls LkState, idx int) GoFunction {
	ls.CheckType(idx, LK_TFUNCTION)
	return ls.ToGoFunction(idx)
}
