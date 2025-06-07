package state

import (
    "fmt"
    . "github.com/lollipopkit/lk/api"
    . "github.com/lollipopkit/lk/json"
)

// 添加元表支持
type lkMap struct {
    _map    map[any]any
    mt      *lkMap // 元表
    keys    map[any]any
    lastKey any
    changed bool
}

func (self *lkMap) hasMetafield(fieldName string) bool {
    if self.mt == nil {
        return false
    }
    return self.mt.get(fieldName) != nil
}

func (self *lkMap) setMetatable(mt *lkMap) {
    self.mt = mt
}

func (self *lkMap) getMetatable() *lkMap {
    return self.mt
}

func newLkMap(size int) *lkMap {
    return &lkMap{
        _map: make(map[any]any, size),
    }
}

func (self *lkMap) len() int {
    return len(self._map)
}

func (self *lkMap) get(key any) any {
    if key == nil {
        return nil
    }
    return self._map[key]
}

func (self *lkMap) put(key, val any) {
    if key == nil {
        panic("map key cannot be nil")
    }
    
    self.changed = true
    
    if val != nil {
        self._map[key] = val
    } else {
        delete(self._map, key)
    }
}

func (self *lkMap) hasKey(key any) bool {
    _, ok := self._map[key]
    return ok
}

func (self *lkMap) String() (string, error) {
    m := make(map[string]any)
    for k, v := range self._map {
        key := fmt.Sprintf("%v", k)
        m[key] = convertToJsonValue(v)
    }
    s, err := Json.Marshal(m)
    return string(s), err
}

func (self *lkMap) Json() any {
    m := make(map[string]any)
    for k, v := range self._map {
        key := fmt.Sprintf("%v", k)
        m[key] = convertToJsonValue(v)
    }
    return m
}

func (self *lkMap) nextKey(key any) any {
    if self.keys == nil || (key == nil && self.changed) {
        self.initKeys()
        self.changed = false
    }
    
    return self.keys[key]
}

func (self *lkMap) initKeys() {
    self.keys = make(map[any]any)
    var key any = nil
    
    for k := range self._map {
        self.keys[key] = k
        key = k
    }
    
    self.lastKey = key
}