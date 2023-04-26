package state

import (
	"fmt"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/term"
	"github.com/lollipopkit/lk/vm"
)

// [-(nargs+1), +nresults, e]
// http://www.lua.org/manual/5.3/manual.html#lua_call
func (self *lkState) Call(nArgs, nResults int) {
	idx := -(nArgs + 1)
	val := self.stack.get(idx)

	c, ok := val.(*lkClosure)
	if !ok {
		if mf := getMetafield(val, "__call", self); mf != nil {
			if c, ok = mf.(*lkClosure); ok {
				self.stack.push(val)
				self.Insert(-(nArgs + 2))
				nArgs += 1
			}
		}
	}

	if ok {
		if c.proto != nil {
			self.callLuaClosure(nArgs, nResults, c)
		} else {
			self.callGoClosure(nArgs, nResults, c)
		}
	} else {
		panic(fmt.Sprintf("attempt to call on %s", self.ToString2(idx)))
	}
}

func (self *lkState) callGoClosure(nArgs, nResults int, c *lkClosure) {
	// create new lua stack
	newStack := newLuaStack(nArgs+LK_MINSTACK, self)
	newStack.closure = c

	// pass args, pop func
	if nArgs > 0 {
		args := self.stack.popN(nArgs)
		newStack.pushN(args, nArgs)
	}
	self.stack.pop()

	// run closure
	self.pushLuaStack(newStack)
	r := c.goFunc(self)
	self.popLuaStack()

	// return results
	if nResults != 0 {
		results := newStack.popN(r)
		self.stack.check(len(results))
		self.stack.pushN(results, nResults)
	}
}

func (self *lkState) callLuaClosure(nArgs, nResults int, c *lkClosure) {
	nRegs := int(c.proto.MaxStackSize)
	nParams := int(c.proto.NumParams)
	isVararg := c.proto.IsVararg == 1

	// create new lua stack
	newStack := newLuaStack(nRegs+LK_MINSTACK, self)
	newStack.closure = c

	// pass args, pop func
	funcAndArgs := self.stack.popN(nArgs + 1)
	newStack.pushN(funcAndArgs[1:], nParams)
	newStack.top = nRegs
	if nArgs > nParams && isVararg {
		newStack.varargs = funcAndArgs[nParams+1:]
	}

	// run closure
	self.pushLuaStack(newStack)
	self.runLuaClosure()
	self.popLuaStack()

	// return results
	if nResults != 0 {
		results := newStack.popN(newStack.top - nRegs)
		self.stack.check(len(results))
		self.stack.pushN(results, nResults)
	}
}

func (self *lkState) runLuaClosure() {
	for {
		inst := vm.Instruction(self.Fetch())
		inst.Execute(self)
		if inst.Opcode() == vm.OP_RETURN {
			break
		}
	}
}

func (self *lkState) CatchAndPrint() {
	if err := recover(); err != nil {
		stack := self.stack
		for stack != nil && stack.closure == nil {
			stack = stack.prev
		}
		line := func() uint32 {
			if stack == nil || stack.closure == nil || stack.closure.proto == nil {
				return 0
			}
			if stack.closure.proto.LineInfo != nil && stack.pc > 0 {
				return stack.closure.proto.LineInfo[stack.pc-1]
			}
			return 0
		}()
		source := func() string {
			if stack == nil || stack.closure == nil || stack.closure.proto == nil {
				return ""
			}
			return stack.closure.proto.Source
		}()
		tip := func() string {
			if source != "" && line != 0 {
				return fmt.Sprintf("[%s:%d] ", source, line)
			}
			return ""
		}()
		errStr := fmt.Sprintf("%s%v", tip, err)
		term.Red(errStr)
	}
}

// Calls a function in protected mode.
// http://www.lua.org/manual/5.3/manual.html#lua_pcall
func (self *lkState) PCall(nArgs, nResults, msgh int) (status LkStatus) {
	caller := self.stack
	status = LK_ERRRUN

	// catch error
	defer func() {
		if err := recover(); err != nil {
			if msgh != 0 {
				panic(err)
			}
			for self.stack != caller {
				self.popLuaStack()
			}
			self.stack.push(err)
		}
	}()

	self.Call(nArgs, nResults)
	status = LK_OK
	return
}
