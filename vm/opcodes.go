package vm

import "github.com/lollipopkit/lk/api"

/* OpMode */
/* basic instruction format */
const (
	IABC  = iota // [  B:9  ][  C:9  ][ A:8  ][OP:6]
	IABx         // [      Bx:18     ][ A:8  ][OP:6]
	IAsBx        // [     sBx:18     ][ A:8  ][OP:6]
	IAx          // [           Ax:26        ][OP:6]
)

/* OpArgMask */
const (
	OpArgN = iota // argument is not used
	OpArgU        // argument is used
	OpArgR        // argument is a register or a jump offset
	OpArgK        // argument is a constant or register/constant
)

/* OpCode */
const (
	OP_MOVE = iota
	OP_LOADK
	OP_LOADKX
	OP_LOADBOOL
	OP_LOADNIL
	OP_GETUPVAL
	OP_GETTABUP
	OP_GETTABLE
	OP_SETTABUP
	OP_SETUPVAL
	OP_SETTABLE
	OP_NEWMAP
	OP_NEWLIST
	OP_SELF
	OP_ADD
	OP_SUB
	OP_MUL
	OP_MOD
	OP_POW
	OP_DIV
	OP_IDIV
	OP_BAND
	OP_BOR
	OP_BXOR
	OP_SHL
	OP_SHR
	OP_UNM
	OP_BNOT
	OP_NOT
	OP_LEN
	OP_JMP
	OP_EQ
	OP_LT
	OP_LE
	OP_TEST
	OP_TESTSET
	OP_CALL
	OP_TAILCALL
	OP_RETURN
	OP_FORLOOP
	OP_FORPREP
	OP_TFORCALL
	OP_TFORLOOP
	OP_SETLIST
	OP_CLOSURE
	OP_VARARG
	OP_EXTRAARG
)

type opcode struct {
	testFlag byte // operator is a test (next instruction must be a jump)
	setAFlag byte // instruction set register A
	argBMode byte // B arg mode
	argCMode byte // C arg mode
	opMode   byte // op mode
	name     string
	action   func(i Instruction, vm api.LkVM)
}

var opcodes = []opcode{
	/*
	 T  A    B       C     mode         name       action
	*/
	{0, 1, OpArgR, OpArgN, IABC /* */, "MOVE    ", move},     // R(A) := R(B)
	{0, 1, OpArgK, OpArgN, IABx /* */, "LOADK   ", loadK},    // R(A) := Kst(Bx)
	{0, 1, OpArgN, OpArgN, IABx /* */, "LOADKX  ", loadKx},   // R(A) := Kst(extra arg)
	{0, 1, OpArgU, OpArgU, IABC /* */, "LOADBOOL", loadBool}, // R(A) := (bool)B; if (C) pc++
	{0, 1, OpArgU, OpArgN, IABC /* */, "LOADNIL ", loadNil},  // R(A), R(A+1), ..., R(A+B) := nil
	{0, 1, OpArgU, OpArgN, IABC /* */, "GETUPVAL", getUpval}, // R(A) := UpValue[B]
	{0, 1, OpArgU, OpArgK, IABC /* */, "GETTABUP", getTabUp}, // R(A) := UpValue[B][RK(C)]
	{0, 1, OpArgR, OpArgK, IABC /* */, "GETTABLE", getTable}, // R(A) := R(B)[RK(C)]
	{0, 0, OpArgK, OpArgK, IABC /* */, "SETTABUP", setTabUp}, // UpValue[A][RK(B)] := RK(C)
	{0, 0, OpArgU, OpArgN, IABC /* */, "SETUPVAL", setUpval}, // UpValue[B] := R(A)
	{0, 0, OpArgK, OpArgK, IABC /* */, "SETTABLE", setTable}, // R(A)[RK(B)] := RK(C)
	{0, 1, OpArgU, OpArgU, IABC /* */, "NEWMAP ", newMap},    // R(A) := {} (size = B,C)
	{0, 1, OpArgU, OpArgU, IABC /* */, "NEWLIST", newList},   // R(A) := [] (size = B)
	{0, 1, OpArgR, OpArgK, IABC /* */, "SELF    ", self},     // R(A+1) := R(B); R(A) := R(B)[RK(C)]
	{0, 1, OpArgK, OpArgK, IABC /* */, "ADD     ", add},      // R(A) := RK(B) + RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "SUB     ", sub},      // R(A) := RK(B) - RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "MUL     ", mul},      // R(A) := RK(B) * RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "MOD     ", mod},      // R(A) := RK(B) % RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "POW     ", pow},      // R(A) := RK(B) ^ RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "DIV     ", div},      // R(A) := RK(B) / RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "IDIV    ", idiv},     // R(A) := RK(B) // RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "BAND    ", band},     // R(A) := RK(B) & RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "BOR     ", bor},      // R(A) := RK(B) | RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "BXOR    ", bxor},     // R(A) := RK(B) ~ RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "SHL     ", shl},      // R(A) := RK(B) << RK(C)
	{0, 1, OpArgK, OpArgK, IABC /* */, "SHR     ", shr},      // R(A) := RK(B) >> RK(C)
	{0, 1, OpArgR, OpArgN, IABC /* */, "UNM     ", unm},      // R(A) := -R(B)
	{0, 1, OpArgR, OpArgN, IABC /* */, "BNOT    ", bnot},     // R(A) := ~R(B)
	{0, 1, OpArgR, OpArgN, IABC /* */, "NOT     ", not},      // R(A) := not R(B)
	{0, 1, OpArgR, OpArgN, IABC /* */, "LEN     ", length},   // R(A) := length of R(B)
	{0, 0, OpArgR, OpArgN, IAsBx /**/, "JMP     ", jmp},      // pc+=sBx; if (A) close all upvalues >= R(A - 1)
	{1, 0, OpArgK, OpArgK, IABC /* */, "EQ      ", eq},       // if ((RK(B) == RK(C)) ~= A) then pc++
	{1, 0, OpArgK, OpArgK, IABC /* */, "LT      ", lt},       // if ((RK(B) <  RK(C)) ~= A) then pc++
	{1, 0, OpArgK, OpArgK, IABC /* */, "LE      ", le},       // if ((RK(B) <= RK(C)) ~= A) then pc++
	{1, 0, OpArgN, OpArgU, IABC /* */, "TEST    ", test},     // if not (R(A) <=> C) then pc++
	{1, 1, OpArgR, OpArgU, IABC /* */, "TESTSET ", testSet},  // if (R(B) <=> C) then R(A) := R(B) else pc++
	{0, 1, OpArgU, OpArgU, IABC /* */, "CALL    ", call},     // R(A), ... ,R(A+C-2) := R(A)(R(A+1), ... ,R(A+B-1))
	{0, 1, OpArgU, OpArgU, IABC /* */, "TAILCALL", tailCall}, // return R(A)(R(A+1), ... ,R(A+B-1))
	{0, 0, OpArgU, OpArgN, IABC /* */, "RETURN  ", _return},  // return R(A), ... ,R(A+B-2)
	{0, 1, OpArgR, OpArgN, IAsBx /**/, "FORLOOP ", forLoop},  // R(A)+=R(A+2); if R(A) <?= R(A+1) then { pc+=sBx; R(A+3)=R(A) }
	{0, 1, OpArgR, OpArgN, IAsBx /**/, "FORPREP ", forPrep},  // R(A)-=R(A+2); pc+=sBx
	{0, 0, OpArgN, OpArgU, IABC /* */, "TFORCALL", tForCall}, // R(A+3), ... ,R(A+2+C) := R(A)(R(A+1), R(A+2));
	{0, 1, OpArgR, OpArgN, IAsBx /**/, "TFORLOOP", tForLoop}, // if R(A+1) ~= nil then { R(A)=R(A+1); pc += sBx }
	{0, 0, OpArgU, OpArgU, IABC /* */, "SETLIST ", setList},  // R(A)[(C-1)*FPF+i] := R(A+i), 1 <= i <= B
	{0, 1, OpArgU, OpArgN, IABx /* */, "CLOSURE ", closure},  // R(A) := closure(KPROTO[Bx])
	{0, 1, OpArgU, OpArgN, IABC /* */, "VARARG  ", vararg},   // R(A), R(A+1), ..., R(A+B-2) = vararg
	{0, 0, OpArgU, OpArgU, IAx /*  */, "EXTRAARG", nil},      // extra (larger) argument for previous opcode
}
