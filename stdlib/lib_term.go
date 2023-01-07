package stdlib

import (
	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/term"
)

var termLib = map[string]GoFunction{
	"input": termInput,
}

func OpenTermLib(ls LkState) int {
	ls.NewLib(termLib)
	ls.PushString(term.CYAN)
	ls.SetField(-2, "cyan")
	ls.PushString(term.GREEN)
	ls.SetField(-2, "green")
	ls.PushString(term.RED)
	ls.SetField(-2, "red")
	ls.PushString(term.YELLOW)
	ls.SetField(-2, "yellow")
	ls.PushString(term.NOCOLOR)
	ls.SetField(-2, "nocolor")
	return 1
}

func termInput(ls LkState) int {
	ls.PushString(term.ReadLine(term.EmptyStringList, ls.OptString(1, "")))
	return 1
}
