package state

import (
	"fmt"

	. "git.lolli.tech/lollipopkit/lk/api"
)

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_typename
func (self *lkState) TypeName(tp LkType) string {
	switch tp {
	case LK_TNONE:
		return "none"
	case LK_TNIL:
		return "nil"
	case LK_TBOOLEAN:
		return "bool"
	case LK_TNUMBER:
		return "num"
	case LK_TSTRING:
		return "str"
	case LK_TTABLE:
		return "table"
	case LK_TFUNCTION:
		return "func"
	case LK_TTHREAD:
		return "thread"
	default:
		return "userdata"
	}
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_type
func (self *lkState) Type(idx int) LkType {
	if self.stack.isValid(idx) {
		val := self.stack.get(idx)
		return typeOf(val)
	}
	return LK_TNONE
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isnone
func (self *lkState) IsNone(idx int) bool {
	return self.Type(idx) == LK_TNONE
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isnil
func (self *lkState) IsNil(idx int) bool {
	return self.Type(idx) == LK_TNIL
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isnoneornil
func (self *lkState) IsNoneOrNil(idx int) bool {
	return self.Type(idx) <= LK_TNIL
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isboolean
func (self *lkState) IsBoolean(idx int) bool {
	return self.Type(idx) == LK_TBOOLEAN
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_istable
func (self *lkState) IsTable(idx int) bool {
	return self.Type(idx) == LK_TTABLE
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isfunction
func (self *lkState) IsFunction(idx int) bool {
	return self.Type(idx) == LK_TFUNCTION
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isthread
func (self *lkState) IsThread(idx int) bool {
	return self.Type(idx) == LK_TTHREAD
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isstring
func (self *lkState) IsString(idx int) bool {
	t := self.Type(idx)
	return t == LK_TSTRING || t == LK_TNUMBER
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isnumber
func (self *lkState) IsNumber(idx int) bool {
	_, ok := self.ToNumberX(idx)
	return ok
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_isinteger
func (self *lkState) IsInteger(idx int) bool {
	val := self.stack.get(idx)
	_, ok := val.(int64)
	return ok
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_iscfunction
func (self *lkState) IsGoFunction(idx int) bool {
	val := self.stack.get(idx)
	if c, ok := val.(*lkClosure); ok {
		return c.goFunc != nil
	}
	return false
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_toboolean
func (self *lkState) ToBoolean(idx int) bool {
	val := self.stack.get(idx)
	return convertToBoolean(val)
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tointeger
func (self *lkState) ToInteger(idx int) int64 {
	i, _ := self.ToIntegerX(idx)
	return i
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tointegerx
func (self *lkState) ToIntegerX(idx int) (int64, bool) {
	val := self.stack.get(idx)
	return convertToInteger(val)
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tonumber
func (self *lkState) ToNumber(idx int) float64 {
	n, _ := self.ToNumberX(idx)
	return n
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tonumberx
func (self *lkState) ToNumberX(idx int) (float64, bool) {
	val := self.stack.get(idx)
	return convertToFloat(val)
}

// [-0, +0, m]
// http://www.lua.org/manual/5.3/manual.html#lua_tostring
func (self *lkState) ToString(idx int) string {
	s, _ := self.ToStringX(idx)
	return s
}

func (self *lkState) ToStringX(idx int) (string, bool) {
	val := self.stack.get(idx)

	switch x := val.(type) {
	case string:
		return x, true
	case int64, float64:
		s := fmt.Sprintf("%v", x) // todo
		self.stack.set(idx, s)
		return s, true
	default:
		return "", false
	}
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tocfunction
func (self *lkState) ToGoFunction(idx int) GoFunction {
	val := self.stack.get(idx)
	if c, ok := val.(*lkClosure); ok {
		return c.goFunc
	}
	return nil
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_tothread
func (self *lkState) ToThread(idx int) LkState {
	val := self.stack.get(idx)
	if val != nil {
		if ls, ok := val.(*lkState); ok {
			return ls
		}
	}
	return nil
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_topointer
func (self *lkState) ToPointer(idx int) interface{} {
	// todo
	return self.stack.get(idx)
}
