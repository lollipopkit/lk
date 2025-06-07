package state

import (
	"fmt"

	. "github.com/lollipopkit/lk/api"
)

// [-2, +0, e]
// http://www.lua.org/manual/5.3/manual.html#lua_settable
func (self *lkState) SetTable(idx int) {
	t := self.stack.get(idx)
	v := self.stack.pop()
	k := self.stack.pop()
	self.setTable(t, k, v, false)
}

// [-1, +0, e]
// http://www.lua.org/manual/5.3/manual.html#lua_setfield
func (self *lkState) SetField(idx int, k string) {
	t := self.stack.get(idx)
	v := self.stack.pop()
	self.setTable(t, k, v, false)
}

// [-1, +0, e]
// http://www.lua.org/manual/5.3/manual.html#lua_seti
func (self *lkState) SetI(idx int, i int64) {
	t := self.stack.get(idx)
	v := self.stack.pop()
	self.setTable(t, i, v, false)
}

// [-2, +0, m]
// http://www.lua.org/manual/5.3/manual.html#lua_rawset
func (self *lkState) RawSet(idx int) {
	t := self.stack.get(idx)
	v := self.stack.pop()
	k := self.stack.pop()
	self.setTable(t, k, v, true)
}

// [-1, +0, m]
// http://www.lua.org/manual/5.3/manual.html#lua_rawseti
func (self *lkState) RawSetI(idx int, i int64) {
	t := self.stack.get(idx)
	v := self.stack.pop()
	self.setTable(t, i, v, true)
}

// [-1, +0, e]
// http://www.lua.org/manual/5.3/manual.html#lua_setglobal
func (self *lkState) SetGlobal(name string) {
	t := self.registry.get(LK_RIDX_GLOBALS)
	v := self.stack.pop()
	self.setTable(t, name, v, false)
}

// [-0, +0, e]
// http://www.lua.org/manual/5.3/manual.html#lua_register
func (self *lkState) Register(name string, f GoFunction) {
	self.PushGoFunction(f)
	self.SetGlobal(name)
}

// [-1, +0, –]
// http://www.lua.org/manual/5.3/manual.html#lua_setmetatable
func (self *lkState) SetMetatable(idx int) {
	val := self.stack.get(idx)
	mtVal := self.stack.pop()

	if mtVal == nil {
		setMetatable(val, nil, self)
	} else if mt, ok := mtVal.(*lkMap); ok {
		setMetatable(val, mt, self)
	} else {
		panic("table expected!") // todo
	}
}

// t[k]=v
func (self *lkState) setTable(t, k, v any, raw bool) {
    switch x := t.(type) {
    case *lkList:
        if idx, ok := k.(int64); ok {
            x.set(idx, v)
            return
        }
        panic("list index must be integer")
    case *lkMap:
        if raw || x.get(k) != nil || !x.hasMetafield("__newindex") {
            x.put(k, v)
            return
        }
    }
    
    // metatable handling...
    panic(fmt.Sprintf("attempt to index %T value", t))
}
