package lexer

import (
	"bytes"
	"fmt"
	"regexp"
	"strconv"
	"strings"
)

// var reSpaces = regexp.MustCompile(`^\s+`)
var reNewLine = regexp.MustCompile("\r\n|\n\r|\n|\r")
var reIdentifier = regexp.MustCompile(`^[_\d\w]+`)
var reNumber = regexp.MustCompile(`^0[xX][0-9a-fA-F]*(\.[0-9a-fA-F]*)?([pP][+\-]?[0-9]+)?|^[0-9]*(\.[0-9]*)?([eE][+\-]?[0-9]+)?`)
var reShortStr = regexp.MustCompile(`(?s)(^'(\\\\|\\'|\\\n|\\z\s*|[^'\n])*')|(^"(\\\\|\\"|\\\n|\\z\s*|[^"\n])*")`)

var reDecEscapeSeq = regexp.MustCompile(`^\\[0-9]{1,3}`)
var reHexEscapeSeq = regexp.MustCompile(`^\\x[0-9a-fA-F]{2}`)
var reUnicodeEscapeSeq = regexp.MustCompile(`^\\u\{[0-9a-fA-F]+\}`)

type Lexer struct {
	chunk         string // source code
	chunkName     string // source name
	line          int    // current line number
	nextToken     string
	nextTokenKind int
	nextTokenLine int
}

func NewLexer(chunk, chunkName string) *Lexer {
	return &Lexer{chunk, chunkName, 1, "", 0, 0}
}

func (self *Lexer) Line() int {
	return self.line
}

func (self *Lexer) LookAhead() int {
	if self.nextTokenLine > 0 {
		return self.nextTokenKind
	}
	currentLine := self.line
	line, kind, token := self.NextToken()
	self.line = currentLine
	self.nextTokenLine = line
	self.nextTokenKind = kind
	self.nextToken = token
	return kind
}

func (self *Lexer) NextIdentifier() (line int, token string) {
	return self.NextTokenOfKind(TOKEN_IDENTIFIER)
}

func (self *Lexer) NextTokenOfKind(kind int) (line int, token string) {
	line, _kind, token := self.NextToken()
	if kind != _kind {
		self.error("syntax error, expect '%s' but '%s'", tokenName(kind), tokenName(_kind))
	}
	return line, token
}

func (self *Lexer) NextToken() (line, kind int, token string) {
	if self.nextTokenLine > 0 {
		line = self.nextTokenLine
		kind = self.nextTokenKind
		token = self.nextToken
		self.line = self.nextTokenLine
		self.nextTokenLine = 0
		return
	}

	self.skipWhiteSpaces()
	if len(self.chunk) == 0 {
		return self.line, TOKEN_EOF, "EOF"
	}

	switch self.chunk[0] {
	case ';':
		self.next(1)
		return self.line, TOKEN_SEP_SEMI, ";"
	case ',':
		self.next(1)
		return self.line, TOKEN_SEP_COMMA, ","
	case '(':
		self.next(1)
		return self.line, TOKEN_SEP_LPAREN, "("
	case ')':
		self.next(1)
		return self.line, TOKEN_SEP_RPAREN, ")"
	case ']':
		self.next(1)
		return self.line, TOKEN_SEP_RBRACK, "]"
	case '{':
		self.next(1)
		return self.line, TOKEN_SEP_LCURLY, "{"
	case '}':
		self.next(1)
		return self.line, TOKEN_SEP_RCURLY, "}"
	case '+':
		self.next(1)
		return self.line, TOKEN_OP_ADD, "+"
	case '-':
		self.next(1)
		return self.line, TOKEN_OP_MINUS, "-"
	case '*':
		self.next(1)
		return self.line, TOKEN_OP_MUL, "*"
	case '^':
		self.next(1)
		return self.line, TOKEN_OP_POW, "^"
	case '%':
		self.next(1)
		return self.line, TOKEN_OP_MOD, "%"
	case '&':
		self.next(1)
		return self.line, TOKEN_OP_BAND, "&"
	case '|':
		self.next(1)
		return self.line, TOKEN_OP_BOR, "|"
	case '#':
		self.next(1)
		return self.line, TOKEN_OP_LEN, "#"
	case ':':
		if self.test(":=") {
			self.next(2)
			return self.line, TOKEN_OP_ASSIGNSHY, ":="
		}
		self.next(1)
		return self.line, TOKEN_SEP_COLON, ":"
	case '/':
		self.next(1)
		return self.line, TOKEN_OP_DIV, "/"
	case '~':
		if self.test("~/") {
			self.next(2)
			return self.line, TOKEN_OP_IDIV, "~/"
		}
		self.next(1)
		return self.line, TOKEN_OP_WAVE, "~"
	case '!':
		if self.test("!=") {
			self.next(2)
			return self.line, TOKEN_OP_NE, "!="
		}
	case '=':
		if self.test("==") {
			self.next(2)
			return self.line, TOKEN_OP_EQ, "=="
		} else if self.test("=>") {
			self.next(2)
			return self.line, TOKEN_OP_ARROW, "=>"
		} else {
			self.next(1)
			return self.line, TOKEN_OP_ASSIGN, "="
		}
	case '<':
		if self.test("<<") {
			self.next(2)
			return self.line, TOKEN_OP_SHL, "<<"
		} else if self.test("<=") {
			self.next(2)
			return self.line, TOKEN_OP_LE, "<="
		} else {
			self.next(1)
			return self.line, TOKEN_OP_LT, "<"
		}
	case '>':
		if self.test(">>") {
			self.next(2)
			return self.line, TOKEN_OP_SHR, ">>"
		} else if self.test(">=") {
			self.next(2)
			return self.line, TOKEN_OP_GE, ">="
		} else {
			self.next(1)
			return self.line, TOKEN_OP_GT, ">"
		}
	case '.':
		if self.test("...") {
			self.next(3)
			return self.line, TOKEN_VARARG, "..."
		} else if len(self.chunk) == 1 || !isDigit(self.chunk[1]) {
			self.next(1)
			return self.line, TOKEN_SEP_DOT, "."
		}
	case '[':
		self.next(1)
		return self.line, TOKEN_SEP_LBRACK, "["
	case '?':
		if self.test("??") {
			self.next(2)
			return self.line, TOKEN_OP_NILCOALESCING, "??"
		} else {
			self.next(1)
			return self.line, TOKEN_OP_QUESTION, "?"
		}
	case '\'', '"':
		return self.line, TOKEN_STRING, self.scanShortString()
	case '`':
		return self.line, TOKEN_STRING, self.scanRawString()
	}

	c := self.chunk[0]
	if c == '.' || isDigit(c) {
		token := self.scanNumber()
		return self.line, TOKEN_NUMBER, token
	}
	if c == '_' || isLetter(c) {
		token := self.scanIdentifier()
		if kind, found := keywords[token]; found {
			return self.line, kind, token // keyword
		} else {
			return self.line, TOKEN_IDENTIFIER, token
		}
	}

	self.error("unexpected symbol near %q", c)
	return
}

func (self *Lexer) next(n int) {
	self.chunk = self.chunk[n:]
}

func (self *Lexer) test(s string) bool {
	return strings.HasPrefix(self.chunk, s)
}

func (self *Lexer) error(f string, a ...interface{}) {
	err := fmt.Sprintf(f, a...)
	err = fmt.Sprintf("%s:%d: %s", self.chunkName, self.line, err)
	panic(err)
}

func (self *Lexer) skipWhiteSpaces() {
	for len(self.chunk) > 0 {
		if self.test("//") {
			self.skipComment()
		} else if self.test("/*") {
			self.skipLongComment()
		} else if self.test("\r\n") || self.test("\n\r") {
			self.next(2)
			self.line += 1
		} else if isNewLine(self.chunk[0]) {
			self.next(1)
			self.line += 1
		} else if isWhiteSpace(self.chunk[0]) {
			self.next(1)
		} else {
			break
		}
	}
}

func (self *Lexer) skipComment() {
	self.next(2) // skip `//`

	// short comment
	for len(self.chunk) > 0 && !isNewLine(self.chunk[0]) {
		self.next(1)
	}
}

func (self *Lexer) skipLongComment() {
	self.next(2)
	idx := strings.Index(self.chunk, "*/")
	if idx < 0 {
		self.error("unfinished long comment at line: " + strconv.Itoa(self.line))
	}
	self.next(idx + 2)
}

func (self *Lexer) scanIdentifier() string {
	return self.scan(reIdentifier)
}

func (self *Lexer) scanNumber() string {
	return self.scan(reNumber)
}

func (self *Lexer) scan(re *regexp.Regexp) string {
	if token := re.FindString(self.chunk); token != "" {
		self.next(len(token))
		return token
	}
	panic("unreachable!")
}

func (self *Lexer) scanShortString() string {
	if str := reShortStr.FindString(self.chunk); str != "" {
		self.next(len(str))
		str = str[1 : len(str)-1]
		if strings.Index(str, `\`) >= 0 {
			self.line += len(reNewLine.FindAllString(str, -1))
			str = self.escape(str)
		}
		return str
	}
	self.error("unfinished string")
	return ""
}

func (self *Lexer) scanRawString() string {
	self.next(1)
	openIdx := strings.Index(self.chunk, "`")
	if openIdx < 0 {
		self.error("unfinished string")
	}

	str := self.chunk[:openIdx]
	if len(str) > 0 && str[0] == '\n' {
		str = str[1:]
	}
	self.next(openIdx + 1)
	return str
}

func (self *Lexer) escape(str string) string {
	var buf bytes.Buffer

	for len(str) > 0 {
		if str[0] != '\\' {
			buf.WriteByte(str[0])
			str = str[1:]
			continue
		}

		if len(str) == 1 {
			self.error("unfinished string")
		}

		switch str[1] {
		case 'a':
			buf.WriteByte('\a')
			str = str[2:]
			continue
		case 'b':
			buf.WriteByte('\b')
			str = str[2:]
			continue
		case 'f':
			buf.WriteByte('\f')
			str = str[2:]
			continue
		case 'n', '\n':
			buf.WriteByte('\n')
			str = str[2:]
			continue
		case 'r':
			buf.WriteByte('\r')
			str = str[2:]
			continue
		case 't':
			buf.WriteByte('\t')
			str = str[2:]
			continue
		case 'v':
			buf.WriteByte('\v')
			str = str[2:]
			continue
		case '"':
			buf.WriteByte('"')
			str = str[2:]
			continue
		case '\'':
			buf.WriteByte('\'')
			str = str[2:]
			continue
		case '\\':
			buf.WriteByte('\\')
			str = str[2:]
			continue
		case '0', '1', '2', '3', '4', '5', '6', '7', '8', '9': // \ddd
			if found := reDecEscapeSeq.FindString(str); found != "" {
				d, _ := strconv.ParseInt(found[1:], 10, 32)
				if d <= 0xFF {
					buf.WriteByte(byte(d))
					str = str[len(found):]
					continue
				}
				self.error("decimal escape too large near '%s'", found)
			}
		case 'x': // \xXX
			if found := reHexEscapeSeq.FindString(str); found != "" {
				d, _ := strconv.ParseInt(found[2:], 16, 32)
				buf.WriteByte(byte(d))
				str = str[len(found):]
				continue
			}
		case 'u': // \u{XXX}
			if found := reUnicodeEscapeSeq.FindString(str); found != "" {
				d, err := strconv.ParseInt(found[3:len(found)-1], 16, 32)
				if err == nil && d <= 0x10FFFF {
					buf.WriteRune(rune(d))
					str = str[len(found):]
					continue
				}
				self.error("UTF-8 value too large near '%s'", found)
			}
		case 'z':
			str = str[2:]
			for len(str) > 0 && isWhiteSpace(str[0]) { // todo
				str = str[1:]
			}
			continue
		}
		self.error("invalid escape sequence near '\\%c'", str[1])
	}

	return buf.String()
}

func isWhiteSpace(c byte) bool {
	switch c {
	case '\t', '\n', '\v', '\f', '\r', ' ':
		return true
	}
	return false
}

func isNewLine(c byte) bool {
	return c == '\r' || c == '\n'
}

func isDigit(c byte) bool {
	return c >= '0' && c <= '9'
}

func isLetter(c byte) bool {
	return c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z'
}
