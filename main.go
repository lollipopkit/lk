package main

import (
	"io/ioutil"
	"os"

	"git.lolli.tech/lollipopkit/go-lang-lk/compiler"
	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

func main() {
	if len(os.Args) > 2 {
		action := os.Args[1]
		file := os.Args[2]
		switch action {
		case "run":
			ls := state.New()
			ls.OpenLibs()
			ls.LoadFile(file)
			ls.Call(0, -1)
		case "compile":
			data, err := ioutil.ReadFile(file)
			if err != nil {
				panic(err)
			}
			bin := compiler.Compile(string(data), file)
			f, err := os.Create(file + ".lkc")
			if err != nil {
				panic(err)
			}
			f.Write(bin.Dump())
		}
	}
}
