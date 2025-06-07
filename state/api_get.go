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
	t := newLkMap(nArr, nRec)
	self.stack.push(t)
}

func (self *lkState) CreateMap(nArr, nRec int) {
	t := newLkMap(nArr, nRec)
	self.stack.push(t)
}

func (self *lkState) CreateList(nArr int) {
	t := newLkList(nArr)
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

// push(t[k])
func (self *lkState) getTable(t, k any, raw bool) LkType {
	mf := getMetafield(t, "__index", self)
	if tbl := toTable(t); tbl != nil {
		v := tbl.get(k)
		if raw || v != nil || !tbl.hasMetafield("__index") && mf == nil {
			self.stack.push(v)
			return typeOf(v)
		}
	}

	if !raw {
		if mf != nil {
			switch x := mf.(type) {
			case *lkMap, *lkList:
				return self.getTable(x, k, true)
			case *lkClosure:
				self.stack.push(mf)
				self.stack.push(t)
				self.stack.push(k)
				self.Call(2, 1)
				v := self.stack.get(-1)
				return typeOf(v)
			}
		}
	}

	self.PushNil()
	return LK_TNIL
}
