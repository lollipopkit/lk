package state

import (
	"fmt"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/utils"
)

func typeOf(val any) LkType {
	switch val.(type) {
	case nil:
		return LUA_TNIL
	case bool:
		return LUA_TBOOLEAN
	case int64, float64:
		return LUA_TNUMBER
	case string:
		return LUA_TSTRING
	case *lkTable:
		return LUA_TTABLE
	case *closure:
		return LUA_TFUNCTION
	case *lkState:
		return LUA_TTHREAD
	default:
		panic(fmt.Sprintf("invalid type: %T<%v>", val, val))
	}
}

func convertToBoolean(val any) bool {
	switch x := val.(type) {
	case nil:
		return false
	case bool:
		return x
	default:
		return true
	}
}

// http://www.lua.org/manual/5.3/manual.html#3.4.3
func convertToFloat(val any) (float64, bool) {
	switch x := val.(type) {
	case int64:
		return float64(x), true
	case float64:
		return x, true
	case string:
		return utils.ParseFloat(x)
	default:
		return 0, false
	}
}

// http://www.lua.org/manual/5.3/manual.html#3.4.3
func convertToInteger(val any) (int64, bool) {
	switch x := val.(type) {
	case int64:
		return x, true
	case float64:
		return utils.FloatToInteger(x)
	case string:
		return _stringToInteger(x)
	default:
		return 0, false
	}
}

func _stringToInteger(s string) (int64, bool) {
	if i, ok := utils.ParseInteger(s); ok {
		return i, true
	}
	if f, ok := utils.ParseFloat(s); ok {
		return utils.FloatToInteger(f)
	}
	return 0, false
}

/* metatable */

func getMetatable(val any, ls *lkState) (mt, global *lkTable) {
	key := fmt.Sprintf("_MT%d", typeOf(val))
	if gmt := ls.registry.get(key); gmt != nil {
		global = gmt.(*lkTable)
	}
	mt, _ = val.(*lkTable)
	return
}

func setMetatable(val any, mt *lkTable, ls *lkState) {
	if t, ok := val.(*lkTable); ok {
		t.combine(mt)
		//return
	}
	key := fmt.Sprintf("_MT%d", typeOf(val))
	ls.registry.put(key, mt)
}

func getMetafield(val any, fieldName string, ls *lkState) any {
	mt, gmt := getMetatable(val, ls)
	if mt != nil {
		f := mt.get(fieldName)
		if f != nil {
			return f
		}
	}
	if gmt != nil {
		return gmt.get(fieldName)
	}
	return nil
}

func callMetamethod(a, b any, mmName string, ls *lkState) (any, bool) {
	var mm any
	if mm = getMetafield(a, mmName, ls); mm == nil {
		if mm = getMetafield(b, mmName, ls); mm == nil {
			return nil, false
		}
	}

	ls.stack.check(4)
	ls.stack.push(mm)
	ls.stack.push(a)
	ls.stack.push(b)
	ls.Call(2, 1)
	return ls.stack.pop(), true
}
