package main

import (
	"io/ioutil"
	"os"
	"strings"

	"git.lolli.tech/lollipopkit/lk/binchunk"
	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
)

func compile(source string) []byte {
	if !exist(source) {
		term.Error("[compile] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[compile] can't read file: " + err.Error())
	}

	bin := compiler.Compile(string(data), source)
	f, err := os.Create(source + "c")
	if err != nil {
		term.Error("[compile] can't create file: " + err.Error())
	}

	compiledData, err := bin.Dump()
	if err != nil {
		term.Error("[compile] dump file failed: " + err.Error())
	}
	f.Write(compiledData)
	return compiledData
}

func run(file string) {
	if !exist(file) {
		term.Error("[run] file not found: " + file)
	}

	data, err := ioutil.ReadFile(file)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}

	var compiledData []byte

	_, err = binchunk.Verify(data)
	if err == nil {
		compiledData = data
	} else if strings.HasSuffix(file, ".lk") {
		compiledData = compile(file)
	} else {
		term.Error("[run] can't compile: " + err.Error())
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
