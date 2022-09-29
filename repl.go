package main

import (
	"bufio"
	"os"

	"git.lolli.tech/lollipopkit/go-lang-lk/consts"
	"git.lolli.tech/lollipopkit/go-lang-lk/state"
)

func repl() {
	ls := state.New()
	ls.OpenLibs()
	println(` 
 _     _      ____            _ 
| |   | | __ |  _ \ ___ _ __ | |
| |   | |/ / | |_) / _ \ '_ \| |
| |___|   <  |  _ <  __/ |_) | |
|_____|_|\_\ |_| \_\___| .__/|_|
                       |_|      `)
	println("	    v" + consts.VERSION)

	for {
		os.Stdout.WriteString("> ")
		ls.LoadString(readline());
	
		ls.PCall(0, -1, 0);
	}
}

func readline() string {
	inputReader := bufio.NewReader(os.Stdin)
    input, _ := inputReader.ReadString('\n')
	return input
}
