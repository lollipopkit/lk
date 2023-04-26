package state

import . "github.com/lollipopkit/lk/api"

type lkState struct {
	registry *lkTable
	stack    *lkStack
	/* coroutine */
	coStatus LkStatus
	coCaller *lkState
	coChan   chan int
}

func New() LkState {
	ls := &lkState{}

	registry := newLkTable(8, 0)
	registry.put(LK_RIDX_MAINTHREAD, ls)
	registry.put(LK_RIDX_GLOBALS, newLkTable(0, 20))

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
