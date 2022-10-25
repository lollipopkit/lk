package stdlib

import (
	"math"
	"math/rand"
	"time"

	. "git.lolli.tech/lollipopkit/lk/api"
)

var randLib = map[string]GoFunction{
	"random": randRandom,
	"seed":   randSeed,
}

func OpenRandLib(ls LkState) int {
	rand.Seed(time.Now().UnixMilli())
	ls.NewLib(randLib)
	return 1
}

/* pseudo-random numbers */

// rand.random ([m [, n]])
// http://www.lua.org/manual/5.3/manual.html#pdf-math.random
// lua-5.3.4/src/lmathlib.c#math_random()
func randRandom(ls LkState) int {
	var low, up int64
	argsNum := ls.GetTop()
	switch argsNum { /* check number of arguments */
	case 0: /* no arguments */
		ls.PushNumber(rand.Float64()) /* Number between 0 and 1 */
		return 1
	case 1: /* only upper limit */
		low = 1
		up = ls.CheckInteger(1)
	case 2: /* lower and upper limits */
		low = ls.CheckInteger(1)
		up = ls.CheckInteger(2)
	default:
		return ls.Error2("number of arguments out of range[0, 3]: %d", argsNum)
	}

	/* random integer in the interval [low, up] */
	ls.ArgCheck(low <= up, 1, "interval is empty")
	ls.ArgCheck(low >= 0 || up <= math.MaxInt64+low, 1,
		"interval too large")
	if up-low == math.MaxInt64 {
		ls.PushInteger(low + rand.Int63())
	} else {
		ls.PushInteger(low + rand.Int63n(up-low+1))
	}
	return 1
}

// rand.seed (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.randomseed
// lua-5.3.4/src/lmathlib.c#math_randomseed()
func randSeed(ls LkState) int {
	x := ls.CheckNumber(1)
	rand.Seed(int64(x))
	return 0
}
