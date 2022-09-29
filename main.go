package main

import (
	"crypto/sha256"
	"encoding/hex"
	"flag"
	"io/ioutil"
	"os"
	"path"

	"git.lolli.tech/lollipopkit/go-lang-lk/compiler"
	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

func main() {
	flag.Parse()

	file := flag.Arg(0)
	if file == "" {
		panic("no input file")
	}

	compiledFileName := getSHA256HashCode([]byte(file)) + ".lkc"
	compiledFile := path.Join(os.TempDir(), compiledFileName)
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

	ls := state.New()
	ls.OpenLibs()
	ls.Load(compiledData, file, "bt")
	ls.Call(0, -1)
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

func getSHA256HashCode(message []byte) string {
	bytes := sha256.Sum256(message)
	hashCode := hex.EncodeToString(bytes[:])
	return hashCode
}
