package codegen

import . "github.com/lollipopkit/lk/compiler/ast"

func isVarargOrFuncCall(exp Exp) bool {
	switch exp.(type) {
	case *VarargExp, *FuncCallExp:
		return true
	}
	return false
}

func removeTailNils(exps []Exp) []Exp {
	for n := len(exps) - 1; n >= 0; n-- {
		if _, ok := exps[n].(*NilExp); !ok {
			return exps[0 : n+1]
		}
	}
	return nil
}

func lineOf(exp Exp) int {
	switch x := exp.(type) {
	case *NilExp:
		return x.Line
	case *TrueExp:
		return x.Line
	case *FalseExp:
		return x.Line
	case *IntegerExp:
		return x.Line
	case *FloatExp:
		return x.Line
	case *StringExp:
		return x.Line
	case *VarargExp:
		return x.Line
	case *NameExp:
		return x.Line
	case *FuncDefExp:
		return x.Line
	case *FuncCallExp:
		return x.Line
	case *MapConstructorExp:
		return x.Line
	case *ListConstructorExp:
		return x.Line
	case *UnopExp:
		return x.Line
	case *TableAccessExp:
		return lineOf(x.PrefixExp)
	case *BinopExp:
		return lineOf(x.Left)
	case *TernaryExp:
		return lineOf(x.Line)
	default:
		panic("unreachable!")
	}
}

func lastLineOf(exp Exp) int {
	switch x := exp.(type) {
	case *NilExp:
		return x.Line
	case *TrueExp:
		return x.Line
	case *FalseExp:
		return x.Line
	case *IntegerExp:
		return x.Line
	case *FloatExp:
		return x.Line
	case *StringExp:
		return x.Line
	case *VarargExp:
		return x.Line
	case *NameExp:
		return x.Line
	case *FuncDefExp:
		return x.LastLine
	case *FuncCallExp:
		return x.LastLine
	case *MapConstructorExp:
		return x.LastLine
	case *ListConstructorExp:
		return x.LastLine
	case *TableAccessExp:
		return x.LastLine
	case *BinopExp:
		return lastLineOf(x.Right)
	case *UnopExp:
		return lastLineOf(x.Unop)
	case *TernaryExp:
		return lastLineOf(x.False)
	default:
		panic("unreachable!")
	}
}
