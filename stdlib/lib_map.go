package stdlib

import . "github.com/lollipopkit/lk/api"

var mapLib = map[string]GoFunction{
}

func OpenMapLib(ls LkState) int {
    ls.NewLib(mapLib)
    return 1
}
