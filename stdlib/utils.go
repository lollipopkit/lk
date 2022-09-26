package stdlib

import (
	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
)

func pushList(ls LkState, items []any) {
	ls.CreateTable(len(items), 0)
	for i, item := range items {
		switch item.(type) {
		case string:
			ls.PushString(item.(string))
		case int, int64:
			ls.PushInteger(item.(int64))
		case float32, float64:
			ls.PushNumber(item.(float64))
		}
		ls.SetI(-2, int64(i+1))
	}
}

func pushTable(ls LkState, items map[string]any) {
	ls.CreateTable(0, len(items))
	for k, v := range items {
		switch v.(type) {
		case string:
			ls.PushString(v.(string))
		case int, int64:
			ls.PushInteger(v.(int64))
		case float32, float64:
			ls.PushNumber(v.(float64))
		}
		ls.SetField(-2, k)
	}
}

func getTable(ls LkState, idx int) map[string]any {
	ls.CheckType(idx, LUA_TTABLE)
	table := make(map[string]any)
	ls.PushNil()
	for ls.Next(idx) {
		key := ls.ToString(-2)
		val := ls.ToPointer(-1)
		table[key] = val
		ls.Pop(1)
	}
	return table
}

func OptTable(ls LkState, idx int, dft map[string]any) map[string]any {
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
		if t != LUA_TNIL { /* some other value? */
			return ls.Error2("field '%s' is not an integer", key)
		} else if dft < 0 { /* absent field; no default? */
			return ls.Error2("field '%s' missing in date table", key)
		}
		res = dft
	}
	ls.Pop(1)
	return int(res)
}
