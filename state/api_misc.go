package state

import (
	"fmt"

	jsoniter "github.com/json-iterator/go"
	"github.com/lollipopkit/lk/utils"
)

var (
	json = jsoniter.ConfigCompatibleWithStandardLibrary
)

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_len
func (self *lkState) Len(idx int) {
	val := self.stack.get(idx)

	if s, ok := val.(string); ok {
		self.stack.push(int64(len(s)))
	} else if result, ok := callMetamethod(val, val, "__len", self); ok {
		self.stack.push(result)
	} else if t, ok := val.(*lkTable); ok {
		self.stack.push(int64(t.len()))
	} else {
		panic(fmt.Sprintf("attempt to get length of %#v (a %T value)", val, val))
	}
}

// [-1, +(2|0), e]
// http://www.lua.org/manual/5.3/manual.html#lua_next
func (self *lkState) Next(idx int) bool {
	val := self.stack.get(idx)
	if t, ok := val.(*lkTable); ok {
		key := self.stack.pop()
		if nextKey := t.nextKey(key); nextKey != nil {
			self.stack.push(nextKey)
			self.stack.push(t.get(nextKey))
			return true
		}
		return false
	}
	panic("table expected!")
}

// [-1, +0, v]
// http://www.lua.org/manual/5.3/manual.html#lua_error
func (self *lkState) Error() int {
	err := self.stack.pop()
	panic(err)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_stringtoutils
func (self *lkState) StringToNumber(s string) bool {
	if n, ok := utils.ParseInteger(s); ok {
		self.PushInteger(n)
		return true
	}
	if n, ok := utils.ParseFloat(s); ok {
		self.PushNumber(n)
		return true
	}
	return false
}
