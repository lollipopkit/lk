package main

import (
	"os"
	"slices"
	"strings"
	"testing"
)

const (
	file = "test/basic"
)

var (
	skipTestList = []string{
		"http_header.lk",
		"http_listen.lk",
		"gf.lk",
		"module.lk",
	}
)

func TestMain(m *testing.M) {
	files, err := os.ReadDir("test")
	if err != nil {
		panic(err)
	}
	for idx := range files {
		name := files[idx].Name()
		if files[idx].IsDir() || slices.Contains(skipTestList, name) || !strings.HasSuffix(name, ".lk") {
			continue
		}
		println("\n=== " + name + " ===")
		runVM("test/" + name)
	}
}

func BenchmarkRun(b *testing.B) {
	f := file + ".lk"
	for i := 0; i < b.N; i++ {
		runVM(f)
	}
}
