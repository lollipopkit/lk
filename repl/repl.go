package repl

import (
	"fmt"
	"os"
	"path/filepath"
	"strings"

	"slices"

	"atomicgo.dev/keyboard/keys"
	"github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/compiler/lexer"
	"github.com/lollipopkit/lk/compiler/parser"
	"github.com/lollipopkit/lk/consts"
	. "github.com/lollipopkit/lk/json"
	"github.com/lollipopkit/lk/state"
	"github.com/lollipopkit/lk/term"
	"github.com/lollipopkit/lk/utils"
)

var (
	linesHistory = []string{}
	helpMsgs     = []string{
		"`Esc`: Exit REPL",
		"`Tab`: Add 2 spaces",
		"",
		"`Ctrl + b`: Wrap current line with `print()`",
		"`Ctrl + n`: Wrap current line with `printf()`",
		"`Ctrl + a`: Clear REPL history",
		"",
		"`reset()`: Reset REPL state",
	}
	printRunesPre  = []rune("print(")
	printfRunesPre = []rune("printf(")
	printRunesSuf  = []rune(")")
	historyPath    = filepath.Join(os.Getenv("HOME"), ".config", "lk_history.json")
	ls             api.LkState
	blockLines     = []string{}
)

func newState() {
	ls = state.New()
	ls.OpenLibs()
	ls.Register("help", func(ls api.LkState) int {
		print(strings.Join(helpMsgs, "\n") + "\n")
		return 0
	})
	ls.Register("reset", func(_ api.LkState) int {
		newState()
		return 0
	})
	blockLines = []string{}
}

func Repl() {
	fmt.Printf(
		"lk (v%s) - %s for help\n",
		term.CYAN+consts.VERSION+term.NOCOLOR,
		term.GREEN+"`help()`"+term.NOCOLOR,
	)

	loadHistory()
	newState()

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

		// Call the code
		protectedCall(ls, blockStr)

		blockLines = []string{}
	}
}

func protectedCall(ls api.LkState, cmd string) {
	// Catch and print any errors
	defer ls.CatchAndPrint(true)

	warpedCmd := cmd
	// Check if input is just an expression, and if so wrap it with printf()
	if isExpression(cmd) {
		warpedCmd = "print(" + cmd + ")"
	}
	// term.Info("[REPL] Running: %s", warpedCmd)

	ls.LoadString(warpedCmd, "stdin")

	ls.PCall(0, api.LK_MULTRET, 1)
	updateHistory(cmd)
}

// isExpression checks if the input is an expression (not a statement)
func isExpression(input string) bool {
	defer func() {
		// If parser.ParseExp panics (e.g., on syntax error), recover.
		// The function will return the zero value for bool (false) if a panic occurs
		// before a successful return statement, effectively treating parsing errors
		// as "not an expression".
		recover()
	}()

	// Skip empty input
	if strings.TrimSpace(input) == "" {
		return false
	}

	// Check if it contains assignment
	if strings.Contains(input, "=") ||
		strings.Contains(input, "+=") ||
		strings.Contains(input, "-=") ||
		strings.Contains(input, "*=") ||
		strings.Contains(input, "/=") {
		return false
	}

	// Use the parser to try parsing as an expression
	l := lexer.NewLexer(input, "")
	expr := parser.ParseExp(l)

	// If we can parse it as an expression and there's no more tokens,
	// it's likely just an expression
	return expr != nil && l.LookAhead() == lexer.TOKEN_EOF
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
	case keys.CtrlA:
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
		linesHistory = slices.Delete(linesHistory, idx, idx+1)
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
	data, err := Json.MarshalIndent(linesHistory, "", "  ")
	if err != nil {
		term.Warn("[REPL] marshal history failed: %v", err)
	}
	if err := os.WriteFile(historyPath, data, 0644); err != nil {
		term.Warn("[REPL] write history failed: %v", err)
	}
}

func loadHistory() {
	if utils.Exist(historyPath) {
		data, err := os.ReadFile(historyPath)
		if err != nil {
			term.Warn("[REPL] read history failed: %v", err)
		}
		err = Json.Unmarshal(data, &linesHistory)
		if err != nil {
			term.Warn("[REPL] unmarshal history failed: %v", err)
		}
	} else {
		writeHistory()
	}
}
