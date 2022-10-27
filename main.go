package main

import (
	"flag"
	"strings"

	"git.lolli.tech/lollipopkit/lk/mods"
	"git.lolli.tech/lollipopkit/lk/state"
)

var (
	args = []string{}
)

func init() {
	go mods.InitMods()
}

func main() {
	flag.Parse()
	args = flag.Args()
	if len(args) == 0 {
		repl()
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
