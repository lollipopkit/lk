package state

import . "git.lolli.tech/lollipopkit/lk/api"

type lkState struct {
	registry *lkTable
	stack    *lkStack
	/* coroutine */
	coStatus int
	coCaller *lkState
	coChan   chan int
}

func New() LkState {
	ls := &lkState{}

	registry := newLuaTable(8, 0)
	registry.put(LUA_RIDX_MAINTHREAD, ls)
	registry.put(LUA_RIDX_GLOBALS, newLuaTable(0, 20))

	ls.registry = registry
	ls.pushLuaStack(newLuaStack(LUA_MINSTACK, ls))
	return ls
}

func (self *lkState) isMainThread() bool {
	return self.registry.get(LUA_RIDX_MAINTHREAD) == self
}

func (self *lkState) pushLuaStack(stack *lkStack) {
	stack.prev = self.stack
	self.stack = stack
}

func (self *lkState) popLuaStack() {
	stack := self.stack
	self.stack = stack.prev
	stack.prev = nil
}
