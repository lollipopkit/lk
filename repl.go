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

	println(` _     _  __  ____  _____ ____  _     
| |   | |/ / |  _ \| ____|  _ \| |    
| |   | ' /  | |_) |  _| | |_) | |    
| |___| . \  |  _ <| |___|  __/| |___ 
|_____|_|\_\ |_| \_\_____|_|   |_____|`)
	println("               v" + consts.VERSION)

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
}

func catchErr(ls api.LkState, first *bool, cmd string) {
	if err := recover(); err != nil {
		if *first {
			*first = false
			addPrintCmd := "print(" + cmd + ")"
			defer catchErr(ls, first, addPrintCmd)
			loadString(ls, addPrintCmd)
		} else {
			term.Warn(fmt.Sprintf("%v", err))
		}
	} else {
		// 更新历史记录
		updateHistory(cmd)
	}
}

func protectedCall(ls api.LkState, cmd string) {
	first := true
	// 捕获错误
	defer catchErr(ls, &first, cmd)
	loadString(ls, cmd)
	ls.PCall(0, api.LK_MULTRET, 0)
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
