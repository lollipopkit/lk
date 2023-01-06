package main

import (
	"os"
	"strings"
	"testing"
)

const (
	file = "test/basic"
)

var (
	skipTestList = []string{
		"http_listen.lk",
	}
)

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
		if files[idx].IsDir() || contains(skipTestList, name) || !strings.HasSuffix(name, ".lk") {
			continue
		}
		println("=== " + name + " ===")
		runVM("test/" + name)
	}
}

func BenchmarkRun(b *testing.B) {
	f := file + ".lk"
	for i := 0; i < b.N; i++ {
		runVM(f)
	}
}

