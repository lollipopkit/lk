package main

import (
	"os"

	"atomicgo.dev/cursor"
	"atomicgo.dev/keyboard"
	"atomicgo.dev/keyboard/keys"
	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/state"
)

var (
	linesHistory = []string{}
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

		line := readline()
		if line == "" {
			continue
		}

		// 更新历史记录
		updateHistory(line)

		// 加载line，调用
		ls.LoadString(line, "stdin")
		ls.PCall(0, -1, 0)
	}
}

func updateHistory(str string) {
	idx := -1
	for i := range linesHistory {
		if linesHistory[i] == str {
			idx = i
			break
		}
	}
	if idx != -1 {
		linesHistory = append(linesHistory[:idx], linesHistory[idx+1:]...)
	}
	linesHistory = append(linesHistory, str)
}

func readline() string {
	str := ""
	linesIdx := len(linesHistory)

	keyboard.Listen(func(key keys.Key) (stop bool, err error) {
		switch key.Code {
		case keys.CtrlC, keys.Escape:
			os.Exit(0)
		case keys.RuneKey:
			runes := key.Runes
			s := string(runes)
			str += s
			print(s)
		case keys.Enter:
			println()
			return true, nil
		case keys.Backspace:
			if len(str) > 0 {
				str = str[:len(str)-1]
				resetLine(str)
			}
		case keys.Left:
			cursor.Left(1)
		case keys.Right:
			cursor.Right(1)
		case keys.Up:
			if linesIdx > 0 {
				linesIdx--
				str = linesHistory[linesIdx]
				resetLine(str)
			}
		case keys.Down:
			if linesIdx < len(linesHistory)-1 {
				linesIdx++
				str = linesHistory[linesIdx]
				resetLine(str)
			}
		}

		return false, nil
	})
	return str
}

func resetLine(str string) {
	cursor.ClearLine()
	cursor.StartOfLine()
	print("> " + str)
}
