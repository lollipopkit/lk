package term

import (
	"os"

	"atomicgo.dev/cursor"
	"atomicgo.dev/keyboard"
	"atomicgo.dev/keyboard/keys"
)

const (
	prompt = "➜ "
)

var (
	promptLen = len([]rune(prompt))
)

func ReadLine(linesHistory []string) string {
	os.Stdout.WriteString(prompt)
	str := ""
	linesIdx := len(linesHistory)
	cursorIdx := 0

	keyboard.Listen(func(key keys.Key) (stop bool, err error) {
		switch key.Code {
		case keys.CtrlC, keys.Escape:
			os.Exit(0)
		case keys.RuneKey:
			runes := key.Runes
			s := string(runes)
			str = str[:cursorIdx] + s + str[cursorIdx:]
			cursorIdx += len(s)
			resetLine(str)
		case keys.Enter:
			println()
			return true, nil
		case keys.Backspace:
			if len(str) > 0 && cursorIdx > 0 {
				str = str[:cursorIdx-1] + str[cursorIdx:]
				resetLine(str)
				cursorIdx--
			}
		case keys.Left:
			if cursorIdx > 0 {
				cursorIdx--
			}
		case keys.Right:
			if cursorIdx < len(str) {
				cursorIdx++
			}
		case keys.Up:
			if linesIdx > 0 {
				linesIdx--
				str = linesHistory[linesIdx]
				resetLine(str)
				cursorIdx = len(str)
			}
		case keys.Down:
			if linesIdx < len(linesHistory)-1 {
				linesIdx++
				str = linesHistory[linesIdx]
				resetLine(str)
				cursorIdx = len(str)
			} else if linesIdx == len(linesHistory)-1 {
				str = ""
				resetLine("")
				cursorIdx = 0
			}
		case keys.Space:
			if cursorIdx == len(str) {
				str += " "
				print(" ")
				cursorIdx++
			} else {
				str = str[:cursorIdx] + " " + str[cursorIdx:]
				resetLine(str)
				cursorIdx++
			}
		case keys.Tab:
			if cursorIdx == len(str) {
				str += "  "
				print("  ")
				cursorIdx += 2
			} else {
				str = str[:cursorIdx] + "  " + str[cursorIdx:]
				resetLine(str)
				cursorIdx += 2
			}
		case keys.Delete:
			if cursorIdx < len(str) {
				str = str[:cursorIdx] + str[cursorIdx+1:]
				resetLine(str)
			}
		}

		cursor.HorizontalAbsolute(cursorIdx + promptLen)
		return false, nil
	})
	return str
}

func resetLine(str string) {
	cursor.ClearLine()
	cursor.StartOfLine()
	print(prompt + str)
}
