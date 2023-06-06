package state

import (
	"fmt"
	"os"
	"strings"

	"github.com/lollipopkit/gommon/log"
	"github.com/lollipopkit/gommon/util"
	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/consts"
	"github.com/lollipopkit/lk/mods"
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
		panic(fmt.Sprintf("attempt to call on %#v", val))
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
		log.Red("%v\n", err)
		stack := self.stack
		for stack.prev != nil {
			_catchEachStack(stack)
			stack = stack.prev
		}
	}
}

func _catchEachStack(stack *lkStack) {
	if stack == nil || stack.closure == nil || stack.closure.proto == nil {
		return
	}
	line := func() uint32 {
		if stack.closure.proto.LineInfo != nil && stack.pc > 0 {
			return stack.closure.proto.LineInfo[stack.pc-1]
		}
		return 0
	}()
	source := stack.closure.proto.Source
	code := func() string {
		var data []byte
		var err error
		if strings.HasPrefix(source, consts.BuiltinPrefix) {
			data, err = mods.Files.ReadFile(source[consts.BuiltinPrefixLen:])
		} else if util.Exist(source) {
			data, err = os.ReadFile(source)
		}

		if data == nil || len(data) == 0 || err != nil {
			return ""
		}
		splited := strings.Split(string(data), "\n")
		if int(line) > len(splited) {
			return fmt.Sprintf("Find code: out of range: line %d >= file len %d", line, len(splited))
		}
		return strings.TrimSpace(splited[line-1])
	}()
	if source != "" {
		log.Yellow(">> %s:%d\n", source, line)
		if len(code) != 0 {
			println("  " + code)
		}
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
