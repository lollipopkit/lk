package main

import (
	"encoding/json"
	"flag"
	"io/ioutil"
	"strings"

	"git.lolli.tech/lollipopkit/lk/compiler/parser"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
)

var (
	args = []string{}
)

func main() {
	ast := flag.Bool("a", false, "Write AST Tree Json")
	compile := flag.Bool("c", false, "Compile file")

	flag.Parse()
	args = flag.Args()
	if len(args) == 0 {
		repl()
		return
	}

	if *ast {
		WriteAst(args[0])
	} else if *compile {
		state.Compile(args[0])
	} else {
		if strings.Contains(args[0], ".lk") {
			runVM(args[0])
		} else {
			term.Warn("Can't run file without suffix '.lk':\n" + args[0])
		}
	}
}

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

func runVM(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}
	ls := state.New()
	defer ls.CatchAndPrint()
	ls.OpenLibs()
	ls.Load(data, path, "bt")
	ls.Call(0, -1)
}
