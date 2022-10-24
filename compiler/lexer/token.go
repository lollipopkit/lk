package lexer

// token kind
const (
	TOKEN_EOF = iota
	TOKEN_VARARG
	TOKEN_SEP_SEMI
	TOKEN_SEP_COMMA
	TOKEN_SEP_DOT
	TOKEN_SEP_COLON
	TOKEN_SEP_LPAREN
	TOKEN_SEP_RPAREN
	TOKEN_SEP_LBRACK
	TOKEN_SEP_RBRACK
	TOKEN_SEP_LCURLY
	TOKEN_SEP_RCURLY
	TOKEN_OP_ASSIGN
	TOKEN_OP_MINUS
	TOKEN_OP_WAVE
	TOKEN_OP_ADD
	TOKEN_OP_MUL
	TOKEN_OP_DIV
	TOKEN_OP_IDIV
	TOKEN_OP_POW
	TOKEN_OP_MOD
	TOKEN_OP_BAND
	TOKEN_OP_BOR
	TOKEN_OP_SHR
	TOKEN_OP_SHL
	TOKEN_OP_LT
	TOKEN_OP_LE
	TOKEN_OP_GT
	TOKEN_OP_GE
	TOKEN_OP_EQ
	TOKEN_OP_NE
	TOKEN_OP_LEN
	TOKEN_OP_AND
	TOKEN_OP_OR
	TOKEN_OP_NOT
	TOKEN_KW_BREAK
	TOKEN_KW_ELSE
	TOKEN_KW_ELSEIF
	TOKEN_KW_FALSE
	TOKEN_KW_FOR
	TOKEN_KW_FUNCTION
	TOKEN_KW_IF
	TOKEN_KW_IN
	TOKEN_KW_LOCAL
	TOKEN_KW_NIL
	TOKEN_KW_RETURN
	TOKEN_KW_TRUE
	TOKEN_KW_WHILE
	TOKEN_IDENTIFIER
	TOKEN_NUMBER
	TOKEN_STRING
	TOKEN_OP_UNM   = TOKEN_OP_MINUS
	TOKEN_OP_SUB   = TOKEN_OP_MINUS
	TOKEN_OP_BNOT  = TOKEN_OP_WAVE
	TOKEN_OP_BXOR  = TOKEN_OP_WAVE
	TOKEN_KW_CLASS = iota - 4
	TOKEN_OP_QUESTION
)

var tokenNames = map[int]string{
	TOKEN_EOF:         "EOF",
	TOKEN_VARARG:      "...",
	TOKEN_SEP_SEMI:    ";",
	TOKEN_SEP_COMMA:   ",",
	TOKEN_SEP_DOT:     ".",
	TOKEN_SEP_COLON:   ":",
	TOKEN_SEP_LPAREN:  "(",
	TOKEN_SEP_RPAREN:  ")",
	TOKEN_SEP_LBRACK:  "[",
	TOKEN_SEP_RBRACK:  "]",
	TOKEN_SEP_LCURLY:  "{",
	TOKEN_SEP_RCURLY:  "}",
	TOKEN_OP_ASSIGN:   "=",
	TOKEN_OP_MINUS:    "-",
	TOKEN_OP_WAVE:     "~",
	TOKEN_OP_ADD:      "+",
	TOKEN_OP_MUL:      "*",
	TOKEN_OP_DIV:      "/",
	TOKEN_OP_IDIV:     "~/",
	TOKEN_OP_POW:      "^",
	TOKEN_OP_MOD:      "%",
	TOKEN_OP_BAND:     "&",
	TOKEN_OP_BOR:      "|",
	TOKEN_OP_SHR:      ">>",
	TOKEN_OP_SHL:      "<<",
	TOKEN_OP_LT:       "<",
	TOKEN_OP_LE:       "<=",
	TOKEN_OP_GT:       ">",
	TOKEN_OP_GE:       ">=",
	TOKEN_OP_EQ:       "==",
	TOKEN_OP_NE:       "!=",
	TOKEN_OP_LEN:      "#",
	TOKEN_OP_AND:      "and",
	TOKEN_OP_OR:       "or",
	TOKEN_OP_NOT:      "not",
	TOKEN_KW_BREAK:    "break",
	TOKEN_KW_ELSE:     "else",
	TOKEN_KW_ELSEIF:   "elif",
	TOKEN_KW_FALSE:    "false",
	TOKEN_KW_FOR:      "for",
	TOKEN_KW_FUNCTION: "fn",
	TOKEN_KW_IF:       "if",
	TOKEN_KW_IN:       "in",
	TOKEN_KW_LOCAL:    "shy",
	TOKEN_KW_NIL:      "nil",
	TOKEN_KW_RETURN:   "rt",
	TOKEN_KW_TRUE:     "true",
	TOKEN_KW_WHILE:    "while",
	TOKEN_IDENTIFIER:  "identifier",
	TOKEN_NUMBER:      "number literal",
	TOKEN_STRING:      "string literal",
	TOKEN_KW_CLASS:    "class",
	TOKEN_OP_QUESTION: "?",
}

func tokenName(token int) string {
	name, ok := tokenNames[token]
	if !ok {
		return "unknown"
	}
	return name
}

var keywords = map[string]int{
	"and":   TOKEN_OP_AND,
	"break": TOKEN_KW_BREAK,
	"else":  TOKEN_KW_ELSE,
	"elif":  TOKEN_KW_ELSEIF,
	"false": TOKEN_KW_FALSE,
	"for":   TOKEN_KW_FOR,
	"fn":    TOKEN_KW_FUNCTION,
	"if":    TOKEN_KW_IF,
	"in":    TOKEN_KW_IN,
	"shy":   TOKEN_KW_LOCAL,
	"nil":   TOKEN_KW_NIL,
	"not":   TOKEN_OP_NOT,
	"or":    TOKEN_OP_OR,
	"rt":    TOKEN_KW_RETURN,
	"true":  TOKEN_KW_TRUE,
	"while": TOKEN_KW_WHILE,
	"class": TOKEN_KW_CLASS,
}
