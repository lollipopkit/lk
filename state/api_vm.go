package state

func (self *lkState) PC() int {
	return self.stack.pc
}

func (self *lkState) AddPC(n int) {
	self.stack.lastPC = self.stack.pc
	self.stack.pc += n
}

func (self *lkState) Fetch() uint32 {
	i := self.stack.closure.proto.Code[self.stack.pc]
	self.stack.lastPC = self.stack.pc
	self.stack.pc++
	return i
}

func (self *lkState) GetConst(idx int) {
	c := self.stack.closure.proto.Constants[idx]
	self.stack.push(c)
}

func (self *lkState) GetRK(rk int) {
	if rk > 0xFF { // constant
		self.GetConst(rk & 0xFF)
	} else { // register
		self.PushValue(rk + 1)
	}
}

func (self *lkState) RegisterCount() int {
	return int(self.stack.closure.proto.MaxStackSize)
}

func (self *lkState) LoadVararg(n int) {
	if n < 0 {
		n = len(self.stack.varargs)
	}

	self.stack.check(n)
	self.stack.pushN(self.stack.varargs, n)
}

func (self *lkState) LoadProto(idx int) {
	stack := self.stack
	subProto := stack.closure.proto.Protos[idx]
	closure := newLuaClosure(subProto)
	stack.push(closure)

	for i := range subProto.Upvalues {
		uvIdx := int(subProto.Upvalues[i].Idx)
		if subProto.Upvalues[i].Instack == 1 {
			if stack.openuvs == nil {
				stack.openuvs = map[int]*any{}
			}

			if openuv, found := stack.openuvs[uvIdx]; found {
				closure.upVals[i] = openuv
			} else {
				closure.upVals[i] = &stack.slots[uvIdx]
				stack.openuvs[uvIdx] = closure.upVals[i]
			}
		} else {
			closure.upVals[i] = stack.closure.upVals[uvIdx]
		}
	}
}

func (self *lkState) CloseUpvalues(a int) {
	for i := range self.stack.openuvs {
		if i >= a-1 {
			val := *self.stack.openuvs[i]
			self.stack.openuvs[i] = &val
			delete(self.stack.openuvs, i)
		}
	}
}
