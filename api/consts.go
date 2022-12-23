package api

import (
	"math/bits"
)

const LK_MINSTACK = 20
const LKI_MAXSTACK = 1000000
const LK_REGISTRYINDEX = -LKI_MAXSTACK - 1000
const LK_RIDX_MAINTHREAD int64 = 1
const LK_RIDX_GLOBALS int64 = 2
const LK_MULTRET = -1

const (
	offset        = bits.UintSize - 1
	LK_MAXINTEGER = 1<<offset - 1
	LK_MININTEGER = -1 << offset
)

/* basic types */
type LkType = int

const (
	LK_TNONE LkType = iota - 1 // -1
	LK_TNIL
	LK_TBOOLEAN
	LK_TLIGHTUSERDATA
	LK_TNUMBER
	LK_TSTRING
	LK_TTABLE
	LK_TFUNCTION
	LK_TUSERDATA
	LK_TTHREAD
)

/* arithmetic functions */
type ArithOp = int

const (
	LK_OPADD  ArithOp = iota // +
	LK_OPSUB                 // -
	LK_OPMUL                 // *
	LK_OPMOD                 // %
	LK_OPPOW                 // ^
	LK_OPDIV                 // /
	LK_OPIDIV                // //
	LK_OPBAND                // &
	LK_OPBOR                 // |
	LK_OPBXOR                // ~
	LK_OPSHL                 // <<
	LK_OPSHR                 // >>
	LK_OPUNM                 // -
	LK_OPBNOT                // ~
)

/* comparison functions */
type CompareOp = int

const (
	LK_OPEQ CompareOp = iota // ==
	LK_OPLT                  // <
	LK_OPLE                  // <=
)

/* thread status */
type LkStatus int

const (
	LK_OK LkStatus = iota
	LK_YIELD
	LK_ERRRUN
	LK_ERRSYNTAX
	LK_ERRMEM
	LK_ERRGCMM
	LK_ERRERR
	LK_ERRFILE
)
