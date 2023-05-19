package stdlib

import (
	"fmt"
	"regexp"
	"strings"

	. "github.com/lollipopkit/lk/api"
)

// tag = %[flags][width][.precision]specifier
var tagPattern = regexp.MustCompile(`%[ #+-0]?[0-9]*(\.[0-9]+)?[cdeEfgGioqsuxX%]`)

func parseFmtStr(fmt string) []string {
	if fmt == "" || strings.IndexByte(fmt, '%') < 0 {
		return []string{fmt}
	}

	parsed := make([]string, 0, len(fmt)/2)
	for {
		if fmt == "" {
			break
		}

		loc := tagPattern.FindStringIndex(fmt)
		if loc == nil {
			parsed = append(parsed, fmt)
			break
		}

		head := fmt[:loc[0]]
		tag := fmt[loc[0]:loc[1]]
		tail := fmt[loc[1]:]

		if head != "" {
			parsed = append(parsed, head)
		}
		parsed = append(parsed, tag)
		fmt = tail
	}
	return parsed
}

func _fmt(fmtStr string, ls LkState) string {
	argIdx := 1
	arr := parseFmtStr(fmtStr)
	for i := range arr {
		if arr[i][0] == '%' {
			if arr[i] == "%%" {
				arr[i] = "%"
			} else {
				argIdx += 1
				arr[i] = _fmtArg(arr[i], ls, argIdx)
			}
		}
	}
	return strings.Join(arr, "")
}

func _fmtArg(tag string, ls LkState, argIdx int) string {
	switch tag[len(tag)-1] { // specifier
	case 'c': // character
		return string([]byte{byte(ls.ToInteger(argIdx))})
	case 'i':
		tag = tag[:len(tag)-1] + "d" // %i -> %d
		return fmt.Sprintf(tag, ls.ToInteger(argIdx))
	case 'd', 'o': // integer, octal
		return fmt.Sprintf(tag, ls.ToInteger(argIdx))
	case 'u': // unsigned integer
		tag = tag[:len(tag)-1] + "d" // %u -> %d
		return fmt.Sprintf(tag, uint(ls.ToInteger(argIdx)))
	case 'x', 'X': // hex integer
		return fmt.Sprintf(tag, uint(ls.ToInteger(argIdx)))
	case 'f': // float
		return fmt.Sprintf(tag, ls.ToNumber(argIdx))
	case 's', 'q': // string
		return fmt.Sprintf(tag, ls.ToString2(argIdx))
	default:
		panic("todo! tag=" + tag)
	}
}

/* helper */

/* translate a relative string position: negative means back from end */
func posRelat(pos int64, _len int) int {
	_pos := int(pos)
	if _pos >= 0 {
		return _pos
	} else if -_pos > _len {
		return 0
	} else {
		return _len + _pos + 1
	}
}
