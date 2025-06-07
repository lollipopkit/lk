package state

import (
	. "github.com/lollipopkit/lk/api"
	. "github.com/lollipopkit/lk/json"
)

type lkList struct {
    arr []any
    mt  *lkMap // 元表
}

func newLkList(size int) *lkList {
	return &lkList{
		arr: make([]any, 0, size),
	}
}

func (self *lkList) len() int {
	return len(self.arr)
}

func (self *lkList) get(idx int64) any {
	if idx >= 0 && idx < int64(len(self.arr)) {
		return self.arr[idx]
	}
	return nil
}

func (self *lkList) set(idx int64, val any) {
	if idx < 0 {
		panic("list index must be non-negative")
	}

	// 自动扩展
	for int64(len(self.arr)) <= idx {
		self.arr = append(self.arr, nil)
	}

	self.arr[idx] = val
}

func (self *lkList) append(val any) {
	self.arr = append(self.arr, val)
}

func (self *lkList) insert(idx int64, val any) {
	if idx < 0 || idx > int64(len(self.arr)) {
		panic("list index out of range")
	}

	self.arr = append(self.arr, nil)
	copy(self.arr[idx+1:], self.arr[idx:])
	self.arr[idx] = val
}

func (self *lkList) remove(idx int64) any {
	if idx < 0 || idx >= int64(len(self.arr)) {
		return nil
	}

	val := self.arr[idx]
	self.arr = append(self.arr[:idx], self.arr[idx+1:]...)
	return val
}

func (self *lkList) String() (string, error) {
	arr := make([]any, len(self.arr))
	for i := range self.arr {
		arr[i] = convertToJsonValue(self.arr[i])
	}
	s, err := Json.Marshal(arr)
	return string(s), err
}

func (self *lkList) Json() any {
	arr := make([]any, len(self.arr))
	for i := range self.arr {
		arr[i] = convertToJsonValue(self.arr[i])
	}
	return arr
}
func (self *lkList) hasMetafield(fieldName string) bool {
	if self.mt == nil {
		return false
	}
	return self.mt.get(fieldName) != nil
}

func (self *lkList) setMetatable(mt *lkMap) {
	self.mt = mt
}

func (self *lkList) getMetatable() *lkMap {
	return self.mt
}
