package term

import (
	"fmt"
)

const (
	RED     = "\033[91m"
	GREEN   = "\033[32m"
	YELLOW  = "\033[93m"
	BLUE    = "\033[94m"
	MAGENTA = "\033[95m"
	CYAN    = "\033[96m"
	NOCOLOR = "\033[0m"
)

const (
	warn    = YELLOW + "[WAR]" + NOCOLOR + " "
	err     = RED + "[ERR]" + NOCOLOR + " "
	info    = CYAN + "[INF]" + NOCOLOR + " "
	success = GREEN + "[SUC]" + NOCOLOR + " "
	debug   = MAGENTA + "[DEBUG]" + NOCOLOR + " "
)

func printf(format string, args ...any) {
	f := fmt.Sprintf(format+"\n", args...)
	print(f)
}

func Warn(format string, args ...any) {
	printf(warn+format, args...)
}

func Yellow(format string, args ...any) {
	printf(YELLOW+format+NOCOLOR, args...)
}

func Info(format string, args ...any) {
	printf(info+format, args...)
}

func Cyan(format string, args ...any) {
	printf(CYAN+format+NOCOLOR, args...)
}

func Err(format string, args ...any) {
	printf(err+format, args...)
}

func Red(format string, args ...any) {
	printf(RED+format+NOCOLOR, args...)
}

func Suc(format string, args ...any) {
	printf(success+format, args...)
}

func Green(format string, args ...any) {
	printf(GREEN+format+NOCOLOR, args...)
}
