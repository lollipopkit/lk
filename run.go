package main

import (
	"io/ioutil"
	
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
)

func runVM(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}
	ls := state.New()
	ls.OpenLibs()
	ls.Load(data, path, "bt")
	ls.Call(0, -1)
}
