package main

import (
	"os"
	"testing"

	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

const (
	file = "test/basic"
)

var (
	skipTestList = []string{
		"http_listen.lk",
	}
)

func run(f string) {
	ls := state.New()
	ls.OpenLibs()
	ls.LoadFile(f)
	ls.Call(0, -1)
}

func contains[T string](list []T, item T) bool {
	for idx := range list {
		if list[idx] == item {
			return true
		}
	}
	return false
}

func TestMain(m *testing.M) {
	files, err := os.ReadDir("test")
	if err != nil {
		panic(err)
	}
	for idx := range files {
		name := files[idx].Name()
		if files[idx].IsDir() || contains(skipTestList, name){
			continue
		}
		println("=== " + name + " ===")
		run("test/" + name)
		println()
	}
}

func BenchmarkRun(b *testing.B) {
	f := file + ".lk"
	for i := 0; i < b.N; i++ {
		run(f)
	}
}

func BenchmarkRunCompiled(b *testing.B) {
	f := file + ".lkc"
	for i := 0; i < b.N; i++ {
		run(f)
	}
}
