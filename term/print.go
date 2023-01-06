package term

import (
	"os"
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

func pri(s string) {
	os.Stdout.WriteString(s + NOCOLOR + "\n")
}

func Red(s string) {
	pri(RED + s)
}

func Green(s string) {
	pri(GREEN + s)
}

func Yellow(s string) {
	pri(YELLOW + s)
}

func Blue(s string) {
	pri(BLUE + s)
}

func Cyan(s string) {
	pri(CYAN + s)
}

func White(s string) {
	pri(WHITE + s)
}
