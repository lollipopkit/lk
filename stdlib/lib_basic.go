package stdlib

import (
	"strconv"
	"strings"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/consts"
	. "github.com/lollipopkit/lk/json"
)

var baseFuncs = map[string]GoFunction{
	"new":       baseNew,
	"print":     basePrint,
	"printf":    basePrintf,
	"fmt":       strFormat,
	"assert":    baseAssert,
	"error":     baseError,
	"errorf":    baseErrorf,
	"iter":      basePairs,
	"next":      baseNext,
	"load":      baseLoad,
	"load_file": baseLoadFile,
	"do_file":   baseDoFile,
	"pcall":     basePCall,
	"type":      baseType,
	"to_str":       baseToString,
	"to_num":       baseToNumber,
	"to_int":       mathToInt,
	"to_map":     baseToJson,
}

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

	ls.NewList()
    ls.CreateTable(0, 10)
    ls.PushValue(-1)
    ls.SetField(-2, "__index")
    ls.SetRegistry("_LIST_MT")
    ls.Pop(1)
    
    ls.NewMap()
    ls.CreateTable(0, 10)
    ls.PushValue(-1)
    ls.SetField(-2, "__index")
    ls.SetRegistry("_MAP_MT")
    ls.Pop(1)
	return 1
}

func baseNew(ls LkState) int {
	ls.CheckType(1, LK_TMAP)
	ls.CreateTable(0, 0)
	ls.PushNil()
	for ls.Next(1) {
		ls.PushValue(-2)
		if ls.IsMap(-2) {
			ls.PushCopyTable(-2)
		} else {
			ls.PushValue(-2)
		}
		ls.SetTable(-5)
		ls.Pop(1)
	}
	return 1
}

func mathToInt(ls LkState) int {
	if i, ok := ls.ToIntegerX(1); ok {
		ls.PushInteger(i)
	} else {
		ls.CheckAny(1)
		ls.PushNil() /* value is not convertible to integer */
	}
	return 1
}

func strFormat(ls LkState) int {
	fmtStr := ls.CheckString(1)
	if len(fmtStr) <= 1 || strings.IndexByte(fmtStr, '%') < 0 {
		ls.PushString(fmtStr)
		return 1
	}

	ls.PushString(_fmt(fmtStr, ls))
	return 1
}

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

func basePrintf(ls LkState) int {
	n := ls.GetTop()
	if n == 0 {
		return 0
	}
	fmtStr := ls.CheckString(1)
	if len(fmtStr) <= 1 || strings.IndexByte(fmtStr, '%') < 0 {
		print(fmtStr)
		return 0
	}

	print(_fmt(fmtStr, ls))
	return 0
}

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

func baseError(ls LkState) int {
	ls.Push(ls.CheckAny(1))
	return ls.Error()
}

func baseErrorf(ls LkState) int {
	fmtStr := ls.CheckString(1)
	ls.PushString(_fmt(fmtStr, ls))
	return ls.Error()
}

func basePairs(ls LkState) int {
	ls.CheckAny(1)
	if ls.GetMetafield(1, "__iter") == LK_TNIL { /* no metamethod? */
		ls.PushGoFunction(baseNext) /* will return generator, */
		ls.PushValue(1)             /* state, */
		ls.PushNil()
	} else {
		ls.PushValue(1) /* argument 'self' to metamethod */
		ls.Call(1, 3)   /* get 3 values from metamethod */
	}
	return 3
}

func baseNext(ls LkState) int {
	ls.CheckType(1, LK_TMAP)
	ls.SetTop(2) /* create a 2nd argument if there isn't one */
	if ls.Next(1) {
		return 2
	} else {
		ls.PushNil()
		return 1
	}
}

func baseLoad(ls LkState) int {
	var status LkStatus
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
func loadAux(ls LkState, status LkStatus, envIdx int) int {
	if status == LK_OK {
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

func baseDoFile(ls LkState) int {
	fname := ls.OptString(1, "bt")
	ls.SetTop(1)
	if ls.LoadFile(fname) != LK_OK {
		ls.PushFString("cannot read file: %s", fname)
		return ls.Error()
	}
	ls.Call(0, LK_MULTRET)
	return ls.GetTop() - 1
}

func basePCall(ls LkState) int {
	nArgs := ls.GetTop() - 1
	status := ls.PCall(nArgs, -1, 0)
	ls.PushBoolean(status == LK_OK)
	ls.Insert(1)
	return ls.GetTop()
}

func baseType(ls LkState) int {
	t := ls.Type(1)
	ls.ArgCheck(t != LK_TNONE, 1, "value expected")
	ls.PushString(ls.TypeName(t))
	return 1
}

func baseToString(ls LkState) int {
	ls.CheckAny(1)
	ls.ToString2(1)
	return 1
}

func baseToNumber(ls LkState) int {
	if ls.IsNoneOrNil(2) { /* standard conversion? */
		ls.CheckAny(1)
		if ls.Type(1) == LK_TNUMBER { /* already a number? */
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
		ls.CheckType(1, LK_TSTRING) /* no numbers as strings */
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

func baseToJson(ls LkState) int {
	str := ls.CheckString(1)
	var item any
	if err := Json.UnmarshalFromString(str, &item); err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	pushValue(ls, item)
	ls.PushNil()
	return 2
}
