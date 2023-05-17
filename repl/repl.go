package repl

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"atomicgo.dev/keyboard/keys"
	"github.com/lollipopkit/gommon/log"
	"github.com/lollipopkit/gommon/res"
	"github.com/lollipopkit/gommon/term"
	"github.com/lollipopkit/gommon/util"
	"github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/consts"
	"github.com/lollipopkit/lk/state"
	. "github.com/lollipopkit/lk/json"
)

var (
	linesHistory = []string{}
	helpMsgs     = []string{
		"`esc`: Exit REPL",
		"`Ctrl + b`: Wrap current line with `print()`",
		"`Ctrl + n`: Wrap current line with `printf()`",
		"`Ctrl + m`: Clear REPL hiistory",
		"`Tab`: Add 2 spaces",
	}
	printRunesPre  = []rune("print(")
	printfRunesPre = []rune("printf(")
	printRunesSuf  = []rune(")")
	historyPath    = filepath.Join(os.Getenv("HOME"), ".config", "lk_history.json")
)

func Repl() {
	loadHistory()
	ls := state.New()
	ls.OpenLibs()
	ls.Register("help", func(ls api.LkState) int {
		print(strings.Join(helpMsgs, "\n") + "\n")
		return 0
	})

	fmt.Printf(
		"REPL for lk (v%s) - enter %s for help\n",
		res.CYAN+consts.VERSION+res.NOCOLOR,
		res.GREEN+"`help()`"+res.NOCOLOR,
	)

	blockLines := []string{}

	for {
		line := term.ReadLine(term.ReadLineConfig{
			History: linesHistory,
			KeyFunc: handleKeyboard,
		})
		if line == "" {
			continue
		}

		blockLines = append(blockLines, line)
		blockStr := strings.Join(blockLines, "\n")
		if _blockNotEndCount(blockStr) != 0 {
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

func handleKeyboard(key keys.Key, rs *[]rune, rIdx *int, lIdx *int) (bool, bool, error) {
	switch key.Code {
	// wrap with `print()``
	case keys.CtrlB:
		*rs = append(printRunesPre, append(*rs, printRunesSuf...)...)
		*rIdx = len(*rs)
		return false, true, nil
	// wrap with `printf`
	case keys.CtrlN:
		*rs = append(printfRunesPre, append(*rs, printRunesSuf...)...)
		*rIdx = len(*rs)
		return false, true, nil
	case keys.Esc:
		os.Exit(0)
	case keys.CtrlM:
		linesHistory = []string{}
		writeHistory()
	}
	return false, false, nil
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
	writeHistory()
}

func _blockNotEndCount(block string) int {
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
	return start - end
}

func writeHistory() {
	data, err := Json.Marshal(linesHistory)
	if err != nil {
		log.Warn("[REPL] marshal history failed")
	}
	if err := os.WriteFile(historyPath, data, 0644); err != nil {
		log.Warn("[REPL] write history failed")
	}
}

func loadHistory() {
	if util.Exist(historyPath) {
		data, err := os.ReadFile(historyPath)
		if err != nil {
			log.Warn("[REPL] read history failed")
		}
		err = Json.Unmarshal(data, &linesHistory)
		if err != nil {
			log.Warn("[REPL] unmarshal history failed")
		}
	} else {
		writeHistory()
	}
}