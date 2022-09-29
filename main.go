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
	force := flag.Bool("f", false, "force to re-compile")
	flag.Parse()
	args := flag.Args()
	if len(args) == 0 {
		println("Usage: \n	lang-lk repl\n	lang-lk [options] run <file>")
		return
	}

	switch args[0] {
	case "repl":
		repl()
		return
	case "run":
		break
	}

	file := args[1]
	if file == "" {
		panic("no input file")
	}

	data, err := ioutil.ReadFile(file)
	if err != nil {
		panic(err)
	}

	compiledFileName := getSHA256HashCode(data) + ".lkc"
	compiledFile := path.Join(os.TempDir(), compiledFileName)
	compiledData, _ := ioutil.ReadFile(compiledFile)

	if !exist(compiledFile) || *force {
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

func getSHA256HashCode(message []byte) string {
	bytes := sha256.Sum256(message)
	hashCode := hex.EncodeToString(bytes[:])
	return hashCode
}
