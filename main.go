package main

import (
	"flag"
	"strings"

	"git.lolli.tech/lollipopkit/lk/consts"
)

var (
	force *bool
	debug *bool
	args []string
)

func main() {
	force = flag.Bool("f", false, "force to re-compile")
	debug = flag.Bool("d", false, "debug mode")
	flag.Parse()

	consts.Debug = *debug

	args = flag.Args()
	if len(args) == 0 {
		repl()
		return
	}
	switch args[0] {
	case "compile":
		compile(args[1], args[2])
	default:
		if strings.Contains(args[0], ".lk") {
			run(args[0])
		} else {
			print("Unknown command: " + args[0])
		}
	}
}
