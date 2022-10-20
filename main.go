package main

import (
	"flag"

	"git.lolli.tech/lollipopkit/lk/consts"
)

func main() {
	force := flag.Bool("f", false, "force to re-compile")
	debug := flag.Bool("d", false, "debug mode")
	flag.Parse()

	consts.Debug = *debug
	
	args := flag.Args()
	switch len(args) {
	case 0:
		repl()
	default:
		run(args[0], *force)
	}
}
