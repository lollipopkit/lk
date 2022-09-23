package api

type LuaVM interface {
	LkState
	PC() int
	AddPC(n int)
	Fetch() uint32
	GetConst(idx int)
	GetRK(rk int)
	RegisterCount() int
	LoadVararg(n int)
	LoadProto(idx int)
	CloseUpvalues(a int)
}
