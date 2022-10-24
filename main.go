package main

import (
	"flag"
	"strings"

	"git.lolli.tech/lollipopkit/lk/consts"
)

var (
	debug = flag.Bool("d", false, "debug mode")
	args  = []string{}
)

func main() {
	flag.Parse()

	consts.Debug = *debug

	args = flag.Args()
	if len(args) == 0 {
		repl()
		return
	}
	switch args[0] {
	case "compile":
		compile(args[1])
	default:
		if strings.Contains(args[0], ".lk") {
			run(args[0])
		} else {
			print("Unknown command: " + args[0])
		}
	}
}
