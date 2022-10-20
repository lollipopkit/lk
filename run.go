package main

import (
	"crypto/sha256"
	"encoding/hex"
	"io/ioutil"
	"os"
	"path"

	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/state"
)

func compile(source, output string) {
	if !exist(source) {
		panic("file not found")
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		panic(err)
	}

	bin := compiler.Compile(string(data), source)
	f, err := os.Create(output)
	if err != nil {
		panic(err)
	}
	compiledData, err := bin.Dump()
	if err != nil {
		panic(err)
	}
	f.Write(compiledData)
}

func run(file string) {
	if !exist(file) {
		panic("file not found")
	}

	data, err := ioutil.ReadFile(file)
	if err != nil {
		panic(err)
	}

	compiledFileName := getSHA256HashCode(data) + ".lkc"
	compiledFilePath := path.Join(os.TempDir(), compiledFileName)
	compiledData, _ := ioutil.ReadFile(compiledFilePath)

	if !exist(compiledFilePath) || *force {
		bin := compiler.Compile(string(data), file)
		f, err := os.Create(compiledFilePath)
		if err != nil {
			panic(err)
		}
		compiledData, err = bin.Dump()
		if err != nil {
			panic(err)
		}
		f.Write(compiledData)
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

func getSHA256HashCode(message []byte) string {
	bytes := sha256.Sum256(message)
	hashCode := hex.EncodeToString(bytes[:])
	return hashCode
}
