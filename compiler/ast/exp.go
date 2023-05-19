package ast

/*
exp ::=  nil | false | true | Numeral | LiteralString | ‘...’ | functiondef |
	 prefixexp | tableconstructor | exp binop exp | unop exp

prefixexp ::= var | functioncall | ‘(’ exp ‘)’

var ::=  Name | prefixexp ‘[’ exp ‘]’ | prefixexp ‘.’ Name

functioncall ::=  prefixexp args | prefixexp ‘:’ Name args
*/

type Exp interface{}

type NilExp struct{ Line int }    // nil
type TrueExp struct{ Line int }   // true
type FalseExp struct{ Line int }  // false
type VarargExp struct{ Line int } // ...

// Numeral
type IntegerExp struct {
	Line int
	Int  int64
}
type FloatExp struct {
	Line  int
	Float float64
}

// LiteralString
type StringExp struct {
	Line int
	Str  string
}

// unop exp
type UnopExp struct {
	Line int // line of operator
	Op   int // operator
	Unop Exp
}

// exp1 op exp2
type BinopExp struct {
	Line  int // line of operator
	Op    int // operator
	Left  Exp
	Right Exp
}

// exp1 ? exp2 : exp3
type TernaryExp struct {
	Line  int // line of operator
	Cond  Exp
	True  Exp
	False Exp
}

// tableconstructor ::= ‘{’ [fieldlist] ‘}’
// fieldlist ::= field {fieldsep field} [fieldsep]
// field ::= ‘[’ exp ‘]’ ‘=’ exp | Name ‘=’ exp | exp
// fieldsep ::= ‘,’ | ‘;’
type TableConstructorExp struct {
	Line     int // line of `{` ?
	LastLine int // line of `}`
	KeyExps  []Exp
	ValExps  []Exp
}

// functiondef ::= function funcbody
// funcbody ::= ‘(’ [parlist] ‘)’ block end
// parlist ::= namelist [‘,’ ‘...’] | ‘...’
// namelist ::= Name {‘,’ Name}
type FuncDefExp struct {
	Line     int
	LastLine int // line of `end`
	ParList  []string
	IsVararg bool
	Block    *Block
}

/*
prefixexp ::= Name |
              ‘(’ exp ‘)’ |
              prefixexp ‘[’ exp ‘]’ |
              prefixexp ‘.’ Name |
              prefixexp ‘:’ Name args |
              prefixexp args
*/

type NameExp struct {
	Line int
	Name string
}

type ParensExp struct {
	Exp Exp
}

type TableAccessExp struct {
	LastLine  int // line of `]` ?
	PrefixExp Exp
	KeyExp    Exp
}

type FuncCallExp struct {
	Line      int // line of `(` ?
	LastLine  int // line of ')'
	PrefixExp Exp
	NameExp   *StringExp
	Args      []Exp
}
