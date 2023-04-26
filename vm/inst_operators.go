package vm

import . "github.com/lollipopkit/lk/api"

/* arith */

func add(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPADD) }  // +
func sub(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPSUB) }  // -
func mul(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPMUL) }  // *
func mod(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPMOD) }  // %
func pow(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPPOW) }  // ^
func div(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPDIV) }  // /
func idiv(i Instruction, vm LkVM) { _binaryArith(i, vm, LK_OPIDIV) } // //
func band(i Instruction, vm LkVM) { _binaryArith(i, vm, LK_OPBAND) } // &
func bor(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPBOR) }  // |
func bxor(i Instruction, vm LkVM) { _binaryArith(i, vm, LK_OPBXOR) } // ~
func shl(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPSHL) }  // <<
func shr(i Instruction, vm LkVM)  { _binaryArith(i, vm, LK_OPSHR) }  // >>
func unm(i Instruction, vm LkVM)  { _unaryArith(i, vm, LK_OPUNM) }   // -
func bnot(i Instruction, vm LkVM) { _unaryArith(i, vm, LK_OPBNOT) }  // ~

// R(A) := RK(B) op RK(C)
func _binaryArith(i Instruction, vm LkVM, op ArithOp) {
	a, b, c := i.ABC()
	a += 1

	vm.GetRK(b)
	vm.GetRK(c)
	vm.Arith(op)
	vm.Replace(a)
}

// R(A) := op R(B)
func _unaryArith(i Instruction, vm LkVM, op ArithOp) {
	a, b, _ := i.ABC()
	a += 1
	b += 1

	vm.PushValue(b)
	vm.Arith(op)
	vm.Replace(a)
}

/* compare */

func eq(i Instruction, vm LkVM) { _compare(i, vm, LK_OPEQ) } // ==
func lt(i Instruction, vm LkVM) { _compare(i, vm, LK_OPLT) } // <
func le(i Instruction, vm LkVM) { _compare(i, vm, LK_OPLE) } // <=

// if ((RK(B) op RK(C)) ~= A) then pc++
func _compare(i Instruction, vm LkVM, op CompareOp) {
	a, b, c := i.ABC()

	vm.GetRK(b)
	vm.GetRK(c)
	if vm.Compare(-2, -1, op) != (a != 0) {
		vm.AddPC(1)
	}
	vm.Pop(2)
}

/* logical */

// R(A) := not R(B)
func not(i Instruction, vm LkVM) {
	a, b, _ := i.ABC()
	a += 1
	b += 1

	vm.PushBoolean(!vm.ToBoolean(b))
	vm.Replace(a)
}

// if not (R(A) <=> C) then pc++
func test(i Instruction, vm LkVM) {
	a, _, c := i.ABC()
	a += 1

	if vm.ToBoolean(a) != (c != 0) {
		vm.AddPC(1)
	}
}

// if (R(B) <=> C) then R(A) := R(B) else pc++
func testSet(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1
	b += 1

	if vm.ToBoolean(b) == (c != 0) {
		vm.Copy(b, a)
	} else {
		vm.AddPC(1)
	}
}

/* len & concat */

// R(A) := length of R(B)
func length(i Instruction, vm LkVM) {
	a, b, _ := i.ABC()
	a += 1
	b += 1

	vm.Len(b)
	vm.Replace(a)
}
