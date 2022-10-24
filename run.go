package main

import (
	"io/ioutil"
	"os"

	"git.lolli.tech/lollipopkit/lk/binchunk"
	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/state"
)

func compile(source string) []byte {
	if !exist(source) {
		panic("file not found")
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		panic(err)
	}

	bin := compiler.Compile(string(data), source)
	f, err := os.Create(source + "c")
	if err != nil {
		panic(err)
	}
	compiledData, err := bin.Dump()
	if err != nil {
		panic(err)
	}
	f.Write(compiledData)
	return compiledData
}

func run(file string) {
	if !exist(file) {
		panic("file not found")
	}

	data, err := ioutil.ReadFile(file)
	if err != nil {
		panic(err)
	}

	compiledFilePath := file + "c"
	compiledData, _ := ioutil.ReadFile(compiledFilePath)

	valid, _ := binchunk.Verify(data)
	if valid {
		compiledData = data
	} else {
		compile(file)
	}

	ls := state.New()
	ls.OpenLibs()
	ls.Load(compiledData, file, "bt")
	ls.Call(0, -1)
}

func exist(path string) bool {
	_, err := os.Stat(path)
	return !os.IsNotExist(err)
}
