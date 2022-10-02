package stdlib

import (
	"io/fs"
	"os"
	"os/exec"
	"time"

	. "git.lolli.tech/lollipopkit/lk/api"
)

var sysLib = map[string]GoFunction{
	"time":  osTime,
	"date":  osDate,
	"rm":    osRemove,
	"mv":    osRename,
	"link":  osLink,
	"tmp":   osTmpName,
	"env":   osGetEnv,
	"exec":  osExecute,
	"exit":  osExit,
	"ls":    osLs,
	"read":  osRead,
	"write": osWrite,
	"sleep": osSleep,
	"mkdir": osMkdir,
}

func OpenOSLib(ls LkState) int {
	ls.NewLib(sysLib)
	pushArgs(ls)
	return 1
}

func pushArgs(ls LkState) {
	args := os.Args
	ags := make([]any, len(args))
	for argIdx := range args {
		ags = append(ags, args[argIdx])
	}
	pushList(ls, ags)
	ls.SetField(-2, "args")
}

func osLink(ls LkState) int {
	src := ls.CheckString(1)
	dst := ls.CheckString(2)
	if err := os.Link(src, dst); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

func osMkdir(ls LkState) int {
	path := ls.CheckString(1)
	rescusive := ls.OptBool(2, false)
	perm := fs.FileMode(ls.OptInteger(3, 0744))
	if rescusive {
		err := os.MkdirAll(path, perm)
		if err != nil {
			ls.PushString(err.Error())
			return 1
		}
	} else if err := os.Mkdir(path, perm); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

func osSleep(ls LkState) int {
	milliSec := ls.CheckInteger(1)
	time.Sleep(time.Duration(milliSec) * time.Millisecond)
	return 0
}

func osLs(ls LkState) int {
	dir := ls.CheckString(1)
	files, err := os.ReadDir(dir)
	if err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	filenames := make([]any, 0, len(files))
	for i := range files {
		filenames = append(filenames, files[i].Name())
	}
	pushList(ls, filenames)
	ls.PushNil()
	return 2
}

func osRead(ls LkState) int {
	path := ls.CheckString(1)
	data, err := os.ReadFile(path)
	if err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	ls.PushString(string(data))
	ls.PushNil()
	return 2
}

func osWrite(ls LkState) int {
	path := ls.CheckString(1)
	data := ls.CheckString(2)
	perm := fs.FileMode(ls.OptInteger(3, 0744))
	if err := os.WriteFile(path, []byte(data), perm); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

// os.time ([table, isUTC])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.time
// lua-5.3.4/src/loslib.c#os_time()
func osTime(ls LkState) int {
	if ls.IsNoneOrNil(1) { /* called without args? */
		t := time.Now().Unix() /* get current time */
		ls.PushInteger(t)
	} else {
		ls.CheckType(1, LUA_TTABLE)
		isUTC := ls.OptBool(2, false)
		sec := _getField(ls, "sec", 0)
		min := _getField(ls, "min", 0)
		hour := _getField(ls, "hour", 12)
		day := _getField(ls, "day", -1)
		month := _getField(ls, "month", -1)
		year := _getField(ls, "year", -1)
		loc := func() *time.Location {
			if isUTC {
				return time.UTC
			}
			return time.Local
		}()
		t := time.Date(year, time.Month(month), day,
			hour, min, sec, 0, loc).Unix()
		ls.PushInteger(t)
	}
	return 1
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

// os.remove (filename, [rmdir])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.remove
func osRemove(ls LkState) int {
	filename := ls.CheckString(1)
	rmdir := ls.OptBool(2, false)
	if rmdir {
		err := os.RemoveAll(filename)
		if err != nil {
			ls.PushString(err.Error())
			return 1
		}
		goto SUC
	}
	if err := os.Remove(filename); err != nil {
		ls.PushString(err.Error())
		return 1
	}
SUC:
	ls.PushNil()
	return 1
}

// os.rename (oldname, newname)
// http://www.lua.org/manual/5.3/manual.html#pdf-os.rename
func osRename(ls LkState) int {
	oldName := ls.CheckString(1)
	newName := ls.CheckString(2)
	if err := os.Rename(oldName, newName); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
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
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	ls.PushString(string(out))
	ls.PushNil()
	return 2
}

// os.exit ([code])
// http://www.lua.org/manual/5.3/manual.html#pdf-os.exit
// lua-5.3.4/src/loslib.c#os_exit()
func osExit(ls LkState) int {
	code := ls.OptInteger(1, 0)
	os.Exit(int(code))
	return 0
}
