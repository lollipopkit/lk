package term

import (
	"os"
)

const (
	RED     = "\033[31m"
	GREEN   = "\033[32m"
	YELLOW  = "\033[33m"
	BLUE    = "\033[34m"
	CYAN    = "\033[36m"
	WHITE   = "\033[37m"
	NOCOLOR = "\033[0m"
)

func print(s string) {
	os.Stdout.WriteString(s + NOCOLOR)
}

func Red(s string, noPanic ...bool) {
	if len(noPanic) > 0 && noPanic[0] {
		print(RED + s)
	} else {
		panic(s)
	}
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
