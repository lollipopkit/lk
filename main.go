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

	compiledFile := strings.Replace(file, ".lk", ".lkc", 1)
	compiledData, _ := ioutil.ReadFile(compiledFile)
	
	if !exist(compiledFile) || sourceChanged(file, compiledFile) {
		data, err := ioutil.ReadFile(file)
		if err != nil {
			panic(err)
		}
		bin := compiler.Compile(string(data), file)
		f, err := os.Create(compiledFile)
		if err != nil {
			panic(err)
		}
		compiledData, err = bin.Dump()
		if err != nil {
			panic(err)
		}
		f.Write(compiledData)
	}

	if !*compile {
		ls := state.New()
		ls.OpenLibs()
		ls.Load(compiledData, file, "bt")
		ls.Call(0, -1)
	}
}

func exist(path string) bool {
	_, err := os.Stat(path)
	return !os.IsNotExist(err)
}

func sourceChanged(source, compiled string) bool {
	s, err := os.Stat(source)
	if err != nil {
		panic(err)
	}
	c, err := os.Stat(compiled)
	if err != nil {
		panic(err)
	}
	return s.ModTime().After(c.ModTime())
}
