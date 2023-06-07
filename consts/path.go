package consts

import "os"

var (
	LkPath = os.Getenv("LK_PATH")
)

const (
	BuiltinPrefix    = "@builtin/"
	BuiltinPrefixLen = len(BuiltinPrefix)
)
