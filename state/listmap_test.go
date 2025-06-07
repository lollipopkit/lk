package state_test

import (
	"testing"

	"github.com/lollipopkit/lk/state"
)

func TestVMListAndMap(t *testing.T) {
	ls := state.New()
	ls.OpenLibs()
	ls.LoadString("rt [1,2]", "stdin")
	ls.Call(0, 1)
	if !ls.IsTable(-1) {
		t.Fatalf("result not table")
	}
	ls.GetI(-1, 0)
	if v := ls.ToInteger(-1); v != 1 {
		t.Fatalf("first val %d", v)
	}
	ls.Pop(1)
	ls.GetI(-1, 1)
	if v := ls.ToInteger(-1); v != 2 {
		t.Fatalf("second val %d", v)
	}
	ls.Pop(1)
	ls.Pop(1)

	ls.LoadString("rt {'a':1}", "stdin")
	ls.Call(0, 1)
	ls.GetField(-1, "a")
	if v := ls.ToInteger(-1); v != 1 {
		t.Fatalf("map value %d", v)
	}
	ls.Pop(1)
}
