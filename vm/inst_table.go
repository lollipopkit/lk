package vm

import . "github.com/lollipopkit/lk/api"

/* number of list items to accumulate before a SETLIST instruction */
const LFIELDS_PER_FLUSH = 50

// R(A) := {} (size = B,C)
func newMap(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1

	vm.CreateTable(Fb2int(b), Fb2int(c))
	vm.Replace(a)
}

func newList(i Instruction, vm LkVM) {
	a, b, _ := i.ABC()
	a += 1

	vm.CreateTable(Fb2int(b), 0)
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

// R(A)[(C-1)*FPF+i] := R(A+i), 1 <= i <= B
func setList(i Instruction, vm LkVM) {
	a, b, c := i.ABC()
	a += 1

	if c > 0 {
		c = c - 1
	} else {
		c = Instruction(vm.Fetch()).Ax()
	}

	bIsZero := b == 0
	if bIsZero {
		b = int(vm.ToInteger(-1)) - a - 1
		vm.Pop(1)
	}

	vm.CheckStack(1)
	idx := int64(c*LFIELDS_PER_FLUSH) - 1
	for j := 1; j <= b; j++ {
		idx++
		vm.PushValue(a + j)
		vm.SetI(a, idx)
	}

	if bIsZero {
		for j := vm.RegisterCount() + 1; j <= vm.GetTop(); j++ {
			idx++
			vm.PushValue(j)
			vm.SetI(a, idx)
		}

		// clear stack
		vm.SetTop(vm.RegisterCount())
	}
}
