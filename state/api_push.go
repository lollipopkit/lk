package state

import (
	"fmt"

	. "git.lolli.tech/lollipopkit/lk/api"
)

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushnil
func (self *lkState) PushNil() {
	self.stack.push(nil)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushboolean
func (self *lkState) PushBoolean(b bool) {
	self.stack.push(b)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushinteger
func (self *lkState) PushInteger(n int64) {
	self.stack.push(n)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushnumber
func (self *lkState) PushNumber(n float64) {
	self.stack.push(n)
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#lua_pushstring
func (self *lkState) PushString(s string) {
	self.stack.push(s)
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_pushfstring
func (self *lkState) PushFString(fmtStr string, a ...interface{}) {
	str := fmt.Sprintf(fmtStr, a...)
	self.stack.push(str)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushcfunction
func (self *lkState) PushGoFunction(f GoFunction) {
	self.stack.push(newGoClosure(f, 0))
}

// [-n, +1, m]
// http://www.lua.org/manual/5.3/manual.html#lua_pushcclosure
func (self *lkState) PushGoClosure(f GoFunction, n int) {
	closure := newGoClosure(f, n)
	for i := n; i > 0; i-- {
		val := self.stack.pop()
		closure.upVals[i-1] = &val
	}
	self.stack.push(closure)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushglobaltable
func (self *lkState) PushGlobalTable() {
	global := self.registry.get(LK_RIDX_GLOBALS)
	self.stack.push(global)
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_pushthread
func (self *lkState) PushThread() bool {
	self.stack.push(self)
	return self.isMainThread()
}
