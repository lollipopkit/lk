package state

import (
	"math"
	"strconv"

	"git.lolli.tech/lollipopkit/lk/utils"
)

// var (
// 	_closureRe = regexp.MustCompile(`"((GoFunc|LkFunc)@0x[0-9a-f]+)"`)
// )

type lkTable struct {
	arr     []any
	_map    map[any]any
	keys    map[any]any // used by next()
	lastKey any         // used by next()
	changed bool        // used by next()
}

func (self *lkTable) String() (string, error) {
	return json.MarshalToString(self.Json())
	// if err != nil {
	// 	return "", err
	// }

	// return _closureRe.ReplaceAllString(s, "$1"), nil
}

func (t *lkTable) Json() any {
	tb := t
	for i := range tb.arr {
		switch v := tb.arr[i].(type) {
		case *lkClosure:
			tb.arr[i] = v.String()
		case *lkTable:
			tb.arr[i] = v.Json()
		}
	}
	for k := range tb._map {
		switch v := tb._map[k].(type) {
		case *lkClosure:
			tb._map[k] = v.String()
		case *lkTable:
			tb._map[k] = v.Json()
		}
	}
	if len(tb._map) == 0 {
		return tb.arr
	}
	return tb._map
}

func (self *lkTable) combine(t *lkTable) {
	if t == nil {
		return
	}
	for i := range t.arr {
		self.put(int64(i), t.arr[i])
	}
	for k := range t._map {
		self.put(k, t._map[k])
	}
}

func newLkTable(nArr, nRec int) *lkTable {
	t := &lkTable{}
	if nArr > 0 {
		t.arr = make([]any, 0, nArr)
	}
	if nRec > 0 {
		t._map = make(map[any]any, nRec)
	}
	return t
}

func (self *lkTable) hasMetafield(fieldName string) bool {
	return self.get(fieldName) != nil
}

func (self *lkTable) len() int {
	return len(self.arr)
}

func (self *lkTable) get(key any) any {
	key = _floatToInteger(key)
	if idx, ok := key.(int64); ok {
		if idx >= 0 && idx < int64(len(self.arr)) {
			return self.arr[idx]
		}
	}
	return self._map[key]
}

func _floatToInteger(key any) any {
	if f, ok := key.(float64); ok {
		if i, ok := utils.FloatToInteger(f); ok {
			return i
		}
	}
	return key
}

func (self *lkTable) put(key, val any) {
	if key == nil {
		panic("table index is nil!")
	}
	if f, ok := key.(float64); ok && math.IsNaN(f) {
		panic("table index is NaN!")
	}

	self.changed = true
	key = _floatToInteger(key)
	if idx, ok := key.(int64); ok && idx >= 0 {
		arrLen := int64(len(self.arr))
		if idx < arrLen {
			self.arr[idx] = val
			if idx == arrLen-1 && val == nil {
				self._shrinkArray()
			}
			return
		}
		if idx == arrLen {
			delete(self._map, key)
			if val != nil {
				self.arr = append(self.arr, val)
				self._expandArray()
			}
			return
		}
	}
	if val != nil {
		if self._map == nil {
			self._map = make(map[any]any, 8)
		}
		self._map[key] = val
	} else {
		delete(self._map, key)
	}
}

func (self *lkTable) _shrinkArray() {
	for i := len(self.arr) - 1; i >= 0; i-- {
		if self.arr[i] == nil {
			self.arr = self.arr[0:i]
		} else {
			break
		}
	}
}

func (self *lkTable) _expandArray() {
	for idx := int64(len(self.arr)) + 1; true; idx++ {
		if val, found := self._map[idx]; found {
			delete(self._map, idx)
			self.arr = append(self.arr, val)
		} else {
			break
		}
	}
}

func (self *lkTable) nextKey(key any) any {
	if self.keys == nil || (key == nil && self.changed) {
		self.initKeys()
		self.changed = false
	}

	nextKey := self.keys[key]
	if nextKey == nil && key != nil && key != self.lastKey {
		k, ok := key.(string)
		if !ok {
			return nil
		}
		intKey, err := strconv.ParseInt(k, 10, 64)
		if err != nil {
			return nil
		}
		nextKey = self.keys[intKey]
	}

	return nextKey
}

func (self *lkTable) initKeys() {
	self.keys = make(map[any]any)
	var key any = nil
	for i := range self.arr {
		if self.arr[i] != nil {
			self.keys[key] = int64(i)
			key = int64(i)
		}
	}
	for k := range self._map {
		if self._map[k] != nil {
			self.keys[key] = k
			key = k
		}
	}
	self.lastKey = key
}
