package api

type FuncReg map[string]GoFunction

// auxiliary library
type AuxLib interface {
	/* Error-report functions */
	Error2(fmt string, a ...interface{}) int
	ArgError(arg int, extraMsg string) int
	/* Argument check functions */
	CheckStack2(sz int, msg string)
	ArgCheck(cond bool, arg int, extraMsg string)
	CheckAny(arg int)
	CheckType(arg int, t LkType)
	CheckInteger(arg int) int64
	CheckNumber(arg int) float64
	CheckString(arg int) string
	CheckBool(arg int) bool
	OptInteger(arg int, d int64) int64
	OptNumber(arg int, d float64) float64
	OptString(arg int, d string) string
	OptBool(arg int, d bool) bool
	/* Load functions */
	DoFile(filename string) bool
	DoString(str, source string) bool
	LoadFile(filename string) int
	LoadFileX(filename, mode string) int
	LoadString(s, source string) int
	/* Other functions */
	TypeName2(idx int) string
	ToString2(idx int) string
	Len2(idx int) int64
	GetSubTable(idx int, fname string) bool
	GetMetafield(obj int, e string) LkType
	CallMeta(obj int, e string) bool
	OpenLibs()
	RequireF(modname string, openf GoFunction, glb bool)
	NewLib(l FuncReg)
	NewLibTable(l FuncReg)
	SetFuncs(l FuncReg, nup int)
}
