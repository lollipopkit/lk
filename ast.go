package main

import (
	"encoding/json"
	"io/ioutil"

	"git.lolli.tech/lollipopkit/lk/compiler/parser"
	"git.lolli.tech/lollipopkit/lk/term"
)

func WriteAst(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		term.Error(err.Error())
	}

	block := parser.Parse(string(data), path)

	j, err := json.MarshalIndent(block, "", "  ")
	if err != nil {
		term.Error(err.Error())
	}

	err = ioutil.WriteFile(path+".ast.json", j, 0644)
	if err != nil {
		term.Error(err.Error())
	}
}
