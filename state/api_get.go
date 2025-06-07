package state

import (
	. "github.com/lollipopkit/lk/api"
)

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#lua_newtable
func (self *lkState) NewTable() {
	self.CreateTable(0, 0)
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#lua_createtable
func (self *lkState) CreateTable(nArr, nRec int) {
	t := newLkMap(nRec)
	self.stack.push(t)
}

// [-1, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_gettable
func (self *lkState) GetTable(idx int) LkType {
	t := self.stack.get(idx)
	k := self.stack.pop()
	return self.getTable(t, k, false)
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_getfield
func (self *lkState) GetField(idx int, k string) LkType {
	t := self.stack.get(idx)
	return self.getTable(t, k, false)
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_geti
func (self *lkState) GetI(idx int, i int64) LkType {
	t := self.stack.get(idx)
	return self.getTable(t, i, false)
}

// [-1, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_rawget
func (self *lkState) RawGet(idx int) LkType {
	t := self.stack.get(idx)
	k := self.stack.pop()
	return self.getTable(t, k, true)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_rawgeti
func (self *lkState) RawGetI(idx int, i int64) LkType {
	t := self.stack.get(idx)
	return self.getTable(t, i, true)
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_getglobal
func (self *lkState) GetGlobal(name string) LkType {
	t := self.registry.get(LK_RIDX_GLOBALS)
	return self.getTable(t, name, false)
}

// [-0, +(0|1), –]
// http://www.lua.org/manual/5.3/manual.html#lua_getmetatable
func (self *lkState) GetMetatable(idx int) bool {
	val := self.stack.get(idx)
	mt, gmt := getMetatable(val, self)

	if mt != nil {
		self.stack.push(mt)
		return true
	} else if gmt != nil {
		self.stack.push(gmt)
		return true
	} else {
		return false
	}
}


func (self *lkState) NewList() {
    self.CreateList(0)
}

func (self *lkState) CreateList(size int) {
    list := newLkList(size)
    self.stack.push(list)
}

func (self *lkState) NewMap() {
    self.CreateMap(0)
}

func (self *lkState) CreateMap(size int) {
    m := newLkMap(size)
    self.stack.push(m)
}

// push(t[k])
func (self *lkState) getTable(t, k any, raw bool) LkType {
    switch x := t.(type) {
    case *lkList:
        if idx, ok := k.(int64); ok {
            self.stack.push(x.get(idx))
            return typeOf(x.get(idx))
        }
    case *lkMap:
        v := x.get(k)
        if raw || v != nil || !x.hasMetafield("__index") {
            self.stack.push(v)
            return typeOf(v)
        }
    }
    
    self.PushNil()
    return LK_TNIL
}
