package vm

import . "github.com/lollipopkit/lk/api"

// R(A) := R(B)
func move(i Instruction, vm LkVM) {
	a, b, _ := i.ABC()
	a += 1
	b += 1

	vm.Copy(b, a)
}

// pc+=sBx; if (A) close all upvalues >= R(A - 1)
func jmp(i Instruction, vm LkVM) {
	a, sBx := i.AsBx()

	vm.AddPC(sBx)
	if a != 0 {
		vm.CloseUpvalues(a)
	}
}
