package main

import (
	"flag"
	"strings"
	"sync"

	"git.lolli.tech/lollipopkit/lk/mods"
	"git.lolli.tech/lollipopkit/lk/state"
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
	flag.Parse()
	args = flag.Args()
	if len(args) == 0 {
		repl(wg)
		return
	}
	switch args[0] {
	case "compile":
		state.Compile(args[1])
	default:
		if strings.Contains(args[0], ".lk") {
			runVM(args[0])
		} else {
			print("Unknown command: " + args[0])
		}
	}
}
