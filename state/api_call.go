package state

import (
	"fmt"
	"io/ioutil"
	"strings"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/binchunk"
	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
	"git.lolli.tech/lollipopkit/lk/vm"
)

func Compile(source string) *binchunk.Prototype {
	if strings.HasPrefix(source, "@") {
		source = source[1:]
	}

	if !utils.Exist(source) {
		term.Error("[compile] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[compile] can't read file: " + err.Error())
	}

	bin := compiler.Compile(string(data), source)

	compiledData, err := bin.Dump(utils.Md5(data))
	if err != nil {
		term.Error("[compile] dump file failed: " + err.Error())
	}
	err = ioutil.WriteFile(source+"c", compiledData, 0744)
	if err != nil {
		term.Error("[compile] write file failed: " + err.Error())
	}
	return bin
}

func loadlk(source string) *binchunk.Prototype {
	lkc := source + "c"
	if utils.Exist(lkc) {
		return loadlkc(lkc)
	}
	return Compile(source)
}

func loadlkc(source string) *binchunk.Prototype {
	if !utils.Exist(source) {
		term.Error("[run] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}

	lkPath := source[:len(source)-1]
	var lkData []byte
	lkExist := utils.Exist(lkPath)
	if lkExist {
		lkData, err = ioutil.ReadFile(lkPath)
		if err != nil {
			term.Error("[run] can't read file: " + err.Error())
		}
	}

	proto, err := binchunk.Verify(data, lkData)
	if err != nil {
		if err == binchunk.ErrMismatchedHash {
			if lkExist {
				term.Info("[run] source changed, recompiling " + lkPath)
				proto = Compile(lkPath)
			} else {
				term.Warn("[run] source not found: " + lkPath)
			}
		} else if strings.HasPrefix(err.Error(), binchunk.MismatchVersionPrefix) {
			if lkExist {
				term.Info("[run] mismatch version, recompiling " + lkPath)
				proto = Compile(lkPath)
			} else {
				term.Error("[run] mismatch version and source not found: " + lkPath)
			}
		} else {
			term.Error("[run] chunk verify failed: " + err.Error())
		}
	}

	return proto
}

func load(file string) *binchunk.Prototype {
	if strings.HasPrefix(file, "@") {
		file = file[1:]
	}
	if strings.HasSuffix(file, ".lk") {
		return loadlk(file)
	} else if strings.HasSuffix(file, ".lkc") {
		return loadlkc(file)
	}
	term.Error("[run] unknown file type: " + file)
	return nil
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_load
func (self *luaState) Load(chunk []byte, chunkName, mode string) int {
	var proto *binchunk.Prototype
	if chunkName == "stdin" {
		proto = compiler.Compile(string(chunk), chunkName)
	} else {
		proto = load(chunkName)
	}

	c := newLuaClosure(proto)
	self.stack.push(c)
	if len(proto.Upvalues) > 0 {
		env := self.registry.get(LUA_RIDX_GLOBALS)
		c.upvals[0] = &upvalue{&env}
	}
	return LUA_OK
}

// [-(nargs+1), +nresults, e]
// http://www.lua.org/manual/5.3/manual.html#lua_call
func (self *luaState) Call(nArgs, nResults int) {
	val := self.stack.get(-(nArgs + 1))

	c, ok := val.(*closure)
	if !ok {
		if mf := getMetafield(val, "__call", self); mf != nil {
			if c, ok = mf.(*closure); ok {
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

func (self *luaState) callGoClosure(nArgs, nResults int, c *closure) {
	// create new lua stack
	newStack := newLuaStack(nArgs+LUA_MINSTACK, self)
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

func (self *luaState) callLuaClosure(nArgs, nResults int, c *closure) {
	nRegs := int(c.proto.MaxStackSize)
	nParams := int(c.proto.NumParams)
	isVararg := c.proto.IsVararg == 1

	// create new lua stack
	newStack := newLuaStack(nRegs+LUA_MINSTACK, self)
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

func (self *luaState) runLuaClosure() {
	for {
		inst := vm.Instruction(self.Fetch())
		inst.Execute(self)
		if inst.Opcode() == vm.OP_RETURN {
			break
		}
	}
}

// Calls a function in protected mode.
// http://www.lua.org/manual/5.3/manual.html#lua_pcall
func (self *luaState) PCall(nArgs, nResults, msgh int, print bool) (status int) {
	caller := self.stack
	status = LUA_ERRRUN

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
			if print {
				fmt.Printf("%v\n", err)
			}
		}
	}()

	self.Call(nArgs, nResults)
	status = LUA_OK
	return
}
