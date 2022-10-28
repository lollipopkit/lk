package parser

import (
	. "git.lolli.tech/lollipopkit/lk/compiler/ast"
	. "git.lolli.tech/lollipopkit/lk/compiler/lexer"
	"git.lolli.tech/lollipopkit/lk/utils"
)

// explist ::= exp {‘,’ exp}
func parseExpList(lexer *Lexer) []Exp {
	exps := make([]Exp, 0, 4)
	exps = append(exps, parseExp(lexer))
	for lexer.LookAhead() == TOKEN_SEP_COMMA {
		lexer.NextToken()
		exps = append(exps, parseExp(lexer))
	}
	return exps
}

/*
exp ::=  nil | false | true | Numeral | LiteralString | ‘...’ | functiondef |
	 prefixexp | tableconstructor | exp binop exp | unop exp
*/
/*
exp   ::= exp16
exp16 ::= exp15 {'++' | '--'}
exp15 ::= exp14 {'+=' | '-=' | '*=' | '/=' | '%=' | '^=' exp14}
exp14 ::= exp13 {'??' exp13}
exp13 ::= exp12 {'?' exp12 : exp12}
exp12 ::= exp11 {or exp11}
exp11 ::= exp10 {and exp10}
exp10 ::= exp9 {(‘<’ | ‘>’ | ‘<=’ | ‘>=’ | ‘!=’ | ‘==’) exp9}
exp9  ::= exp8 {‘|’ exp8}
exp8  ::= exp7 {‘~’ exp7}
exp7  ::= exp6 {‘&’ exp6}
exp6  ::= exp5 {(‘<<’ | ‘>>’) exp5}
exp5  ::= exp4 {‘..’ exp4}
exp4  ::= exp3 {(‘+’ | ‘-’) exp3}
exp3  ::= exp2 {(‘*’ | ‘/’ | ‘//’ | ‘%’) exp2}
exp2  ::= {(‘not’ | ‘#’ | ‘-’ | ‘~’)} exp1
exp1  ::= exp0 {‘^’ exp2}
exp0  ::= nil | false | true | Numeral | LiteralString
		| ‘...’ | functiondef | prefixexp | tableconstructor
*/
func parseExp(lexer *Lexer) Exp {
	return parseExp14(lexer)
}

func parseExp14(lexer *Lexer) Exp {
	exp := parseExp13(lexer)
	for lexer.LookAhead() == TOKEN_OP_NILCOALESCING {
		line, _, _ := lexer.NextToken()
		exp2 := parseExp13(lexer)
		exp = &TernaryExp{line, &BinopExp{line, TOKEN_OP_EQ, exp, &NilExp{}}, exp2, exp}
	}
	return exp
}

func parseExp13(lexer *Lexer) Exp {
	exp1 := parseExp12(lexer)
	if lexer.LookAhead() == TOKEN_OP_QUESTION {
		line, _, _ := lexer.NextToken()
		exp2 := parseExp12(lexer)
		lexer.NextTokenOfKind(TOKEN_SEP_COLON)
		exp3 := parseExp12(lexer)
		return &TernaryExp{line, exp1, exp2, exp3}
	}
	return exp1
}

// x or y
func parseExp12(lexer *Lexer) Exp {
	exp := parseExp11(lexer)
	for lexer.LookAhead() == TOKEN_OP_OR {
		line, op, _ := lexer.NextToken()
		lor := &BinopExp{line, op, exp, parseExp11(lexer)}
		exp = optimizeLogicalOr(lor)
	}
	return exp
}

// x and y
func parseExp11(lexer *Lexer) Exp {
	exp := parseExp10(lexer)
	for lexer.LookAhead() == TOKEN_OP_AND {
		line, op, _ := lexer.NextToken()
		land := &BinopExp{line, op, exp, parseExp10(lexer)}
		exp = optimizeLogicalAnd(land)
	}
	return exp
}

// compare
func parseExp10(lexer *Lexer) Exp {
	exp := parseExp9(lexer)
	for {
		switch lexer.LookAhead() {
		case TOKEN_OP_LT, TOKEN_OP_GT, TOKEN_OP_NE,
			TOKEN_OP_LE, TOKEN_OP_GE, TOKEN_OP_EQ:
			line, op, _ := lexer.NextToken()
			exp = &BinopExp{line, op, exp, parseExp9(lexer)}
		default:
			return exp
		}
	}
}

// x | y
func parseExp9(lexer *Lexer) Exp {
	exp := parseExp8(lexer)
	for lexer.LookAhead() == TOKEN_OP_BOR {
		line, op, _ := lexer.NextToken()
		bor := &BinopExp{line, op, exp, parseExp8(lexer)}
		exp = optimizeBitwiseBinaryOp(bor)
	}
	return exp
}

// x ~ y
func parseExp8(lexer *Lexer) Exp {
	exp := parseExp7(lexer)
	for lexer.LookAhead() == TOKEN_OP_BXOR {
		line, op, _ := lexer.NextToken()
		bxor := &BinopExp{line, op, exp, parseExp7(lexer)}
		exp = optimizeBitwiseBinaryOp(bxor)
	}
	return exp
}

// x & y
func parseExp7(lexer *Lexer) Exp {
	exp := parseExp6(lexer)
	for lexer.LookAhead() == TOKEN_OP_BAND {
		line, op, _ := lexer.NextToken()
		band := &BinopExp{line, op, exp, parseExp6(lexer)}
		exp = optimizeBitwiseBinaryOp(band)
	}
	return exp
}

// shift
func parseExp6(lexer *Lexer) Exp {
	exp := parseExp4(lexer)
	for {
		switch lexer.LookAhead() {
		case TOKEN_OP_SHL, TOKEN_OP_SHR:
			line, op, _ := lexer.NextToken()
			shx := &BinopExp{line, op, exp, parseExp4(lexer)}
			exp = optimizeBitwiseBinaryOp(shx)
		default:
			return exp
		}
	}
}

// x +/- y
func parseExp4(lexer *Lexer) Exp {
	exp := parseExp3(lexer)
	for {
		switch lexer.LookAhead() {
		case TOKEN_OP_ADD, TOKEN_OP_SUB:
			line, op, _ := lexer.NextToken()
			arith := &BinopExp{line, op, exp, parseExp3(lexer)}
			exp = optimizeArithBinaryOp(arith)
		default:
			return exp
		}
	}
}

// *, %, /, //
func parseExp3(lexer *Lexer) Exp {
	exp := parseExp2(lexer)
	for {
		switch lexer.LookAhead() {
		case TOKEN_OP_MUL, TOKEN_OP_MOD, TOKEN_OP_DIV, TOKEN_OP_IDIV:
			line, op, _ := lexer.NextToken()
			arith := &BinopExp{line, op, exp, parseExp2(lexer)}
			exp = optimizeArithBinaryOp(arith)
		default:
			return exp
		}
	}
}

// unary
func parseExp2(lexer *Lexer) Exp {
	switch lexer.LookAhead() {
	case TOKEN_OP_UNM, TOKEN_OP_BNOT, TOKEN_OP_LEN, TOKEN_OP_NOT:
		line, op, _ := lexer.NextToken()
		exp := &UnopExp{line, op, parseExp2(lexer)}
		return optimizeUnaryOp(exp)
	}
	return parseExp1(lexer)
}

// x ^ y
func parseExp1(lexer *Lexer) Exp { // pow is right associative
	exp := parseExp0(lexer)
	if lexer.LookAhead() == TOKEN_OP_POW {
		line, op, _ := lexer.NextToken()
		exp = &BinopExp{line, op, exp, parseExp2(lexer)}
	}
	return optimizePow(exp)
}

func parseExp0(lexer *Lexer) Exp {
	switch lexer.LookAhead() {
	case TOKEN_VARARG: // ...
		line, _, _ := lexer.NextToken()
		return &VarargExp{line}
	case TOKEN_KW_NIL: // nil
		line, _, _ := lexer.NextToken()
		return &NilExp{line}
	case TOKEN_KW_TRUE: // true
		line, _, _ := lexer.NextToken()
		return &TrueExp{line}
	case TOKEN_KW_FALSE: // false
		line, _, _ := lexer.NextToken()
		return &FalseExp{line}
	case TOKEN_STRING: // LiteralString
		line, _, token := lexer.NextToken()
		return &StringExp{line, token}
	case TOKEN_NUMBER: // Numeral
		return parseNumberExp(lexer)
	case TOKEN_SEP_LCURLY: // tableconstructor
		return parseTableConstructorExp(lexer)
	case TOKEN_KW_FUNCTION: // functiondef
		lexer.NextToken()
		return parseFuncDefExp(lexer)
	default: // prefixexp
		return parsePrefixExp(lexer)
	}
}

func parseNumberExp(lexer *Lexer) Exp {
	line, _, token := lexer.NextToken()
	if i, ok := utils.ParseInteger(token); ok {
		return &IntegerExp{line, i}
	} else if f, ok := utils.ParseFloat(token); ok {
		return &FloatExp{line, f}
	} else { // todo
		panic("not a utils: " + token)
	}
}

// functiondef ::= fn funcbody
// funcbody ::= ‘(’ [parlist] ‘)’ `{` block `}`
func parseFuncDefExp(lexer *Lexer) *FuncDefExp {
	line := lexer.Line()                      // fn
	lexer.NextTokenOfKind(TOKEN_SEP_LPAREN)   // (
	parList, isVararg := _parseParList(lexer) // [parlist]
	lexer.NextTokenOfKind(TOKEN_SEP_RPAREN)   // )
	if lexer.LookAhead() == TOKEN_OP_ARROW {
		lexer.NextToken() // ->
		return &FuncDefExp{line, line, parList, isVararg, &Block{
			Stats:    []Stat{},
			RetExps:  parseExpList(lexer),
			LastLine: line,
		}}
	}
	lexer.NextTokenOfKind(TOKEN_SEP_LCURLY)                // {
	block := parseBlock(lexer)                             // block
	lastLine, _ := lexer.NextTokenOfKind(TOKEN_SEP_RCURLY) // }
	return &FuncDefExp{line, lastLine, parList, isVararg, block}
}

// [parlist]
// parlist ::= namelist [‘,’ ‘...’] | ‘...’
func _parseParList(lexer *Lexer) (names []string, isVararg bool) {
	switch lexer.LookAhead() {
	case TOKEN_SEP_RPAREN:
		return nil, false
	case TOKEN_VARARG:
		lexer.NextToken()
		return nil, true
	}

	_, name := lexer.NextIdentifier()
	names = append(names, name)
	for lexer.LookAhead() == TOKEN_SEP_COMMA {
		lexer.NextToken()
		if lexer.LookAhead() == TOKEN_IDENTIFIER {
			_, name := lexer.NextIdentifier()
			names = append(names, name)
		} else {
			lexer.NextTokenOfKind(TOKEN_VARARG)
			isVararg = true
			break
		}
	}
	return
}

// tableconstructor ::= ‘{’ [fieldlist] ‘}’
func parseTableConstructorExp(lexer *Lexer) *TableConstructorExp {
	line := lexer.Line()
	lexer.NextTokenOfKind(TOKEN_SEP_LCURLY)    // {
	keyExps, valExps := _parseFieldList(lexer) // [fieldlist]
	lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)    // }
	lastLine := lexer.Line()
	return &TableConstructorExp{line, lastLine, keyExps, valExps}
}

// fieldlist ::= field {fieldsep field} [fieldsep]
func _parseFieldList(lexer *Lexer) (ks, vs []Exp) {
	if lexer.LookAhead() != TOKEN_SEP_RCURLY {
		k, v := _parseField(lexer)
		ks = append(ks, k)
		vs = append(vs, v)

		for lexer.LookAhead() == TOKEN_SEP_COMMA {
			lexer.NextToken()
			if lexer.LookAhead() != TOKEN_SEP_RCURLY {
				k, v := _parseField(lexer)
				ks = append(ks, k)
				vs = append(vs, v)
			} else {
				break
			}
		}
	}
	return
}

// field ::= ‘[’ exp ‘]’ ‘:’ exp | Name ‘:’ exp | exp
func _parseField(lexer *Lexer) (k, v Exp) {
	if lexer.LookAhead() == TOKEN_SEP_LBRACK {
		lexer.NextToken()                       // [
		k = parseExp(lexer)                     // exp
		lexer.NextTokenOfKind(TOKEN_SEP_RBRACK) // ]
		lexer.NextTokenOfKind(TOKEN_SEP_COLON)  // :
		v = parseExp(lexer)                     // exp
		return
	}

	exp := parseExp(lexer)
	if nameExp, ok := exp.(*StringExp); ok {
		if lexer.LookAhead() == TOKEN_SEP_COLON {
			// Name ‘:’ exp => ‘[’ LiteralString ‘]’ = exp
			lexer.NextToken()
			k = &StringExp{nameExp.Line, nameExp.Str}
			v = parseExp(lexer)
			return
		}
	}

	return nil, exp
}
