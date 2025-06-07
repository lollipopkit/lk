package stdlib

import . "github.com/lollipopkit/lk/api"

var listLib = map[string]GoFunction{
    "append": listAppend,
}

func OpenListLib(ls LkState) int {
    ls.NewLib(listLib)
    return 1
}

func listAppend(ls LkState) int {
    ls.CheckType(1, LK_TLIST)
    val := ls.CheckAny(2)
    // 实现 append
    return 0
}