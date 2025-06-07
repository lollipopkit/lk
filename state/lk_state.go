package state

import . "github.com/lollipopkit/lk/api"

type lkState struct {
	registry *tableBase
	stack    *lkStack
	/* coroutine */
	coStatus LkStatus
	coCaller *lkState
	coChan   chan int
}

func New() LkState {
	ls := &lkState{}

	registry := newLkMap(8, 0).tableBase
	registry.put(LK_RIDX_MAINTHREAD, ls)
	registry.put(LK_RIDX_GLOBALS, newLkMap(0, 20).tableBase)

	ls.registry = registry
	ls.pushLuaStack(newLuaStack(LK_MINSTACK, ls))
	return ls
}

func (self *lkState) isMainThread() bool {
	return self.registry.get(LK_RIDX_MAINTHREAD) == self
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
