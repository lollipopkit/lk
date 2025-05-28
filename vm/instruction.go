package vm

import (
	"github.com/lollipopkit/lk/api"
)

const MAXARG_Bx = 1<<18 - 1       // 262143
const MAXARG_sBx = MAXARG_Bx >> 1 // 131071

/*
31       22       13       5    0

	+-------+^------+-^-----+-^-----
	|b=9bits |c=9bits |a=8bits|op=6|
	+-------+^------+-^-----+-^-----
	|    bx=18bits    |a=8bits|op=6|
	+-------+^------+-^-----+-^-----
	|   sbx=18bits    |a=8bits|op=6|
	+-------+^------+-^-----+-^-----
	|    ax=26bits            |op=6|
	+-------+^------+-^-----+-^-----

31      23      15       7      0
*/
type Instruction uint32

func (self Instruction) Opcode() int {
	return int(self & 0x3F)
}

func (self Instruction) ABC() (a, b, c int) {
	a = int(self >> 6 & 0xFF)
	c = int(self >> 14 & 0x1FF)
	b = int(self >> 23 & 0x1FF)
	return
}

func (self Instruction) ABx() (a, bx int) {
	a = int(self >> 6 & 0xFF)
	bx = int(self >> 14)
	return
}

func (self Instruction) AsBx() (a, sbx int) {
	a, bx := self.ABx()
	return a, bx - MAXARG_sBx
}

func (self Instruction) Ax() int {
	return int(self >> 6)
}

func (self Instruction) OpName() string {
	return opcodes[self.Opcode()].name
}

func (self Instruction) OpMode() byte {
	return opcodes[self.Opcode()].opMode
}

func (self Instruction) BMode() byte {
	return opcodes[self.Opcode()].argBMode
}

func (self Instruction) CMode() byte {
	return opcodes[self.Opcode()].argCMode
}

type instructionFunc func(Instruction, api.LkVM)

// 预计算的跳转表
var jumpTable [64]instructionFunc

func init() {
    // 初始化跳转表
    for i := range opcodes {
        if opcodes[i].action != nil {
            jumpTable[i] = opcodes[i].action
        }
    }
}

func (self Instruction) Execute(vm api.LkVM) {
    op := self & 0x3F
    if fn := jumpTable[op]; fn != nil {
        fn(self, vm)
    } else {
        panic("No instruction: " + opcodes[op].name)
    }
}
