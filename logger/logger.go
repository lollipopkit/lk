package logger

import (
	"fmt"

	"git.lolli.tech/lollipopkit/lk/consts"
)

func I(fm string, a ...any) {
	if consts.Debug {
		s := fmt.Sprintf("[INFO] %s\n", fm)
		fmt.Printf(s, a...)
	}
}

func E(fm string, a ...any) {
	if consts.Debug {
		s := fmt.Sprintf("[ERROR] %s\n", fm)
		fmt.Printf(s, a...)
	}
}

func W(fm string, a ...any) {
	if consts.Debug {
		s := fmt.Sprintf("[WARN] %s\n", fm)
		fmt.Printf(s, a...)
	}
}
