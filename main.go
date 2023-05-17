package main

import (
	"flag"
	"io/ioutil"
	"os"
	"strings"

	"github.com/lollipopkit/gommon/log"
	"github.com/lollipopkit/lk/compiler/parser"
	"github.com/lollipopkit/lk/repl"
	"github.com/lollipopkit/lk/state"
	. "github.com/lollipopkit/lk/json"
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
			log.Yellow("Can't run file without suffix '.lk(c)':\n" + fPath)
		}
	}
}

func writeAst(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		log.Red(err.Error())
		os.Exit(1)
	}

	block := parser.Parse(string(data), path)

	j, err := Json.MarshalIndent(block, "", "  ")
	if err != nil {
		log.Red(err.Error())
		os.Exit(1)
	}

	err = ioutil.WriteFile(path+".ast.json", j, 0644)
	if err != nil {
		log.Red(err.Error())
		os.Exit(1)
	}
}

func runVM(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		log.Red("[run] can't read file: " + err.Error())
		os.Exit(1)
	}
	ls := state.New()
	defer ls.CatchAndPrint()
	ls.OpenLibs()
	ls.Load(data, path, "bt")
	ls.Call(0, -1)
}
