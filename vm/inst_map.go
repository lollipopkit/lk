package vm

import . "github.com/lollipopkit/lk/api"

// R(A) := {} (size = B,C)
func newMap(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1

	vm.CreateMap(Fb2int(b), Fb2int(c))
	vm.Replace(a)
}

// R(A) := R(B)[RK(C)]
func getTable(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1
	b += 1

	vm.GetRK(c)
	vm.GetTable(b)
	vm.Replace(a)
}

// R(A)[RK(B)] := RK(C)
func setTable(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1

	vm.GetRK(b)
	vm.GetRK(c)
	vm.SetTable(a)
}
