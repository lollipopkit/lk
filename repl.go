package main

import (
	"fmt"
	"regexp"
	"strings"
	"sync"

	"git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/state"
	"git.lolli.tech/lollipopkit/lk/term"
)

var (
	linesHistory  = []string{}
	blockStartReg = regexp.MustCompile(strings.Join([]string{
		consts.ForInReStr,
		consts.FnReStr,
		consts.WhileReStr,
		consts.IfReStr,
		consts.ElseIfReStr,
		consts.ElseReStr,
		consts.ClassDefReStr,
	}, "|"))
	blockEndReg = regexp.MustCompile("} *$")
	printReg    = regexp.MustCompile(`print\(.*\)`)
)

func repl(wg *sync.WaitGroup) {
	ls := state.New()
	ls.OpenLibs()

	term.Cyan("REPL - Lang LK v" + consts.VERSION + "\n")

	blockStr := ""
	blockStartCount := 0
	blockEndCount := 0
	wg.Wait()

	for {
		line := term.ReadLine(linesHistory)
		if line == "" {
			continue
		}
		if blockStartReg.MatchString(line) {
			blockStartCount++
		}
		if blockEndReg.MatchString(line) {
			blockEndCount++
		}

		blockStr += line
		if blockStartCount != blockEndCount {
			blockStr += "\n"
		}

		cmd := ""
		if blockStartCount > 0 && blockStartCount == blockEndCount {
			cmd = blockStr
		} else if blockStartCount > 0 {
			continue
		} else {
			blockStr = ""
			cmd = line
		}
		// println("==", cmd, "==")

		// 加载line，调用
		protectedCall(ls, cmd)

		blockStartCount = 0
		blockEndCount = 0
		blockStr = ""
	}
}

func loadString(ls api.LkState, cmd string) {
	ls.LoadString(cmd, "stdin")
	ls.Call(0, api.LK_MULTRET)
}

func catchErr(ls api.LkState, cmd string) {
	if err := recover(); err != nil {
		term.Red(fmt.Sprintf("%v\n", err), true)
	} else {
		// 更新历史记录
		updateHistory(cmd)
	}
}

func protectedCall(ls api.LkState, cmd string) {
	// 捕获错误
	defer catchErr(ls, cmd)
	loadString(ls, cmd)
}

func _updateHistory(str string) {
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

func updateHistory(str string) {
	str = strings.Trim(str, "\n")
	strs := strings.Split(str, "\n")
	for idx := range strs {
		_updateHistory(strs[idx])
	}
}
