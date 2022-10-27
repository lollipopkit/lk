package stdlib

import (
	"strconv"
	"strings"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/term"
)

var baseFuncs = map[string]GoFunction{
	"new":       baseNew,
	"print":     basePrint,
	"input":     baseInput,
	"assert":    baseAssert,
	"error":     baseError,
	"irange":    baseIPairs,
	"range":     basePairs,
	"next":      baseNext,
	"load":      baseLoad,
	"load_file": baseLoadFile,
	"do_file":   baseDoFile,
	"pcall":     basePCall,
	// "rawget":       baseRawGet,
	// "rawset":       baseRawSet,
	"type": baseType,
	"str":  baseToString,
	"num":  baseToNumber,
	"int":  mathToInt,
	"kv":   baseKV,
	// string
	"fmt": strFormat,
}

// lua-5.3.4/src/lbaselib.c#luaopen_base()
func OpenBaseLib(ls LkState) int {
	/* open lib into global table */
	ls.PushGlobalTable()
	ls.SetFuncs(baseFuncs, 0)
	/* set global _G */
	ls.PushValue(-1)
	ls.SetField(-2, "_G")
	/* set global _VERSION */
	ls.PushString(consts.VERSION)
	ls.SetField(-2, "_VERSION")
	return 1
}

func baseNew(ls LkState) int {
	ls.CheckType(1, LUA_TTABLE)
	ls.CreateTable(0, 0)
	ls.PushNil()
	for ls.Next(1) {
		ls.PushValue(-2)
		ls.PushValue(-2)
		ls.SetTable(-5)
		ls.Pop(1)
	}
	return 1
}

func baseInput(ls LkState) int {
	ls.PushString(term.ReadLine([]string{}))
	return 1
}

// int (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.tointeger
// lua-5.3.4/src/lmathlib.c#math_toint()
func mathToInt(ls LkState) int {
	if i, ok := ls.ToIntegerX(1); ok {
		ls.PushInteger(i)
	} else {
		ls.CheckAny(1)
		ls.PushNil() /* value is not convertible to integer */
	}
	return 1
}

func baseKV(ls LkState) int {
	tb := getTable(&ls, 1)
	keys := make([]any, 0, len(tb))
	for k := range tb {
		keys = append(keys, k)
	}
	values := make([]any, 0, len(tb))
	for k := range tb {
		values = append(values, tb[k])
	}
	pushList(&ls, keys)
	pushList(&ls, values)
	return 2
}

// format (formatstring, ···)
// http://www.lua.org/manual/5.3/manual.html#pdf-string.format
func strFormat(ls LkState) int {
	fmtStr := ls.CheckString(1)
	if len(fmtStr) <= 1 || strings.IndexByte(fmtStr, '%') < 0 {
		ls.PushString(fmtStr)
		return 1
	}

	argIdx := 1
	arr := parseFmtStr(fmtStr)
	for i := range arr {
		if arr[i][0] == '%' {
			if arr[i] == "%%" {
				arr[i] = "%"
			} else {
				argIdx += 1
				arr[i] = _fmtArg(arr[i], ls, argIdx)
			}
		}
	}

	ls.PushString(strings.Join(arr, ""))
	return 1
}

// print (···)
// http://www.lua.org/manual/5.3/manual.html#pdf-print
// lua-5.3.4/src/lbaselib.c#luaB_print()
func basePrint(ls LkState) int {
	n := ls.GetTop() /* number of arguments */
	for i := 1; i <= n; i++ {
		if i > 1 {
			print("\t")
		}
		print(ls.ToString2(i))
		ls.Pop(1) /* pop result */
	}
	println()
	return 0
}

// assert (v [, message])
// http://www.lua.org/manual/5.3/manual.html#pdf-assert
// lua-5.3.4/src/lbaselib.c#luaB_assert()
func baseAssert(ls LkState) int {
	if ls.ToBoolean(1) { /* condition is true? */
		return ls.GetTop() /* return all arguments */
	} else { /* error */
		ls.CheckAny(1)                     /* there must be a condition */
		ls.Remove(1)                       /* remove it */
		ls.PushString("assertion failed!") /* default message */
		ls.SetTop(1)                       /* leave only message (default if no other one) */
		return baseError(ls)               /* call 'error' */
	}
}

// error (message [, level])
// http://www.lua.org/manual/5.3/manual.html#pdf-error
// lua-5.3.4/src/lbaselib.c#luaB_error()
func baseError(ls LkState) int {
	level := int(ls.OptInteger(2, 1))
	ls.SetTop(1)
	if ls.Type(1) == LUA_TSTRING && level > 0 {
		// ls.Where(level) /* add extra information */
		// ls.PushValue(1)
		// ls.Concat(2)
	}
	return ls.Error()
}

// ipairs (t)
// http://www.lua.org/manual/5.3/manual.html#pdf-ipairs
// lua-5.3.4/src/lbaselib.c#luaB_ipairs()
func baseIPairs(ls LkState) int {
	ls.CheckAny(1)
	ls.PushGoFunction(iPairsAux) /* iteration function */
	ls.PushValue(1)              /* state */
	ls.PushInteger(0)            /* initial value */
	return 3
}

func iPairsAux(ls LkState) int {
	i := ls.CheckInteger(2) + 1
	ls.PushInteger(i)
	if ls.GetI(1, i) == LUA_TNIL {
		return 1
	} else {
		return 2
	}
}

// pairs (t)
// http://www.lua.org/manual/5.3/manual.html#pdf-pairs
// lua-5.3.4/src/lbaselib.c#luaB_pairs()
func basePairs(ls LkState) int {
	ls.CheckAny(1)
	if ls.GetMetafield(1, "__range") == LUA_TNIL { /* no metamethod? */
		ls.PushGoFunction(baseNext) /* will return generator, */
		ls.PushValue(1)             /* state, */
		ls.PushNil()
	} else {
		ls.PushValue(1) /* argument 'self' to metamethod */
		ls.Call(1, 3)   /* get 3 values from metamethod */
	}
	return 3
}

// next (table [, index])
// http://www.lua.org/manual/5.3/manual.html#pdf-next
// lua-5.3.4/src/lbaselib.c#luaB_next()
func baseNext(ls LkState) int {
	ls.CheckType(1, LUA_TTABLE)
	ls.SetTop(2) /* create a 2nd argument if there isn't one */
	if ls.Next(1) {
		return 2
	} else {
		ls.PushNil()
		return 1
	}
}

// load (chunk [, chunkname [, mode [, env]]])
// http://www.lua.org/manual/5.3/manual.html#pdf-load
// lua-5.3.4/src/lbaselib.c#luaB_load()
func baseLoad(ls LkState) int {
	var status int
	chunk, isStr := ls.ToStringX(1)
	mode := ls.OptString(3, "bt")
	env := 0 /* 'env' index or 0 if no 'env' */
	if !ls.IsNone(4) {
		env = 4
	}
	if isStr { /* loading a string? */
		chunkname := ls.OptString(2, chunk)
		status = ls.Load([]byte(chunk), chunkname, mode)
	} else { /* loading from a reader function */
		panic("loading from a reader function") // todo
	}
	return loadAux(ls, status, env)
}

// lua-5.3.4/src/lbaselib.c#load_aux()
func loadAux(ls LkState, status, envIdx int) int {
	if status == LUA_OK {
		if envIdx != 0 { /* 'env' parameter? */
			panic("todo!")
		}
		return 1
	} else { /* error (message is on top of the stack) */
		ls.PushNil()
		ls.Insert(-2) /* put before error message */
		return 2      /* return nil plus error message */
	}
}

// loadfile ([filename [, mode [, env]]])
// http://www.lua.org/manual/5.3/manual.html#pdf-loadfile
// lua-5.3.4/src/lbaselib.c#luaB_loadfile()
func baseLoadFile(ls LkState) int {
	fname := ls.OptString(1, "")
	mode := ls.OptString(1, "bt")
	env := 0 /* 'env' index or 0 if no 'env' */
	if !ls.IsNone(3) {
		env = 3
	}
	status := ls.LoadFileX(fname, mode)
	return loadAux(ls, status, env)
}

// dofile ([filename])
// http://www.lua.org/manual/5.3/manual.html#pdf-dofile
// lua-5.3.4/src/lbaselib.c#luaB_dofile()
func baseDoFile(ls LkState) int {
	fname := ls.OptString(1, "bt")
	ls.SetTop(1)
	if ls.LoadFile(fname) != LUA_OK {
		return ls.Error()
	}
	ls.Call(0, LUA_MULTRET)
	return ls.GetTop() - 1
}

// pcall (f [, arg1, ···])
// http://www.lua.org/manual/5.3/manual.html#pdf-pcall
func basePCall(ls LkState) int {
	nArgs := ls.GetTop() - 1
	status := ls.PCall(nArgs, -1, 0, false)
	ls.PushBoolean(status == LUA_OK)
	ls.Insert(1)
	return ls.GetTop()
}

// rawget (table, index)
// http://www.lua.org/manual/5.3/manual.html#pdf-rawget
// lua-5.3.4/src/lbaselib.c#luaB_rawget()
func baseRawGet(ls LkState) int {
	ls.CheckType(1, LUA_TTABLE)
	ls.CheckAny(2)
	ls.SetTop(2)
	ls.RawGet(1)
	return 1
}

// rawset (table, index, value)
// http://www.lua.org/manual/5.3/manual.html#pdf-rawset
// lua-5.3.4/src/lbaselib.c#luaB_rawset()
func baseRawSet(ls LkState) int {
	ls.CheckType(1, LUA_TTABLE)
	ls.CheckAny(2)
	ls.CheckAny(3)
	ls.SetTop(3)
	ls.RawSet(1)
	return 1
}

// type (v)
// http://www.lua.org/manual/5.3/manual.html#pdf-type
// lua-5.3.4/src/lbaselib.c#luaB_type()
func baseType(ls LkState) int {
	t := ls.Type(1)
	ls.ArgCheck(t != LUA_TNONE, 1, "value expected")
	ls.PushString(ls.TypeName(t))
	return 1
}

// str (v)
// http://www.lua.org/manual/5.3/manual.html#pdf-tostring
// lua-5.3.4/src/lbaselib.c#luaB_tostring()
func baseToString(ls LkState) int {
	ls.CheckAny(1)
	ls.ToString2(1)
	return 1
}

// num (e [, base])
// http://www.lua.org/manual/5.3/manual.html#pdf-tonumber
// lua-5.3.4/src/lbaselib.c#luaB_tonumber()
func baseToNumber(ls LkState) int {
	if ls.IsNoneOrNil(2) { /* standard conversion? */
		ls.CheckAny(1)
		if ls.Type(1) == LUA_TNUMBER { /* already a number? */
			ls.SetTop(1) /* yes; return it */
			return 1
		} else {
			if s, ok := ls.ToStringX(1); ok {
				if ls.StringToNumber(s) {
					return 1 /* successful conversion to number */
				} /* else not a number */
			}
		}
	} else {
		ls.CheckType(1, LUA_TSTRING) /* no numbers as strings */
		s := strings.TrimSpace(ls.ToString(1))
		base := int(ls.CheckInteger(2))
		ls.ArgCheck(2 <= base && base <= 36, 2, "base out of range")
		if n, err := strconv.ParseInt(s, base, 64); err == nil {
			ls.PushInteger(n)
			return 1
		} /* else not a number */
	} /* else not a number */
	ls.PushNil() /* not a number */
	return 1
}
