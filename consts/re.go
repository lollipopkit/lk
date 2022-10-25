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

	// a++
	NameExpPPReStr = `(\S+)\+\+`
	// a--
	NameExpMMReStr = `(\S+)\-\-`
	// a += 1
	NameExpAddReStr = `(\S+) *\+= *(\S+)`
	// a -= 1
	NameExpSubReStr = `(\S+) *-= *(\S+)`
	// a *= 1
	NameExpMulReStr = `(\S+) *\*= *(\S+)`
	// a /= 1
	NameExpDivReStr = `(\S+) */= *(\S+)`
	// a %= 1
	NameExpModReStr = `(\S+) *%= *(\S+)`
	// a ^= 1
	NameExpPowReStr = `(\S+) *\^= *(\S+)`
	// a &= 1
	NameExpAndReStr = `(\S+) *&= *(\S+)`
	// a |= 1
	NameExpOrReStr = `(\S+) *\|= *(\S+)`
	// a <<= 1
	NameExpLShiftReStr = `(\S+) *<<= *(\S+)`
	// a >>= 1
	NameExpRShiftReStr = `(\S+) *>>= *(\S+)`
)

var (
	ForInRe    = _re(ForInReStr)
	FnRe       = _re(FnReStr)
	WhileRe    = _re(WhileReStr)
	IfRe       = _re(IfReStr)
	ElseIfRe   = _re(ElseIfReStr)
	ElseRe     = _re(ElseReStr)
	ClassDefRe = _re(ClassDefReStr)

	NameExpPPRe     = _re(NameExpPPReStr)
	NameExpMMRe     = _re(NameExpMMReStr)
	NameExpAddRe    = _re(NameExpAddReStr)
	NameExpSubRe    = _re(NameExpSubReStr)
	NameExpMulRe    = _re(NameExpMulReStr)
	NameExpDivRe    = _re(NameExpDivReStr)
	NameExpModRe    = _re(NameExpModReStr)
	NameExpPowRe    = _re(NameExpPowReStr)
	NameExpAndRe    = _re(NameExpAndReStr)
	NameExpOrRe     = _re(NameExpOrReStr)
	NameExpLShiftRe = _re(NameExpLShiftReStr)
	NameExpRShiftRe = _re(NameExpRShiftReStr)
)

func _re(s string) *regexp.Regexp {
	return regexp.MustCompile(s)
}
