package main

import (
	"flag"
	"io/ioutil"
	"os"
	"strings"

	"git.lolli.tech/lollipopkit/go-lang-lk/compiler"
	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

func main() {
	compile := flag.Bool("c", false, "compile, not run")
	flag.Parse()

	file := flag.Arg(0)
	if file == "" {
		panic("no input file")
	}

	if !*compile {
		ls := state.New()
		ls.OpenLibs()
		ls.LoadFile(file)
		ls.Call(0, -1)
	} else {
		data, err := ioutil.ReadFile(file)
		if err != nil {
			panic(err)
		}
		bin := compiler.Compile(string(data), file)
		f, err := os.Create(strings.Replace(file, ".lk", ".lkc", 1))
		if err != nil {
			panic(err)
		}
		data, err = bin.Dump()
		if err != nil {
			panic(err)
		}
		f.Write(data)
	}
}
