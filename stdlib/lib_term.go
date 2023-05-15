package stdlib

import (
	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/gommon/res"
	"github.com/lollipopkit/gommon/term"
)

var termLib = map[string]GoFunction{
	"input": termInput,
}

func OpenTermLib(ls LkState) int {
	ls.NewLib(termLib)
	ls.PushString(res.CYAN)
	ls.SetField(-2, "cyan")
	ls.PushString(res.GREEN)
	ls.SetField(-2, "green")
	ls.PushString(res.RED)
	ls.SetField(-2, "red")
	ls.PushString(res.YELLOW)
	ls.SetField(-2, "yellow")
	ls.PushString(res.NOCOLOR)
	ls.SetField(-2, "nocolor")
	return 1
}

func termInput(ls LkState) int {
	ls.PushString(term.ReadLine(term.ReadLineConfig{
		Prompt: ls.OptString(1, ""),
	}))
	return 1
}
