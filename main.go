package main

import (
	"encoding/json"
	"flag"
	"io/ioutil"
	"strings"
	"sync"

	"git.lolli.tech/lollipopkit/lk/compiler/parser"
	"git.lolli.tech/lollipopkit/lk/mods"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
)

var (
	args = []string{}
	wg   = new(sync.WaitGroup)
)

func init() {
	go mods.InitMods(wg)
}

func main() {
	ast := flag.Bool("a", false, "Write AST Tree Json")
	compile := flag.Bool("c", false, "Compile file")

	flag.Parse()
	args = flag.Args()
	if len(args) == 0 {
		repl(wg)
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
			term.Yellow("Can't run file without suffix '.lk':\n" + args[0])
		}
	}
}

func WriteAst(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		term.Red("[ast] read file err: " + err.Error())
	}

	block := parser.Parse(string(data), path)

	j, err := json.MarshalIndent(block, "", "  ")
	if err != nil {
		term.Red("[ast] json encode err: " + err.Error())
	}

	err = ioutil.WriteFile(path+".ast.json", j, 0644)
	if err != nil {
		term.Red("[ast] write file err: " + err.Error())
	}
}

func runVM(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		term.Red("[run] can't read file: " + err.Error())
	}
	ls := state.New()
	defer ls.CatchAndPrint()
	ls.OpenLibs()
	ls.Load(data, path, "bt")
	ls.Call(0, -1)
}
