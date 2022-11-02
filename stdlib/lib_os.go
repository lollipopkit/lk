package stdlib

import (
	"bytes"
	"io/fs"
	"io/ioutil"
	"os"
	"os/exec"
	"path"
	"strings"
	"time"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/utils"
)

var sysLib = map[string]GoFunction{
	"time":    osTime,
	"stat":    osStat,
	"date":    osDate,
	"rm":      osRemove,
	"mv":      osRename,
	"cp":      osCp,
	"link":    osLink,
	"tmp":     osTmpName,
	"get_env": osGetEnv,
	"set_env": osSetEnv,
	"exec":    osExecute,
	"exit":    osExit,
	"ls":      osLs,
	"read":    osRead,
	"write":   osWrite,
	"sleep":   osSleep,
	"mkdir":   osMkdir,
}

func OpenOSLib(ls LkState) int {
	ls.NewLib(sysLib)
	pushArgs(ls)
	return 1
}

func pushArgs(ls LkState) {
	pushList(ls, os.Args)
	ls.SetField(-2, "args")
}

func osCp(ls LkState) int {
	src := ls.CheckString(1)
	dst := ls.CheckString(2)
	if err := utils.Copy(src, dst); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

func osStat(ls LkState) int {
	path := ls.CheckString(1)
	info, err := os.Stat(path)
	if err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	stat := lkMap{
		"size":   info.Size(),
		"mode":   info.Mode().String(),
		"time":   info.ModTime().UnixMilli(),
		"name":   info.Name(),
		"is_dir": info.IsDir(),
	}
	pushTable(ls, stat)
	ls.PushNil()
	return 2
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
		t := time.Now().UnixMilli() /* get current time */
		ls.PushInteger(t)
	} else {
		ls.CheckType(1, LK_TTABLE)
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
			hour, min, sec, 0, loc).UnixMilli()
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

func osSetEnv(ls LkState) int {
	key := ls.CheckString(1)
	value := ls.CheckString(2)
	if err := os.Setenv(key, value); err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}

// os.exec (script)
func osExecute(ls LkState) int {
	script := ls.CheckString(1)
	tempDir := os.TempDir()
	path := path.Join(tempDir, "lkscript"+utils.Md5([]byte(script)))
	err := ioutil.WriteFile(path, []byte(script), 0744)
	if err != nil {
		ls.PushNil()
		ls.PushString(err.Error())
		return 2
	}
	cmd := exec.Command("bash", path)
	cmdOut := new(bytes.Buffer)
	cmdErr := new(bytes.Buffer)
	cmd.Stdout = cmdOut
	cmd.Stderr = cmdErr
	err = cmd.Run()
	if err != nil {
		ls.PushNil()
		ls.PushString(strings.Trim(cmdErr.String(), "\n"))
	} else {
		ls.PushString(strings.Trim(cmdOut.String(), "\n"))
		ls.PushNil()
	}
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
