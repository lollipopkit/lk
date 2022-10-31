package state

import (
	"fmt"
	"io/ioutil"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/stdlib"
)

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_error
func (self *lkState) Error2(fmt string, a ...interface{}) int {
	self.PushFString(fmt, a...) // todo
	return self.Error()
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_argerror
func (self *lkState) ArgError(arg int, extraMsg string) int {
	// bad argument #arg to 'funcname' (extramsg)
	return self.Error2("bad argument #%d (%s)", arg, extraMsg) // todo
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checkstack
func (self *lkState) CheckStack2(sz int, msg string) {
	if !self.CheckStack(sz) {
		if msg != "" {
			self.Error2("stack overflow (%s)", msg)
		} else {
			self.Error2("stack overflow")
		}
	}
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_argcheck
func (self *lkState) ArgCheck(cond bool, arg int, extraMsg string) {
	if !cond {
		self.ArgError(arg, extraMsg)
	}
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checkany
func (self *lkState) CheckAny(arg int) {
	if self.Type(arg) == LUA_TNONE {
		self.ArgError(arg, "value expected")
	}
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checktype
func (self *lkState) CheckType(arg int, t LkType) {
	if self.Type(arg) != t {
		self.tagError(arg, t)
	}
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checkinteger
func (self *lkState) CheckInteger(arg int) int64 {
	i, ok := self.ToIntegerX(arg)
	if !ok {
		self.intError(arg)
	}
	return i
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checknumber
func (self *lkState) CheckNumber(arg int) float64 {
	f, ok := self.ToNumberX(arg)
	if !ok {
		self.tagError(arg, LUA_TNUMBER)
	}
	return f
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_checkstring
// http://www.lua.org/manual/5.3/manual.html#luaL_checklstring
func (self *lkState) CheckString(arg int) string {
	s, ok := self.ToStringX(arg)
	if !ok {
		self.tagError(arg, LUA_TSTRING)
	}
	return s
}

func (self *lkState) CheckBool(arg int) bool {
	if self.Type(arg) != LUA_TBOOLEAN {
		self.tagError(arg, LUA_TBOOLEAN)
	}
	return self.ToBoolean(arg)
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_optinteger
func (self *lkState) OptInteger(arg int, def int64) int64 {
	if self.IsNoneOrNil(arg) {
		return def
	}
	return self.CheckInteger(arg)
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_optnumber
func (self *lkState) OptNumber(arg int, def float64) float64 {
	if self.IsNoneOrNil(arg) {
		return def
	}
	return self.CheckNumber(arg)
}

// [-0, +0, v]
// http://www.lua.org/manual/5.3/manual.html#luaL_optstring
func (self *lkState) OptString(arg int, def string) string {
	if self.IsNoneOrNil(arg) {
		return def
	}
	return self.CheckString(arg)
}

func (self *lkState) OptBool(arg int, def bool) bool {
	if self.IsNoneOrNil(arg) {
		return def
	}
	return self.ToBoolean(arg)
}

// [-0, +?, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_dofile
func (self *lkState) DoFile(filename string) bool {
	return self.LoadFile(filename) != LUA_OK ||
		self.PCall(0, LUA_MULTRET, 0, false) != LUA_OK
}

// [-0, +?, –]
// http://www.lua.org/manual/5.3/manual.html#luaL_dostring
func (self *lkState) DoString(str, source string) bool {
	return self.LoadString(str, source) != LUA_OK ||
		self.PCall(0, LUA_MULTRET, 0, false) != LUA_OK
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#luaL_loadfile
func (self *lkState) LoadFile(filename string) int {
	return self.LoadFileX(filename, "bt")
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#luaL_loadfilex
func (self *lkState) LoadFileX(filename, mode string) int {
	if data, err := ioutil.ReadFile(filename); err == nil {
		return self.Load(data, "@"+filename, mode)
	}
	return LUA_ERRFILE
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#luaL_loadstring
func (self *lkState) LoadString(s, source string) int {
	return self.Load([]byte(s), source, "bt")
}

// [-0, +0, –]
// http://www.lua.org/manual/5.3/manual.html#luaL_typename
func (self *lkState) TypeName2(idx int) string {
	return self.TypeName(self.Type(idx))
}

// [-0, +0, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_len
func (self *lkState) Len2(idx int) int64 {
	self.Len(idx)
	i, isNum := self.ToIntegerX(-1)
	if !isNum {
		self.Error2("object length is not an integer")
	}
	self.Pop(1)
	return i
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_tolstring
func (self *lkState) ToString2(idx int) string {
	if self.CallMeta(idx, "__str") { /* metafield? */
		if !self.IsString(-1) {
			self.Error2("'__str' must return a string")
		}
	} else {
		switch self.Type(idx) {
		case LUA_TNUMBER:
			if self.IsInteger(idx) {
				self.PushString(fmt.Sprintf("%d", self.ToInteger(idx))) // todo
			} else {
				self.PushString(fmt.Sprintf("%g", self.ToNumber(idx))) // todo
			}
		case LUA_TSTRING:
			self.PushValue(idx)
		case LUA_TBOOLEAN:
			if self.ToBoolean(idx) {
				self.PushString("true")
			} else {
				self.PushString("false")
			}
		case LUA_TNIL:
			self.PushString("nil")
		case LUA_TTABLE:
			tb, ok := self.ToPointer(idx).(*lkTable)
			if ok {
				s, err := tb.String()
				if err == nil {
					if s == "null" {
						self.PushString("{}")
					} else {
						self.PushString(s)
					}
				} else {
					panic(err)
				}
			}
		default:
			tt := self.GetMetafield(idx, "__name") /* try name */
			var kind string
			if tt == LUA_TSTRING {
				kind = self.CheckString(-1)
			} else {
				kind = self.TypeName2(idx)
			}

			self.PushString(fmt.Sprintf("%s: %v", kind, self.ToPointer(idx)))

			if tt != LUA_TNIL {
				self.Remove(-2) /* remove '__name' */
			}
		}
	}
	return self.CheckString(-1)
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_getsubtable
func (self *lkState) GetSubTable(idx int, fname string) bool {
	if self.GetField(idx, fname) == LUA_TTABLE {
		return true /* table already there */
	}
	self.Pop(1) /* remove previous result */
	idx = self.stack.absIndex(idx)
	self.NewTable()
	self.PushValue(-1)        /* copy to be left at top */
	self.SetField(idx, fname) /* assign new table to field */
	return false              /* false, because did not find table there */
}

// [-0, +(0|1), m]
// http://www.lua.org/manual/5.3/manual.html#luaL_getmetafield
func (self *lkState) GetMetafield(obj int, event string) LkType {
	if !self.GetMetatable(obj) { /* no metatable? */
		return LUA_TNIL
	}

	self.PushString(event)
	tt := self.RawGet(-2)
	if tt == LUA_TNIL { /* is metafield nil? */
		self.Pop(2) /* remove metatable and metafield */
	} else {
		self.Remove(-2) /* remove only metatable */
	}
	return tt /* return metafield type */
}

// [-0, +(0|1), e]
// http://www.lua.org/manual/5.3/manual.html#luaL_callmeta
func (self *lkState) CallMeta(obj int, event string) bool {
	obj = self.AbsIndex(obj)
	if self.GetMetafield(obj, event) == LUA_TNIL { /* no metafield? */
		return false
	}

	self.PushValue(obj)
	self.Call(1, 1)
	return true
}

// [-0, +0, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_openlibs
func (self *lkState) OpenLibs() {
	libs := map[string]GoFunction{
		"_G":   stdlib.OpenBaseLib,
		"math": stdlib.OpenMathLib,
		"strs": stdlib.OpenStringLib,
		"utf8": stdlib.OpenUTF8Lib,
		"os":   stdlib.OpenOSLib,
		"pkg":  stdlib.OpenPackageLib,
		"sync": stdlib.OpenCoroutineLib,
		"http": stdlib.OpenHttpLib,
		"json": stdlib.OpenJsonLib,
		"re":   stdlib.OpenReLib,
		"rand": stdlib.OpenRandLib,
		"table":stdlib.OpenTableLib,
		"nums": stdlib.OpenNumLib,
	}

	for name := range libs {
		self.RequireF(name, libs[name], true)
		self.Pop(1)
	}
}

// [-0, +1, e]
// http://www.lua.org/manual/5.3/manual.html#luaL_requiref
func (self *lkState) RequireF(modname string, openf GoFunction, glb bool) {
	self.GetSubTable(LUA_REGISTRYINDEX, "_LOADED")
	self.GetField(-1, modname) /* LOADED[modname] */
	if !self.ToBoolean(-1) {   /* package not already loaded? */
		self.Pop(1) /* remove field */
		self.PushGoFunction(openf)
		self.PushString(modname)   /* argument to open function */
		self.Call(1, 1)            /* call 'openf' to open module */
		self.PushValue(-1)         /* make copy of module (call result) */
		self.SetField(-3, modname) /* _LOADED[modname] = module */
	}
	self.Remove(-2) /* remove _LOADED table */
	if glb {
		self.PushValue(-1)      /* copy of module */
		self.SetGlobal(modname) /* _G[modname] = module */
	}
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#luaL_newlib
func (self *lkState) NewLib(l FuncReg) {
	self.NewLibTable(l)
	self.SetFuncs(l, 0)
}

// [-0, +1, m]
// http://www.lua.org/manual/5.3/manual.html#luaL_newlibtable
func (self *lkState) NewLibTable(l FuncReg) {
	self.CreateTable(0, len(l))
}

// [-nup, +0, m]
// http://www.lua.org/manual/5.3/manual.html#luaL_setfuncs
func (self *lkState) SetFuncs(l FuncReg, nup int) {
	self.CheckStack2(nup, "too many upvalues")
	for name := range l { /* fill the table with given functions */
		for i := 0; i < nup; i++ { /* copy upvalues to the top */
			self.PushValue(-nup)
		}
		// r[-(nup+2)][name]=fun
		self.PushGoClosure(l[name], nup) /* closure with those upvalues */
		self.SetField(-(nup + 2), name)
	}
	self.Pop(nup) /* remove upvalues */
}

func (self *lkState) intError(arg int) {
	if self.IsNumber(arg) {
		self.ArgError(arg, "number has no integer representation")
	} else {
		self.tagError(arg, LUA_TNUMBER)
	}
}

func (self *lkState) tagError(arg int, tag LkType) {
	self.typeError(arg, self.TypeName(LkType(tag)))
}

func (self *lkState) typeError(arg int, tname string) int {
	var typeArg string /* name for the type of the actual argument */
	if self.GetMetafield(arg, "__name") == LUA_TSTRING {
		typeArg = self.ToString(-1) /* use the given type name */
	} else if self.Type(arg) == LUA_TLIGHTUSERDATA {
		typeArg = "light userdata" /* special name for messages */
	} else {
		typeArg = self.TypeName2(arg) /* standard name */
	}
	msg := tname + " expected, got " + typeArg
	self.PushString(msg)
	return self.ArgError(arg, msg)
}
