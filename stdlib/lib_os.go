package stdlib

//#include <time.h>
import "C"

import (
	"io/fs"
	"os"
	"os/exec"
	"strings"
	"time"

	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
)

var sysLib = map[string]GoFunction{
	"time":  osTime,
	"date":  osDate,
	"rm":    osRemove,
	"mv":    osRename,
	"tmp":   osTmpName,
	"env":   osGetEnv,
	"exec":  osExecute,
	"exit":  osExit,
	"dir":   osDir,
	"read":  osRead,
	"write": osWrite,
}

func OpenOSLib(ls LkState) int {
	ls.NewLib(sysLib)
	return 1
}

func osDir(ls LkState) int {
	dir := ls.CheckString(1)
	files, err := os.ReadDir(dir)
	if err != nil {
		ls.PushNil()
		return 1
	}
	ls.CreateTable(len(files), 0)
	for i, file := range files {
		ls.PushString(file.Name())
		ls.SetI(-2, int64(i+1))
	}
	return 1
}

func osRead(ls LkState) int {
	path := ls.CheckString(1)
	data, err := os.ReadFile(path)
	if err != nil {
		ls.PushNil()
		return 1
	}
	ls.PushString(string(data))
	return 1
}

func dirName(path string) string {
	if strings.Contains(path, "/") {
		return path[:strings.LastIndex(path, "/")]
	}
	return ""
}

func osWrite(ls LkState) int {
	path := ls.CheckString(1)
	data := ls.CheckString(2)
	perm := fs.FileMode(ls.OptInteger(3, 0744))
	dir := dirName(path)
	if dir != "" {
		os.MkdirAll(dir, perm)
	}
	if err := os.WriteFile(path, []byte(data), perm); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

// os.time ([table])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.time
// lua-5.3.4/src/loslib.c#os_time()
func osTime(ls LkState) int {
	if ls.IsNoneOrNil(1) { /* called without args? */
		t := time.Now().Unix() /* get current time */
		ls.PushInteger(t)
	} else {
		ls.CheckType(1, LUA_TTABLE)
		sec := _getField(ls, "sec", 0)
		min := _getField(ls, "min", 0)
		hour := _getField(ls, "hour", 12)
		day := _getField(ls, "day", -1)
		month := _getField(ls, "month", -1)
		year := _getField(ls, "year", -1)
		// todo: isdst
		t := time.Date(year, time.Month(month), day,
			hour, min, sec, 0, time.Local).Unix()
		ls.PushInteger(t)
	}
	return 1
}

// lua-5.3.4/src/loslib.c#getfield()
func _getField(ls LkState, key string, dft int64) int {
	t := ls.GetField(-1, key) /* get field and its type */
	res, isNum := ls.ToIntegerX(-1)
	if !isNum { /* field is not an integer? */
		if t != LUA_TNIL { /* some other value? */
			return ls.Error2("field '%s' is not an integer", key)
		} else if dft < 0 { /* absent field; no default? */
			return ls.Error2("field '%s' missing in date table", key)
		}
		res = dft
	}
	ls.Pop(1)
	return int(res)
}

// os.date ([format [, time]])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.date
// lua-5.3.4/src/loslib.c#os_date()
func osDate(ls LkState) int {
	format := ls.OptString(1, "%c")
	var t time.Time
	if ls.IsInteger(2) {
		t = time.Unix(ls.ToInteger(2), 0)
	} else {
		t = time.Now()
	}

	if format != "" && format[0] == '!' { /* UTC? */
		format = format[1:] /* skip '!' */
		t = t.In(time.UTC)
	}

	if format == "*t" {
		ls.CreateTable(0, 9) /* 9 = number of fields */
		_setField(ls, "sec", t.Second())
		_setField(ls, "min", t.Minute())
		_setField(ls, "hour", t.Hour())
		_setField(ls, "day", t.Day())
		_setField(ls, "month", int(t.Month()))
		_setField(ls, "year", t.Year())
		_setField(ls, "wday", int(t.Weekday())+1)
		_setField(ls, "yday", t.YearDay())
	} else if format == "%c" {
		ls.PushString(t.Format(time.ANSIC))
	} else {
		ls.PushString(format) // TODO
	}

	return 1
}

func _setField(ls LkState, key string, value int) {
	ls.PushInteger(int64(value))
	ls.SetField(-2, key)
}

// os.remove (filename)
// http://www.lua.org/manual/5.3/manual.html#pdf-os.remove
func osRemove(ls LkState) int {
	filename := ls.CheckString(1)
	if err := os.Remove(filename); err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	} else {
		ls.PushBoolean(true)
		return 1
	}
}

// os.rename (oldname, newname)
// http://www.lua.org/manual/5.3/manual.html#pdf-os.rename
func osRename(ls LkState) int {
	oldName := ls.CheckString(1)
	newName := ls.CheckString(2)
	if err := os.Rename(oldName, newName); err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	} else {
		ls.PushBoolean(true)
		return 1
	}
}

// os.tmpname ()
// http://www.lua.org/manual/5.3/manual.html#pdf-os.tmpname
func osTmpName(ls LkState) int {
	ls.PushString(os.TempDir())
	return 1
}

// os.getenv (varname)
// http://www.lua.org/manual/5.3/manual.html#pdf-os.getenv
// lua-5.3.4/src/loslib.c#os_getenv()
func osGetEnv(ls LkState) int {
	key := ls.CheckString(1)
	if env := os.Getenv(key); env != "" {
		ls.PushString(env)
	} else {
		ls.PushNil()
	}
	return 1
}

// os.exec (exe, [args...])
func osExecute(ls LkState) int {
	exe := ls.CheckString(1)
	args := make([]string, 0, ls.GetTop()-1)
	for i := 2; i <= ls.GetTop(); i++ {
		args = append(args, ls.CheckString(i))
	}
	cmd := exec.Command(exe, args...)
	out, err := cmd.Output()
	if err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushString(string(out))
	return 1
}

// os.exit ([code [, close]])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.exit
// lua-5.3.4/src/loslib.c#os_exit()
func osExit(ls LkState) int {
	if ls.IsBoolean(1) {
		if ls.ToBoolean(1) {
			os.Exit(0)
		} else {
			os.Exit(1) // todo
		}
	} else {
		code := ls.OptInteger(1, 1)
		os.Exit(int(code))
	}
	if ls.ToBoolean(2) {
		//ls.Close()
	}
	return 0
}
