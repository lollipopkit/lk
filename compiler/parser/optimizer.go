package parser

import (
	"math"

	. "github.com/lollipopkit/lk/compiler/ast"
	"github.com/lollipopkit/lk/utils"

	. "github.com/lollipopkit/lk/compiler/lexer"
)

func optimizeLogicalOr(exp *BinopExp) Exp {
	if isTrue(exp.Left) {
		return exp.Left // true or x => true
	}
	if isFalse(exp.Left) && !isVarargOrFuncCall(exp.Right) {
		return exp.Right // false or x => x
	}
	return exp
}

func optimizeLogicalAnd(exp *BinopExp) Exp {
	if isFalse(exp.Left) {
		return exp.Left // false and x => false
	}
	if isTrue(exp.Left) && !isVarargOrFuncCall(exp.Right) {
		return exp.Right // true and x => x
	}
	return exp
}

func optimizeBitwiseBinaryOp(exp *BinopExp) Exp {
	if i, ok := castToInt(exp.Left); ok {
		if j, ok := castToInt(exp.Right); ok {
			switch exp.Op {
			case TOKEN_OP_BAND:
				return &IntegerExp{exp.Line, i & j}
			case TOKEN_OP_BOR:
				return &IntegerExp{exp.Line, i | j}
			case TOKEN_OP_BXOR:
				return &IntegerExp{exp.Line, i ^ j}
			case TOKEN_OP_SHL:
				return &IntegerExp{exp.Line, utils.ShiftLeft(i, j)}
			case TOKEN_OP_SHR:
				return &IntegerExp{exp.Line, utils.ShiftRight(i, j)}
			}
		}
	}
	return exp
}

func optimizeArithBinaryOp(exp *BinopExp) Exp {
	if x, ok := exp.Left.(*IntegerExp); ok {
		if y, ok := exp.Right.(*IntegerExp); ok {
			switch exp.Op {
			case TOKEN_OP_ADD:
				return &IntegerExp{exp.Line, x.Int + y.Int}
			case TOKEN_OP_SUB:
				return &IntegerExp{exp.Line, x.Int - y.Int}
			case TOKEN_OP_MUL:
				return &IntegerExp{exp.Line, x.Int * y.Int}
			case TOKEN_OP_IDIV:
				if y.Int != 0 {
					return &IntegerExp{exp.Line, utils.IFloorDiv(x.Int, y.Int)}
				}
			case TOKEN_OP_MOD:
				if y.Int != 0 {
					return &IntegerExp{exp.Line, utils.IMod(x.Int, y.Int)}
				}
			}
		}
	}
	if f, ok := castToFloat(exp.Left); ok {
		if g, ok := castToFloat(exp.Right); ok {
			switch exp.Op {
			case TOKEN_OP_ADD:
				return &FloatExp{exp.Line, f + g}
			case TOKEN_OP_SUB:
				return &FloatExp{exp.Line, f - g}
			case TOKEN_OP_MUL:
				return &FloatExp{exp.Line, f * g}
			case TOKEN_OP_DIV:
				if g != 0 {
					return &FloatExp{exp.Line, f / g}
				}
			case TOKEN_OP_IDIV:
				if g != 0 {
					return &FloatExp{exp.Line, utils.FFloorDiv(f, g)}
				}
			case TOKEN_OP_MOD:
				if g != 0 {
					return &FloatExp{exp.Line, utils.FMod(f, g)}
				}
			case TOKEN_OP_POW:
				return &FloatExp{exp.Line, math.Pow(f, g)}
			}
		}
	}
	return exp
}

func optimizePow(exp Exp) Exp {
	if binop, ok := exp.(*BinopExp); ok {
		if binop.Op == TOKEN_OP_POW {
			binop.Right = optimizePow(binop.Right)
		}
		return optimizeArithBinaryOp(binop)
	}
	return exp
}

func optimizeUnaryOp(exp *UnopExp) Exp {
	switch exp.Op {
	case TOKEN_OP_UNM:
		return optimizeUnm(exp)
	case TOKEN_OP_NOT:
		return optimizeNot(exp)
	case TOKEN_OP_BNOT:
		return optimizeBnot(exp)
	default:
		return exp
	}
}

func optimizeUnm(exp *UnopExp) Exp {
	switch x := exp.Unop.(type) { // utils?
	case *IntegerExp:
		x.Int = -x.Int
		return x
	case *FloatExp:
		if x.Float != 0 {
			x.Float = -x.Float
			return x
		}
	}
	return exp
}

func optimizeNot(exp *UnopExp) Exp {
	switch exp.Unop.(type) {
	case *NilExp, *FalseExp: // false
		return &TrueExp{exp.Line}
	case *TrueExp, *IntegerExp, *FloatExp, *StringExp: // true
		return &FalseExp{exp.Line}
	default:
		return exp
	}
}

func optimizeBnot(exp *UnopExp) Exp {
	switch x := exp.Unop.(type) { // utils?
	case *IntegerExp:
		x.Int = ^x.Int
		return x
	case *FloatExp:
		if i, ok := utils.FloatToInteger(x.Float); ok {
			return &IntegerExp{x.Line, ^i}
		}
	}
	return exp
}

func isFalse(exp Exp) bool {
	switch exp.(type) {
	case *FalseExp, *NilExp:
		return true
	default:
		return false
	}
}

func isTrue(exp Exp) bool {
	switch exp.(type) {
	case *TrueExp, *IntegerExp, *FloatExp, *StringExp:
		return true
	default:
		return false
	}
}

// todo
func isVarargOrFuncCall(exp Exp) bool {
	switch exp.(type) {
	case *VarargExp, *FuncCallExp:
		return true
	}
	return false
}

func castToInt(exp Exp) (int64, bool) {
	switch x := exp.(type) {
	case *IntegerExp:
		return x.Int, true
	case *FloatExp:
		return utils.FloatToInteger(x.Float)
	default:
		return 0, false
	}
}

func castToFloat(exp Exp) (float64, bool) {
	switch x := exp.(type) {
	case *IntegerExp:
		return float64(x.Int), true
	case *FloatExp:
		return x.Float, true
	default:
		return 0, false
	}
}
