package main

import (
	"io/ioutil"
	"os"
	"strings"

	"git.lolli.tech/lollipopkit/lk/binchunk"
	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
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

	compiledData, err := bin.Dump(utils.Md5(data))
	if err != nil {
		term.Error("[compile] dump file failed: " + err.Error())
	}
	err = ioutil.WriteFile(source+"c", compiledData, 0744)
	if err != nil {
		term.Error("[compile] write file failed: " + err.Error())
	}
	return compiledData
}

func runVM(data []byte, source string) {
	ls := state.New()
	ls.OpenLibs()
	ls.Load(data, source, "bt")
	ls.Call(0, -1)
}

func runlk(source string) {
	lkc := source + "c"
	if exist(lkc) {
		runlkc(lkc)
	} else {
		data := compile(source)
		runVM(data, source)
	}
}

func runlkc(source string) {
	if !exist(source) {
		term.Error("[run] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}

	lkPath := source[:len(source)-1]
	lkData := make([]byte, 0)
	lkExist := exist(lkPath)
	if lkExist {
		lkData, err = ioutil.ReadFile(lkPath)
		if err != nil {
			term.Error("[run] can't read file: " + err.Error())
		}
	}

	_, err = binchunk.Verify(data, lkData)
	if err != nil {
		if err == binchunk.ErrMismatchedHash {
			if lkExist {
				term.Info("[run] source changed, recompiling " + lkPath)
				data = compile(lkPath)
			} else {
				term.Warn("[run] source not found: " + lkPath)
			}
		} else if strings.HasPrefix(err.Error(), binchunk.MismatchVersionPrefix) {
			if lkExist {
				term.Info("[run] mismatch version, recompiling " + lkPath)
				data = compile(lkPath)
			} else {
				term.Error("[run] mismatch version and source not found: " + lkPath)
			}
		} else {
			term.Error("[run] chunk verify failed: " + err.Error())
		}
	}

	runVM(data, source)
}

func run(file string) {
	if strings.HasSuffix(file, ".lk") {
		runlk(file)
	} else if strings.HasSuffix(file, ".lkc") {
		runlkc(file)
	}
}

func exist(path string) bool {
	_, err := os.Stat(path)
	return !os.IsNotExist(err)
}
