package main

import (
	"flag"
	"io/ioutil"
	"os"
	"strings"

	jsoniter "github.com/json-iterator/go"
	"github.com/lollipopkit/gommon/log"
	"github.com/lollipopkit/lk/compiler/parser"
	"github.com/lollipopkit/lk/state"
)

var (
	args = []string{}
	json = jsoniter.ConfigCompatibleWithStandardLibrary
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
			log.Yellow("Can't run file without suffix '.lk':\n" + args[0])
		}
	}
}

func WriteAst(path string) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		log.Red(err.Error())
		os.Exit(1)
	}

	block := parser.Parse(string(data), path)

	j, err := json.MarshalIndent(block, "", "  ")
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
