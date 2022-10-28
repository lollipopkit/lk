package consts

import "regexp"

const (
	ForInReStr    = `for +(\S+(, *\S+)*) +in +(\S+) *\{`
	FnReStr       = `fn *(\S*)\((\S*(, *\S+)*)\) *\{`
	WhileReStr    = `while +(\S+ ) *\{`
	IfReStr       = `if +(\S+ )+ *\{`
	ElseIfReStr   = `elif +(\S+ ) *\{`
	ElseReStr     = `else *\{`
	ClassDefReStr = `class +(\S+) *\{`
)

var (
	ForInRe    = _re(ForInReStr)
	FnRe       = _re(FnReStr)
	WhileRe    = _re(WhileReStr)
	IfRe       = _re(IfReStr)
	ElseIfRe   = _re(ElseIfReStr)
	ElseRe     = _re(ElseReStr)
	ClassDefRe = _re(ClassDefReStr)
)

func _re(s string) *regexp.Regexp {
	return regexp.MustCompile(s)
}
