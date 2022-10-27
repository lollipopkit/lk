package main

import (
	"flag"
	"strings"
	"sync"

	"git.lolli.tech/lollipopkit/lk/mods"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
)

var (
	args = []string{}
	wg   = new(sync.WaitGroup)
)

func init() {
	go mods.InitMods(wg)
	go utils.CheckUpgrade(wg)
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
			term.Warn("Can't run file without suffix '.lk':\n" + args[0])
		}
	}
}
