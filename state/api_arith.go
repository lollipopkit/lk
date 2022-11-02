package state

import (
	"fmt"
	"math"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/utils"
)

type operator struct {
	metamethod  string
	integerFunc func(int64, int64) int64
	floatFunc   func(float64, float64) float64
}

var (
	iadd  = func(a, b int64) int64 { return a + b }
	fadd  = func(a, b float64) float64 { return a + b }
	isub  = func(a, b int64) int64 { return a - b }
	fsub  = func(a, b float64) float64 { return a - b }
	imul  = func(a, b int64) int64 { return a * b }
	fmul  = func(a, b float64) float64 { return a * b }
	imod  = utils.IMod
	fmod  = utils.FMod
	pow   = math.Pow
	div   = func(a, b float64) float64 { return a / b }
	iidiv = utils.IFloorDiv
	fidiv = utils.FFloorDiv
	band  = func(a, b int64) int64 { return a & b }
	bor   = func(a, b int64) int64 { return a | b }
	bxor  = func(a, b int64) int64 { return a ^ b }
	shl   = utils.ShiftLeft
	shr   = utils.ShiftRight
	iunm  = func(a, _ int64) int64 { return -a }
	funm  = func(a, _ float64) float64 { return -a }
	bnot  = func(a, _ int64) int64 { return ^a }
)

var operators = []operator{
	{"__add", iadd, fadd},
	{"__sub", isub, fsub},
	{"__mul", imul, fmul},
	{"__mod", imod, fmod},
	{"__pow", nil, pow},
	{"__div", nil, div},
	{"__idiv", iidiv, fidiv},
	{"__band", band, nil},
	{"__bor", bor, nil},
	{"__bxor", bxor, nil},
	{"__shl", shl, nil},
	{"__shr", shr, nil},
	{"__unm", iunm, funm},
	{"__bnot", bnot, nil},
}

func opSymbol(opName string) string {
	switch opName {
	case "__add":
		return "+"
	case "__sub":
		return "-"
	case "__mul":
		return "*"
	case "__mod":
		return "%"
	case "__pow":
		return "^"
	case "__div":
		return "/"
	case "__idiv":
		return "~/"
	case "__band":
		return "and"
	case "__bor":
		return "or"
	case "__bxor":
		return "xor"
	case "__shl":
		return "<<"
	case "__shr":
		return ">>"
	case "__unm":
		return "-"
	case "__bnot":
		return "not"
	default:
		return opName
	}
}

// [-(2|1), +1, e]
// http://www.lua.org/manual/5.3/manual.html#lua_arith
func (self *lkState) Arith(op ArithOp) {
	var a, b any // operands
	b = self.stack.pop()
	if op != LK_OPUNM && op != LK_OPBNOT {
		a = self.stack.pop()
	} else {
		a = b
	}

	operator := operators[op]
	if result := _arith(a, b, operator); result != nil {
		self.stack.push(result)
		return
	}

	mm := operator.metamethod
	if result, ok := callMetamethod(a, b, mm, self); ok {
		self.stack.push(result)
		return
	}

	if a == nil && b == nil {
		self.PushNil()
		return
	}

	as, oka := a.(string)
	bs, okb := b.(string)
	if oka && okb {
		switch op {
		case LK_OPADD:
			self.stack.push(as + bs)
			return
		}
	}

	at, oka := a.(*lkTable)
	bt, okb := b.(*lkTable)
	if oka && okb {
		switch op {
		case LK_OPADD:
			at.combine(bt)
			self.stack.push(at)
			return
		}
	}
	panic(fmt.Sprintf("invalid arith: %T %s %T", a, opSymbol(mm), b))
}

func _arith(a, b any, op operator) any {
	if op.floatFunc == nil { // bitwise
		if x, ok := convertToInteger(a); ok {
			if y, ok := convertToInteger(b); ok {
				return op.integerFunc(x, y)
			}
		}
	} else { // arith
		if op.integerFunc != nil { // add,sub,mul,mod,idiv,unm
			if x, ok := a.(int64); ok {
				if y, ok := b.(int64); ok {
					return op.integerFunc(x, y)
				}
			}
		}
		if x, ok := convertToFloat(a); ok {
			if y, ok := convertToFloat(b); ok {
				return op.floatFunc(x, y)
			}
		}
	}
	return nil
}
