package state

import (
	"fmt"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/utils"
)

//go:inline
func typeOf(val any) LkType {
	switch val.(type) {
	case nil:
		return LK_TNIL
	case bool:
		return LK_TBOOLEAN
	case int64, float64, int, float32:
		return LK_TNUMBER
	case string:
		return LK_TSTRING
	case *lkList:
		return LK_TLIST
	case *lkMap:
		return LK_TMAP
	case *lkClosure:
		return LK_TFUNCTION
	case *lkState:
		return LK_TTHREAD
	default:
		panic(fmt.Sprintf("invalid type: %T<%v>", val, val))
	}
}

//go:inline
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
	case int:
		return float64(x), true
	case float64:
		return x, true
	case float32:
		return float64(x), true
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
	case int:
		return int64(x), true
	case float64:
		return utils.FloatToInteger(x)
	case float32:
		return utils.FloatToInteger(float64(x))
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

func getMetatable(val any, ls *lkState) (mt, global *lkMap) {
	key := fmt.Sprintf("_MT%d", typeOf(val))
	if gmt := ls.registry.get(key); gmt != nil {
		global = gmt.(*lkMap)
	}
	mt, _ = val.(*lkMap)
	return
}

func setMetatable(val any, mt *lkMap, ls *lkState) {
	if t, ok := val.(*lkMap); ok {
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
