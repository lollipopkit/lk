package parser

import (
	. "github.com/lollipopkit/lk/compiler/ast"
	. "github.com/lollipopkit/lk/compiler/lexer"
)

var _statEmpty = &EmptyStat{}

/*
stat ::=  ‘;’

	| break
	| while exp '{' block '}'
	| if exp '{' block {elif exp '{' block '}' } [else block] '}'
	| for Name ‘=’ exp ‘,’ exp [‘,’ exp] '{' block '}'
	| for namelist in explist '{' block '}'
	| function funcname funcbody
	| shy function Name funcbody
	| shy namelist [‘=’ explist]
	| varlist ‘=’ explist
	| functioncall
*/
func ParseStat(lexer *Lexer) Stat {
	switch lexer.LookAhead() {
	case TOKEN_SEP_SEMI:
		return parseEmptyStat(lexer)
	case TOKEN_KW_BREAK:
		return parseBreakStat(lexer)
	case TOKEN_KW_WHILE:
		return parseWhileStat(lexer)
	case TOKEN_KW_IF:
		return parseIfStat(lexer)
	case TOKEN_KW_FOR:
		return parseForStat(lexer)
	case TOKEN_KW_FUNCTION:
		return parseFuncDefStat(lexer)
	case TOKEN_KW_SHY:
		return parseLocalAssignOrFuncDefStat(lexer)
	case TOKEN_KW_CLASS:
		return parseClassDefStat(lexer)
	default:
		return parseAssignOrFuncCallStat(lexer)
	}
}

// ;
func parseEmptyStat(lexer *Lexer) *EmptyStat {
	lexer.NextTokenOfKind(TOKEN_SEP_SEMI)
	return _statEmpty
}

// break
func parseBreakStat(lexer *Lexer) *BreakStat {
	lexer.NextTokenOfKind(TOKEN_KW_BREAK)
	return &BreakStat{lexer.Line()}
}

// while exp do block end
func parseWhileStat(lexer *Lexer) *WhileStat {
	lexer.NextTokenOfKind(TOKEN_KW_WHILE)   // while
	exp := ParseExp(lexer)                  // exp
	lexer.NextTokenOfKind(TOKEN_SEP_LCURLY) // {
	block := ParseBlock(lexer)              // block
	lexer.NextTokenOfKind(TOKEN_SEP_RCURLY) // }
	return &WhileStat{exp, block}
}

// if exp then block {elseif exp then block} [else block] end
func parseIfStat(lexer *Lexer) *IfStat {
	exps := make([]Exp, 0, 4)
	blocks := make([]*Block, 0, 4)

	lexer.NextTokenOfKind(TOKEN_KW_IF)         // if
	exps = append(exps, ParseExp(lexer))       // exp
	lexer.NextTokenOfKind(TOKEN_SEP_LCURLY)    // {
	blocks = append(blocks, ParseBlock(lexer)) // block
	lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)    // }
	for lexer.LookAhead() == TOKEN_KW_ELSEIF {
		lexer.NextToken()                          // elseif
		exps = append(exps, ParseExp(lexer))       // exp
		lexer.NextTokenOfKind(TOKEN_SEP_LCURLY)    // {
		blocks = append(blocks, ParseBlock(lexer)) // block
		lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)    // }
	}

	// else block => elseif true then block
	if lexer.LookAhead() == TOKEN_KW_ELSE {
		lexer.NextToken()                           // else
		lexer.NextTokenOfKind(TOKEN_SEP_LCURLY)     // {
		exps = append(exps, &TrueExp{lexer.Line()}) //
		blocks = append(blocks, ParseBlock(lexer))  // block
		lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)     // }
	}

	return &IfStat{exps, blocks}
}

// for Name ‘=’ exp ‘,’ exp [‘,’ exp] do block end
// for namelist in explist do block end
func parseForStat(lexer *Lexer) Stat {
	lineOfFor, _ := lexer.NextTokenOfKind(TOKEN_KW_FOR)
	_, name := lexer.NextIdentifier()
	if lexer.LookAhead() == TOKEN_OP_ASSIGN {
		return _finishForNumStat(lexer, lineOfFor, name)
	} else {
		return _finishForInStat(lexer, name)
	}
}

// for Name ‘=’ exp ‘,’ exp [‘,’ exp] do block end
func _finishForNumStat(lexer *Lexer, lineOfFor int, varName string) *ForNumStat {
	lexer.NextTokenOfKind(TOKEN_OP_ASSIGN) // for name =
	initExp := ParseExp(lexer)             // exp
	lexer.NextTokenOfKind(TOKEN_SEP_COMMA) // ,
	limitExp := ParseExp(lexer)            // exp

	var stepExp Exp
	if lexer.LookAhead() == TOKEN_SEP_COMMA {
		lexer.NextToken()         // ,
		stepExp = ParseExp(lexer) // exp
	} else {
		stepExp = &IntegerExp{lexer.Line(), 1}
	}

	lineOfDo, _ := lexer.NextTokenOfKind(TOKEN_SEP_LCURLY) // {
	block := ParseBlock(lexer)                             // block
	lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)                // }

	return &ForNumStat{lineOfFor, lineOfDo,
		varName, initExp, limitExp, stepExp, block}
}

// for namelist in explist do block end
// namelist ::= Name {‘,’ Name}
// explist ::= exp {‘,’ exp}
func _finishForInStat(lexer *Lexer, name0 string) *ForInStat {
	nameList := _finishNameList(lexer, name0)              // for namelist
	lexer.NextTokenOfKind(TOKEN_KW_IN)                     // in
	expList := parseExpList(lexer)                         // explist
	lineOfDo, _ := lexer.NextTokenOfKind(TOKEN_SEP_LCURLY) // {
	block := ParseBlock(lexer)                             // block
	lexer.NextTokenOfKind(TOKEN_SEP_RCURLY)                // }
	if len(expList) == 1 {
		e := expList[0]
		expList[0] = &FuncCallExp{
			Line:      lineOfDo,
			LastLine:  lineOfDo,
			PrefixExp: &NameExp{lineOfDo, "iter"},
			NameExp:   nil,
			Args:      []Exp{e},
		}
	}
	return &ForInStat{lineOfDo, nameList, expList, block}
}

// namelist ::= Name {‘,’ Name}
func _finishNameList(lexer *Lexer, name0 string) []string {
	names := []string{name0}
	for lexer.LookAhead() == TOKEN_SEP_COMMA {
		lexer.NextToken()                 // ,
		_, name := lexer.NextIdentifier() // Name
		names = append(names, name)
	}
	return names
}

// local function Name funcbody
// local namelist [‘=’ explist]
func parseLocalAssignOrFuncDefStat(lexer *Lexer) Stat {
	lexer.NextTokenOfKind(TOKEN_KW_SHY)
	if lexer.LookAhead() == TOKEN_KW_FUNCTION {
		return _finishLocalFuncDefStat(lexer)
	} else {
		return _finishLocalVarDeclStat(lexer)
	}
}

/*
http://www.lua.org/manual/5.3/manual.html#3.4.11

function f() end          =>  f = function() end
function t.a.b.c.f() end  =>  t.a.b.c.f = function() end
function t.a.b.c:f() end  =>  t.a.b.c.f = function(self) end
local function f() end    =>  local f; f = function() end

The statement `local function f () body end`
translates to `local f; f = function () body end`
not to `local f = function () body end`
(This only makes a difference when the body of the function
 contains references to f.)
*/
// local function Name funcbody
func _finishLocalFuncDefStat(lexer *Lexer) *LocalFuncDefStat {
	lexer.NextTokenOfKind(TOKEN_KW_FUNCTION) // local function
	_, name := lexer.NextIdentifier()        // name
	fdExp := parseFuncDefExp(lexer)          // funcbody
	return &LocalFuncDefStat{name, fdExp}
}

// local namelist [‘=’ explist]
func _finishLocalVarDeclStat(lexer *Lexer) *LocalVarDeclStat {
	_, name0 := lexer.NextIdentifier()        // local Name
	nameList := _finishNameList(lexer, name0) // { , Name }
	var expList []Exp = nil
	if lexer.LookAhead() == TOKEN_OP_ASSIGN {
		lexer.NextToken()             // ==
		expList = parseExpList(lexer) // explist
	}
	lastLine := lexer.Line()
	return &LocalVarDeclStat{lastLine, nameList, expList}
}

// varlist ‘=’ explist
// functioncall
func parseAssignOrFuncCallStat(lexer *Lexer) Stat {
	prefixExp := parsePrefixExp(lexer)
	if fc, ok := prefixExp.(*FuncCallExp); ok {
		return fc
	} else {
		return parseAssignStat(lexer, prefixExp)
	}
}

// varlist ‘=’ explist
func parseAssignStat(lexer *Lexer, var0 Exp) Stat {
	varList := _finishVarList(lexer, var0) // varlist
	if lexer.LookAhead() == TOKEN_OP_ASSIGNSHY {
		lexer.NextToken()              // :=
		expList := parseExpList(lexer) // explist
		strExps := make([]string, len(varList))
		for i := range varList {
			name, ok := varList[i].(*NameExp)
			if !ok {
				panic("invalid assignment")
			}
			strExps[i] = name.Name
		}
		return &LocalVarDeclStat{lexer.Line(), strExps, expList}
	}
	switch lexer.LookAhead() {
	case TOKEN_OP_MINUS_EQ, TOKEN_OP_ADD_EQ,
		TOKEN_OP_MUL_EQ, TOKEN_OP_DIV_EQ,
		TOKEN_OP_MOD_EQ, TOKEN_OP_POW_EQ,
		TOKEN_OP_NILCOALESCING_EQ:
		line, op, _ := lexer.NextToken()
		expList := parseExpList(lexer)
		for i := range expList {
			expList[i] = &BinopExp{line, SourceOp(op), varList[i], expList[i]}
		}
		return &AssignStat{line, varList, expList}
	case TOKEN_OP_INC, TOKEN_OP_DEC:
		line, op, _ := lexer.NextToken()
		expList := []Exp{&BinopExp{line, SourceOp(op), varList[0], &IntegerExp{line, 1}}}
		return &AssignStat{line, varList, expList}
	}
	lexer.NextTokenOfKind(TOKEN_OP_ASSIGN) // =
	expList := parseExpList(lexer)         // explist
	lastLine := lexer.Line()
	return &AssignStat{lastLine, varList, expList}
}

// varlist ::= var {‘,’ var}
func _finishVarList(lexer *Lexer, var0 Exp) []Exp {
	vars := []Exp{_checkVar(lexer, var0)}      // var
	for lexer.LookAhead() == TOKEN_SEP_COMMA { // {
		lexer.NextToken()                          // ,
		exp := parsePrefixExp(lexer)               // var
		vars = append(vars, _checkVar(lexer, exp)) //
	} // }
	return vars
}

// var ::=  Name | prefixexp ‘[’ exp ‘]’ | prefixexp ‘.’ Name
func _checkVar(lexer *Lexer, exp Exp) Exp {
	switch exp.(type) {
	case *NameExp, *TableAccessExp:
		return exp
	}
	lexer.NextTokenOfKind(-1) // trigger error
	panic("unreachable!")
}

// function funcname funcbody
// funcname ::= Name {‘.’ Name} [‘:’ Name]
// funcbody ::= ‘(’ [parlist] ‘)’ block end
// parlist ::= namelist [‘,’ ‘...’] | ‘...’
// namelist ::= Name {‘,’ Name}
func parseFuncDefStat(lexer *Lexer) *AssignStat {
	lexer.NextTokenOfKind(TOKEN_KW_FUNCTION) // function
	fnExp, hasColon := _parseFuncName(lexer) // funcname
	fdExp := parseFuncDefExp(lexer)          // funcbody
	if hasColon {                            // insert self
		fdExp.ParList = append(fdExp.ParList, "")
		copy(fdExp.ParList[1:], fdExp.ParList)
		fdExp.ParList[0] = "self"
	}

	return &AssignStat{
		LastLine: fdExp.Line,
		VarList:  []Exp{fnExp},
		ExpList:  []Exp{fdExp},
	}
}

// funcname ::= Name {‘.’ Name} [‘:’ Name]
func _parseFuncName(lexer *Lexer) (exp Exp, hasColon bool) {
	line, name := lexer.NextIdentifier()
	exp = &NameExp{line, name}

	switch lexer.LookAhead() {
	case TOKEN_SEP_COLON:
		hasColon = true
		fallthrough
	case TOKEN_SEP_DOT:
		lexer.NextToken()
		line, name := lexer.NextIdentifier()
		idx := &StringExp{line, name}
		exp = &TableAccessExp{line, exp, idx}
	}
	return
}

func parseClassDefStat(lexer *Lexer) *AssignStat {
	lexer.NextTokenOfKind(TOKEN_KW_CLASS) // class
	line, name := lexer.NextIdentifier()  // Name
	tb := parseMapConstructorExp(lexer) // map
	return &AssignStat{line, []Exp{&NameExp{line, name}}, []Exp{tb}}
}
