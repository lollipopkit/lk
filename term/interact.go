package term

import (
	"fmt"
	"os"
	"regexp"
	"strconv"
	"strings"

	"atomicgo.dev/cursor"
	"atomicgo.dev/keyboard"
	"atomicgo.dev/keyboard/keys"
)

var (
	doubleByteCharacterRegexp = regexp.MustCompile(`[^\x00-\xff]`)
	emptyStringList           = []string{}
)

const (
	_prompt = "> "
)

type KeyListenFunc func(
	key keys.Key,
	rs *[]rune,
	rIdx *int,
	lIdx *int,
) (
	stop bool,
	reset bool,
	err error,
)

type ReadLineConfig struct {
	// History is the history of lines.
	History []string
	// Prompt is the prompt to show.
	Prompt string
	// KeyFunc is the function to handle key press.
	// rs is the current line runes.
	// rIdx is the current rune index.
	// lIdx is the current line index.
	KeyFunc KeyListenFunc
}

func ReadLine(config ReadLineConfig) string {
	if len(config.Prompt) == 0 {
		config.Prompt = _prompt
	}
	if config.History == nil {
		config.History = emptyStringList
	}
	os.Stdout.WriteString(config.Prompt)
	rs := []rune{}
	linesIdx := len(config.History)
	runeIdx := 0

	keyboard.Listen(func(key keys.Key) (stop bool, err error) {
		switch key.Code {
		default:
			if config.KeyFunc != nil {
				stop, reset, err := config.KeyFunc(key, &rs, &runeIdx, &linesIdx)
				if reset {
					resetLine(rs, config.Prompt)
				}
				return stop, err
			}
		case keys.CtrlC:
			exit()
		case keys.RuneKey:
			runes := key.Runes
			rs = append(rs[:runeIdx], append(runes, rs[runeIdx:]...)...)
			runeIdx += len(runes)
			resetLine(rs, config.Prompt)
		case keys.Enter:
			println()
			return true, nil
		case keys.Backspace:
			if len(rs) > 0 && runeIdx > 0 {
				rs = append(rs[:runeIdx-1], rs[runeIdx:]...)
				resetLine(rs, config.Prompt)
				runeIdx--
			}
		case keys.Left:
			if runeIdx > 0 {
				runeIdx--
			}
		case keys.Right:
			if runeIdx < len(rs) {
				runeIdx++
			}
		case keys.Up:
			if linesIdx > 0 {
				linesIdx--
				rs = []rune(config.History[linesIdx])
				resetLine(rs, config.Prompt)
				runeIdx = len(rs)
			}
		case keys.Down:
			if linesIdx < len(config.History)-1 {
				linesIdx++
				rs = []rune(config.History[linesIdx])
				resetLine(rs, config.Prompt)
				runeIdx = len(rs)
			} else if linesIdx == len(config.History)-1 {
				linesIdx++
				rs = []rune("")
				resetLine(rs, config.Prompt)
				runeIdx = 0
			}
		case keys.Space:
			if runeIdx == len(rs) {
				rs = append(rs, ' ')
				print(" ")
				runeIdx++
			} else {
				rs = append(rs[:runeIdx], append([]rune(" "), rs[runeIdx:]...)...)
				resetLine(rs, config.Prompt)
				runeIdx++
			}
		case keys.Tab:
			if runeIdx == len(rs) {
				rs = append(rs, '\t')
				print("\t")
				runeIdx++
			} else {
				rs = append(rs[:runeIdx], append([]rune("\t"), rs[runeIdx:]...)...)
				resetLine(rs, config.Prompt)
				runeIdx++
			}
		case keys.Delete:
			if runeIdx < len(rs) {
				rs = append(rs[:runeIdx], rs[runeIdx+1:]...)
				resetLine(rs, config.Prompt)
			}
		}

		idx := calcIdx(rs, runeIdx)
		pRunes := []rune(config.Prompt)
		pIdx := calcIdx(pRunes, len(pRunes))
		cursor.HorizontalAbsolute(idx + pIdx)
		return false, nil
	})
	return string(rs)
}

func resetLine(rs []rune, prompt string) {
	cursor.ClearLine()
	cursor.StartOfLine()
	print(prompt + string(rs))
}

func calcIdx(rs []rune, runeIdx int) int {
	idx := 0
	for rIdx, r := range rs {
		if rIdx >= runeIdx {
			break
		}
		if isHan(r) {
			idx += 2
		} else {
			idx++
		}
	}
	return idx
}

func isHan(r rune) bool {
	return doubleByteCharacterRegexp.MatchString(string(r))
}

func exit() {
	os.Exit(0)
}

func Confirm(question string, default_ bool) bool {
	suffix := func() string {
		if default_ {
			return " [Y/n]"
		}
		return " [y/N]"
	}()

	input := ReadLine(ReadLineConfig{
		Prompt: fmt.Sprintf("%s%s: ", question, suffix),
	})
	if input == "" {
		return default_
	}
	return strings.ToLower(input) == "y"
}

func Option(question string, options []string, default_ int) int {
	println()
	for i := range options {
		print(fmt.Sprintf("%d. %s\n", i+1, options[i]))
	}
	suffix := fmt.Sprintf("[default %d]", default_+1)

	input := ReadLine(ReadLineConfig{
		Prompt: fmt.Sprintf("%s %s:", question, suffix),
	})
	if input == "" {
		return default_
	}
	inputIdx, err := strconv.Atoi(input)
	if err != nil {
		return default_
	}
	return inputIdx - 1
}