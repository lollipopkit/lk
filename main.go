package main

import (
	"flag"
	"os"
	"strings"

	"github.com/lollipopkit/lk/compiler/parser"
	. "github.com/lollipopkit/lk/json"
	"github.com/lollipopkit/lk/term"
	"github.com/lollipopkit/lk/repl"
	"github.com/lollipopkit/lk/state"
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
		repl.Repl()
		return
	}

	fPath := args[0]
	if *ast {
		writeAst(fPath)
	} else if *compile {
		state.Compile(fPath)
	} else {
		if strings.HasSuffix(fPath, ".lk") || strings.HasSuffix(fPath, ".lkc") {
			runVM(fPath)
		} else {
			term.Yellow("Can't run file without suffix '.lk(c)':\n" + fPath)
		}
	}
}

func writeAst(path string) {
	data, err := os.ReadFile(path)
	if err != nil {
		term.Red(err.Error())
		os.Exit(1)
	}

	block := parser.Parse(string(data), path)

	j, err := Json.MarshalIndent(block, "", "  ")
	if err != nil {
		term.Red(err.Error())
		os.Exit(1)
	}

	err = os.WriteFile(path+".ast.json", j, 0644)
	if err != nil {
		term.Red(err.Error())
		os.Exit(1)
	}
}

func runVM(path string) {
	data, err := os.ReadFile(path)
	if err != nil {
		term.Red("[run] can't read file: " + err.Error())
		os.Exit(1)
	}
	ls := state.New()
	defer ls.CatchAndPrint(false)
	ls.OpenLibs()
	ls.Load(data, path, "bt")
	ls.Call(0, -1)
}
