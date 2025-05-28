package stdlib

import (
	"math"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/utils"
)

var mathLib = map[string]GoFunction{
	"max":   mathMax,
	"min":   mathMin,
	"exp":   mathExp,
	"log":   mathLog,
	"deg":   mathDeg,
	"rad":   mathRad,
	"sin":   mathSin,
	"cos":   mathCos,
	"tan":   mathTan,
	"asin":  mathAsin,
	"acos":  mathAcos,
	"atan":  mathAtan,
	"ceil":  mathCeil,
	"floor": mathFloor,
	"fmod":  mathFmod,
	"modf":  mathModf,
	"abs":   mathAbs,
	"sqrt":  mathSqrt,
	"ult":   mathUlt,
	"type":  mathType,
}

func OpenMathLib(ls LkState) int {
	ls.NewLib(mathLib)
	ls.PushNumber(math.Pi)
	ls.SetField(-2, "pi")
	ls.PushNumber(math.Inf(1))
	ls.SetField(-2, "huge")
	ls.PushInteger(math.MaxInt)
	ls.SetField(-2, "maxint")
	ls.PushInteger(math.MinInt)
	ls.SetField(-2, "minint")
	return 1
}

/* max & min */

// math.max (x, ···)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.max
// lua-5.3.4/src/lmathlib.c#math_max()
func mathMax(ls LkState) int {
	n := ls.GetTop() /* utils of arguments */
	imax := 1        /* index of current maximum value */
	ls.ArgCheck(n >= 1, 1, "value expected")
	for i := 2; i <= n; i++ {
		if ls.Compare(imax, i, LK_OPLT) {
			imax = i
		}
	}
	ls.PushValue(imax)
	return 1
}

// math.min (x, ···)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.min
// lua-5.3.4/src/lmathlib.c#math_min()
func mathMin(ls LkState) int {
	n := ls.GetTop() /* utils of arguments */
	imin := 1        /* index of current minimum value */
	ls.ArgCheck(n >= 1, 1, "value expected")
	for i := 2; i <= n; i++ {
		if ls.Compare(i, imin, LK_OPLT) {
			imin = i
		}
	}
	ls.PushValue(imin)
	return 1
}

/* exponentiation and logarithms */

// math.exp (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.exp
// lua-5.3.4/src/lmathlib.c#math_exp()
func mathExp(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Exp(x))
	return 1
}

// math.log (x [, base])
// http://www.lua.org/manual/5.3/manual.html#pdf-math.log
// lua-5.3.4/src/lmathlib.c#math_log()
func mathLog(ls LkState) int {
	x := ls.CheckNumber(1)
	var res float64

	if ls.IsNoneOrNil(2) {
		res = math.Log(x)
	} else {
		base := ls.ToNumber(2)
		if base == 2 {
			res = math.Log2(x)
		} else if base == 10 {
			res = math.Log10(x)
		} else {
			res = math.Log(x) / math.Log(base)
		}
	}

	ls.PushNumber(res)
	return 1
}

/* trigonometric functions */

// math.deg (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.deg
// lua-5.3.4/src/lmathlib.c#math_deg()
func mathDeg(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(x * 180 / math.Pi)
	return 1
}

// math.rad (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.rad
// lua-5.3.4/src/lmathlib.c#math_rad()
func mathRad(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(x * math.Pi / 180)
	return 1
}

// math.sin (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.sin
// lua-5.3.4/src/lmathlib.c#math_sin()
func mathSin(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Sin(x))
	return 1
}

// math.cos (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.cos
// lua-5.3.4/src/lmathlib.c#math_cos()
func mathCos(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Cos(x))
	return 1
}

// math.tan (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.tan
// lua-5.3.4/src/lmathlib.c#math_tan()
func mathTan(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Tan(x))
	return 1
}

// math.asin (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.asin
// lua-5.3.4/src/lmathlib.c#math_asin()
func mathAsin(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Asin(x))
	return 1
}

// math.acos (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.acos
// lua-5.3.4/src/lmathlib.c#math_acos()
func mathAcos(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Acos(x))
	return 1
}

// math.atan (y [, x])
// http://www.lua.org/manual/5.3/manual.html#pdf-math.atan
// lua-5.3.4/src/lmathlib.c#math_atan()
func mathAtan(ls LkState) int {
	y := ls.CheckNumber(1)
	x := ls.OptNumber(2, 1.0)
	ls.PushNumber(math.Atan2(y, x))
	return 1
}

/* rounding functions */

// math.ceil (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.ceil
// lua-5.3.4/src/lmathlib.c#math_ceil()
func mathCeil(ls LkState) int {
	if ls.IsInteger(1) {
		ls.SetTop(1) /* integer is its own ceil */
	} else {
		x := ls.CheckNumber(1)
		_pushNumInt(ls, math.Ceil(x))
	}
	return 1
}

// math.floor (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.floor
// lua-5.3.4/src/lmathlib.c#math_floor()
func mathFloor(ls LkState) int {
	if ls.IsInteger(1) {
		ls.SetTop(1) /* integer is its own floor */
	} else {
		x := ls.CheckNumber(1)
		_pushNumInt(ls, math.Floor(x))
	}
	return 1
}

/* others */

// math.fmod (x, y)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.fmod
// lua-5.3.4/src/lmathlib.c#math_fmod()
func mathFmod(ls LkState) int {
	if ls.IsInteger(1) && ls.IsInteger(2) {
		d := ls.ToInteger(2)
		if uint64(d)+1 <= 1 { /* special cases: -1 or 0 */
			ls.ArgCheck(d != 0, 2, "zero")
			ls.PushInteger(0) /* avoid overflow with 0x80000... / -1 */
		} else {
			ls.PushInteger(ls.ToInteger(1) % d)
		}
	} else {
		x := ls.CheckNumber(1)
		y := ls.CheckNumber(2)
		ls.PushNumber(x - math.Trunc(x/y)*y)
	}

	return 1
}

// math.modf (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.modf
// lua-5.3.4/src/lmathlib.c#math_modf()
func mathModf(ls LkState) int {
	if ls.IsInteger(1) {
		ls.SetTop(1)     /* utils is its own integer part */
		ls.PushNumber(0) /* no fractional part */
	} else {
		x := ls.CheckNumber(1)
		i, f := math.Modf(x)
		_pushNumInt(ls, i)
		if math.IsInf(x, 0) {
			ls.PushNumber(0)
		} else {
			ls.PushNumber(f)
		}
	}

	return 2
}

// math.abs (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.abs
// lua-5.3.4/src/lmathlib.c#math_abs()
func mathAbs(ls LkState) int {
	if ls.IsInteger(1) {
		x := ls.ToInteger(1)
		if x < 0 {
			ls.PushInteger(-x)
		}
	} else {
		x := ls.CheckNumber(1)
		ls.PushNumber(math.Abs(x))
	}
	return 1
}

// math.sqrt (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.sqrt
// lua-5.3.4/src/lmathlib.c#math_sqrt()
func mathSqrt(ls LkState) int {
	x := ls.CheckNumber(1)
	ls.PushNumber(math.Sqrt(x))
	return 1
}

// math.ult (m, n)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.ult
// lua-5.3.4/src/lmathlib.c#math_ult()
func mathUlt(ls LkState) int {
	m := ls.CheckInteger(1)
	n := ls.CheckInteger(2)
	ls.PushBoolean(uint64(m) < uint64(n))
	return 1
}

// math.type (x)
// http://www.lua.org/manual/5.3/manual.html#pdf-math.type
// lua-5.3.4/src/lmathlib.c#math_type()
func mathType(ls LkState) int {
	if ls.Type(1) == LK_TNUMBER {
		if ls.IsInteger(1) {
			ls.PushString("integer")
		} else {
			ls.PushString("float")
		}
	} else {
		ls.CheckAny(1)
		ls.PushNil()
	}
	return 1
}

func _pushNumInt(ls LkState, d float64) {
	if i, ok := utils.FloatToInteger(d); ok { /* does 'd' fit in an integer? */
		ls.PushInteger(i) /* result is integer */
	} else {
		ls.PushNumber(d) /* result is float */
	}
}
