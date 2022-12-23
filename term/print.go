package term

import (
	"os"
	"strings"
)

const (
	RED     = "\033[91m"
	GREEN   = "\033[92m"
	YELLOW  = "\033[93m"
	BLUE    = "\033[94m"
	CYAN    = "\033[96m"
	WHITE   = "\033[97m"
	NOCOLOR = "\033[0m"
)

func print(s string) {
	os.Stdout.WriteString(s + NOCOLOR)
}

func addBorder(s, title string) string {
	lines := strings.Split(s, "\n")
	longest := 4
	for idx := range lines {
		if len(lines[idx]) > longest {
			longest = len(lines[idx])
		}
	}

	w := longest + 6
	titleW := len(title)
	if w < titleW {
		w = titleW
	}
	result := "╔═ " + title + " " + strings.Repeat("═", w-titleW-3) + "╗\n"
	for idx := range lines {
		blankWidth := w - len(lines[idx])
		blank := strings.Repeat(" ", blankWidth/2)
		moreBlank := strings.Repeat(" ", blankWidth%2)
		result += "║" + blank + lines[idx] + blank + moreBlank + "║\n"
	}
	result += "╚" + strings.Repeat("═", w) + "╝\n"
	return result
}

func Warn(s string) {
	Yellow(addBorder(s, "Warn"))
}

func Error(s string, noPanic ...bool) {
	Red(addBorder(s, "Error"))
	if len(noPanic) > 0 && !noPanic[0] {
		panic(s)
	}
}

func Info(s string) {
	Cyan(addBorder(s, "Info"))
}

func Red(s string) {
	print(RED + s)
}

func Green(s string) {
	print(GREEN + s)
}

func Yellow(s string) {
	print(YELLOW + s)
}

func Blue(s string) {
	print(BLUE + s)
}

func Cyan(s string) {
	print(CYAN + s)
}

func White(s string) {
	print(WHITE + s)
}
