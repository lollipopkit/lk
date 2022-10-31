package state

import . "git.lolli.tech/lollipopkit/lk/api"

type lkStack struct {
	/* virtual stack */
	slots []any
	top   int
	/* call info */
	state   *lkState
	closure *closure
	varargs []any
	openuvs map[int]*any
	pc      int
	/* linked list */
	prev *lkStack
}

func newLuaStack(size int, state *lkState) *lkStack {
	return &lkStack{
		slots: make([]any, size),
		top:   0,
		state: state,
	}
}

func (self *lkStack) check(n int) {
	free := len(self.slots) - self.top
	for i := free; i < n; i++ {
		self.slots = append(self.slots, nil)
	}
}

func (self *lkStack) push(val any) {
	if self.top == len(self.slots) {
		panic("stack overflow!")
	}
	self.slots[self.top] = val
	self.top++
}

func (self *lkStack) pop() any {
	if self.top < 1 {
		panic("stack underflow!")
	}
	self.top--
	val := self.slots[self.top]
	self.slots[self.top] = nil
	return val
}

func (self *lkStack) pushN(vals []any, n int) {
	nVals := len(vals)
	if n < 0 {
		n = nVals
	}

	for i := 0; i < n; i++ {
		if i < nVals {
			self.push(vals[i])
		} else {
			self.push(nil)
		}
	}
}

func (self *lkStack) popN(n int) []any {
	vals := make([]any, n)
	for i := n - 1; i >= 0; i-- {
		vals[i] = self.pop()
	}
	return vals
}

func (self *lkStack) absIndex(idx int) int {
	if idx >= 0 || idx <= LUA_REGISTRYINDEX {
		return idx
	}
	return idx + self.top + 1
}

func (self *lkStack) isValid(idx int) bool {
	if idx < LUA_REGISTRYINDEX { /* upvalues */
		uvIdx := LUA_REGISTRYINDEX - idx - 1
		c := self.closure
		return c != nil && uvIdx < len(c.upVals)
	}
	if idx == LUA_REGISTRYINDEX {
		return true
	}
	absIdx := self.absIndex(idx)
	return absIdx > 0 && absIdx <= self.top
}

func (self *lkStack) get(idx int) any {
	if idx < LUA_REGISTRYINDEX { /* upvalues */
		uvIdx := LUA_REGISTRYINDEX - idx - 1
		c := self.closure
		if c == nil || uvIdx >= len(c.upVals) {
			return nil
		}
		return *(c.upVals[uvIdx])
	}

	if idx == LUA_REGISTRYINDEX {
		return self.state.registry
	}

	absIdx := self.absIndex(idx)
	if absIdx > 0 && absIdx <= self.top {
		return self.slots[absIdx-1]
	}
	return nil
}

func (self *lkStack) set(idx int, val any) {
	if idx < LUA_REGISTRYINDEX { /* upvalues */
		uvIdx := LUA_REGISTRYINDEX - idx - 1
		c := self.closure
		if c != nil && uvIdx < len(c.upVals) {
			c.upVals[uvIdx] = &val
		}
		return
	}

	if idx == LUA_REGISTRYINDEX {
		self.state.registry = val.(*lkTable)
		return
	}

	absIdx := self.absIndex(idx)
	if absIdx > 0 && absIdx <= self.top {
		self.slots[absIdx-1] = val
		return
	}
	panic("invalid index!")
}

func (self *lkStack) reverse(from, to int) {
	slots := self.slots
	for from < to {
		slots[from], slots[to] = slots[to], slots[from]
		from++
		to--
	}
}
