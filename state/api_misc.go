package state

import (
	"fmt"

	"github.com/lollipopkit/lk/utils"
)

// 更新 Next 函数以支持 list 和 map
func (self *lkState) Next(idx int) bool {
	val := self.stack.get(idx)
	key := self.stack.pop()

	switch t := val.(type) {
	case *lkList:
		return self.nextList(t, key)
	case *lkMap:
		return self.nextMap(t, key)
	default:
		panic(fmt.Sprintf("table expected, got %T", val))
	}
}

func (self *lkState) nextList(list *lkList, key any) bool {
	var idx int64 = -1

	if key != nil {
		if i, ok := convertToInteger(key); ok {
			idx = i
		}
	}

	idx++
	if idx < int64(list.len()) {
		self.stack.push(idx)
		self.stack.push(list.get(idx))
		return true
	}

	return false
}

func (self *lkState) nextMap(m *lkMap, key any) bool {
	nextKey := m.nextKey(key)
	if nextKey != nil {
		self.stack.push(nextKey)
		self.stack.push(m.get(nextKey))
		return true
	}
	return false
}

func (self *lkState) Len(idx int) {
    val := self.stack.get(idx)
    
    switch v := val.(type) {
    case string:
        self.stack.push(int64(len(v)))
    case *lkList:
        self.stack.push(int64(v.len()))
    case *lkMap:
        self.stack.push(int64(v.len()))
    default:
        if result, ok := callMetamethod(val, val, "__len", self); ok {
            self.stack.push(result)
        } else {
            panic(fmt.Sprintf("attempt to get length of %T value", val))
        }
    }
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
