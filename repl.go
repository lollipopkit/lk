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
	linesHistory       = []string{}
	printReg           = regexp.MustCompile(`print\(.*\)`)
)

func repl(wg *sync.WaitGroup) {
	ls := state.New()
	ls.OpenLibs()
	wg.Wait()

	term.Cyan("LK REPL (v" + consts.VERSION + ")\n")

	blockStr := ""

	for {
		line := term.ReadLine(linesHistory)
		if line == "" {
			continue
		}

		blockStr += line + "\n"
		if !_isBlockEnd(blockStr) {
			continue
		}

		// 加载line，调用
		protectedCall(ls, blockStr)

		blockStr = ""
	}
}

func loadString(ls api.LkState, cmd string) {
	// term.Green(cmd + "\n")
	ls.LoadString(cmd, "stdin")
}

func catchErr(ls api.LkState, first *bool, cmd string) {
	err := recover()
	if err != nil {
		if *first {
			*first = false
			defer catchErr(ls, first, cmd)
			loadString(ls, cmd)
			ls.PCall(0, api.LK_MULTRET, 0)
		} else {
			term.Warn(fmt.Sprintf("%v", err))
		}
	}
}

func protectedCall(ls api.LkState, cmd string) {
	havePrint := printReg.MatchString(cmd)
	first := !havePrint
	// 捕获错误
	defer catchErr(ls, &first, cmd)
	
	if havePrint {
		loadString(ls, cmd)
	} else {
		loadString(ls, "print(" + cmd + ")")
	}
	
	ls.PCall(0, api.LK_MULTRET, 0)
	updateHistory(cmd)
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

func _isBlockEnd(block string) bool {
	start := 0
	end := 0
	inStr := false
	var lastPairChar rune
	for idx, c := range block {
		switch c {
		case '{':
			if inStr {
				continue
			}
			start++
		case '}':
			if inStr {
				continue
			}
			end++
		case '\'', '"', '`':
			if idx == 0 || block[idx-1] != '\\' {
				if lastPairChar == c {
					inStr = !inStr
				} else if !inStr {
					inStr = true
					lastPairChar = c
				}
			}
		}
	}
	if inStr {
		return false
	}
	return start == end
}
