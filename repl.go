package main

import (
	"fmt"
	"regexp"
	"strings"

	"github.com/lollipopkit/gommon/log"
	"github.com/lollipopkit/gommon/term"
	"github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/consts"
	"github.com/lollipopkit/lk/state"
)

var (
	linesHistory = []string{}
	printReg     = regexp.MustCompile(`print\(.*\)`)
)

func repl() {
	ls := state.New()
	ls.OpenLibs()

	log.Cyan("REPL for LK (v" + consts.VERSION + ")\n")

	blockLines := []string{}

	for {
		line := term.ReadLine(term.ReadLineConfig{
			History: linesHistory,
		})
		if line == "" {
			continue
		}

		blockLines = append(blockLines, line)
		blockStr := strings.Join(blockLines, "\n")
		if !_isBlockEnd(blockStr) {
			continue
		}

		// 加载line，调用
		protectedCall(ls, blockStr)

		blockLines = []string{}
	}
}

func catchErr(ls api.LkState, cmd string) {
	err := recover()
	if err != nil {
		log.Red(fmt.Sprintf("%v\n", err))
	}
}

func protectedCall(ls api.LkState, cmd string) {
	// 捕获错误
	defer catchErr(ls, cmd)

	//log.Green(">>> " + cmd)
	ls.LoadString(cmd, "stdin")

	ls.PCall(0, api.LK_MULTRET, 1)
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
