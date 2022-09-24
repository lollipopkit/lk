package main

import (
	"testing"

	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

const file = "test/basic"

func BenchmarkRun(b *testing.B) {
	f := file + ".lk"
	for i := 0; i < b.N; i++ {
		ls := state.New()
		ls.OpenLibs()
		ls.LoadFile(f)
		ls.Call(0, -1)
	}
}

func BenchmarkRunCompiled(b *testing.B) {
	f := file + ".lkc"
	for i := 0; i < b.N; i++ {
		ls := state.New()
		ls.OpenLibs()
		ls.LoadFile(f)
		ls.Call(0, -1)
	}
}