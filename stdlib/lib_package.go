package stdlib

import (
	"os"
	"strings"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/consts"
	"github.com/lollipopkit/lk/mods"
)

/* key, in the registry, for table of loaded modules */
const LUA_LOADED_TABLE = "_LOADED"

/* key, in the registry, for table of preloaded loaders */
const LUA_PRELOAD_TABLE = "_PRELOAD"

const (
	LUA_DIRSEP    = string(os.PathSeparator)
	LUA_PATH_SEP  = ";"
	LUA_PATH_MARK = "?"
	LUA_EXEC_DIR  = "!"
	LUA_IGMARK    = "-"
)

var pkgFuncs = map[string]GoFunction{
	"search": pkgSearchPath,
	/* placeholders */
	"preload":   nil,
	"cpath":     nil,
	"path":      nil,
	"searchers": nil,
	"loaded":    nil,
}

var llFuncs = map[string]GoFunction{
	"import": pkgImport,
}

func OpenPackageLib(ls LkState) int {
	ls.NewLib(pkgFuncs) /* create 'package' table */
	createSearchersTable(ls)
	/* set paths */
	ls.PushString("?.lk;?.lkc;?/init.lk")
	ls.SetField(-2, "path")
	/* store config information */
	ls.PushString(LUA_DIRSEP + "\n" + LUA_PATH_SEP + "\n" +
		LUA_PATH_MARK + "\n" + LUA_EXEC_DIR + "\n" + LUA_IGMARK + "\n")
	ls.SetField(-2, "config")
	/* set field 'loaded' */
	ls.GetSubTable(LK_REGISTRYINDEX, LUA_LOADED_TABLE)
	ls.SetField(-2, "loaded")
	/* set field 'preload' */
	ls.GetSubTable(LK_REGISTRYINDEX, LUA_PRELOAD_TABLE)
	ls.SetField(-2, "preload")
	ls.PushGlobalTable()
	ls.PushValue(-2)        /* set 'package' as upvalue for next lib */
	ls.SetFuncs(llFuncs, 1) /* open lib into global table */
	ls.Pop(1)               /* pop global table */
	return 1                /* return 'package' table */
}

func createSearchersTable(ls LkState) {
	searchers := []GoFunction{
		preloadSearcher,
		lkSearcher,
	}
	/* create 'searchers' table */
	ls.CreateTable(len(searchers), 0)
	/* fill it with predefined searchers */
	for idx := range searchers {
		ls.PushValue(-2) /* set 'package' as upvalue for all searchers */
		ls.PushGoClosure(searchers[idx], 1)
		ls.RawSetI(-2, int64(idx+1))
	}
	ls.SetField(-2, "searchers") /* put it in field 'searchers' */
}

func preloadSearcher(ls LkState) int {
	name := ls.CheckString(1)
	ls.GetField(LK_REGISTRYINDEX, "_PRELOAD")
	if ls.GetField(-1, name) == LK_TNIL { /* not found? */
		ls.PushString("\n\tno field pkg.preload['" + name + "']")
	}
	return 1
}

func lkSearcher(ls LkState) int {
	name := ls.CheckString(1)
	ls.GetField(LkUpvalueIndex(1), "path")
	path, ok := ls.ToStringX(-1)
	if !ok {
		ls.Error2("'pkg.path' must be a string")
	}

	c, filename, errMsg := _searchPath(name, path, ".", LUA_DIRSEP)
	if errMsg != "" {
		ls.PushString(errMsg)
		return 1
	}

	if ls.Load(c, filename, "bt") == LK_OK { /* module loaded successfully? */
		ls.PushString(filename) /* will be 2nd argument to module */
		return 2                /* return open function and file name */
	} else {
		return ls.Error2("error loading module '%s' from file '%s':\n\t%s",
			ls.CheckString(1), filename, ls.CheckString(-1))
	}
}

// package.searchpath (name, path [, sep [, rep]])
// http://www.lua.org/manual/5.3/manual.html#pdf-package.searchpath
// loadlib.c#ll_searchpath
func pkgSearchPath(ls LkState) int {
	name := ls.CheckString(1)
	path := ls.CheckString(2)
	sep := ls.OptString(3, ".")
	rep := ls.OptString(4, LUA_DIRSEP)
	if _, filename, errMsg := _searchPath(name, path, sep, rep); errMsg == "" {
		ls.PushString(filename)
		return 1
	} else {
		ls.PushNil()
		ls.PushString(errMsg)
		return 2
	}
}

func _searchPath(name, path, sep, dirSep string) (content []byte, fname, errMsg string) {
	if sep != "" {
		name = strings.Replace(name, sep, dirSep, -1)
	}

	for _, filename := range strings.Split(path, LUA_PATH_SEP) {
		// 优先在磁盘内搜索
		filename = strings.Replace(filename, LUA_PATH_MARK, name, -1)
		if _, err := os.Stat(filename); !os.IsNotExist(err) {
			c, err := os.ReadFile(filename)
			if err != nil {
				return nil, filename, err.Error()
			}
			return c, filename, ""
		}

		// 如果磁盘内无 builtin 模块，再在内置 mods 内搜索
		// 意味着可以覆盖 builtin 的实现
		if c, err := mods.Files.ReadFile(filename); !os.IsNotExist(err) {
			return c, consts.BuiltinPrefix + filename, ""
		}

		errMsg += "\n\tno file '" + filename + "'"
	}

	return nil, "", errMsg
}

func pkgImport(ls LkState) int {
	name := ls.CheckString(1)
	ls.SetTop(1) /* LOADED table will be at index 2 */
	ls.GetField(LK_REGISTRYINDEX, LUA_LOADED_TABLE)
	ls.GetField(2, name)  /* LOADED[name] */
	if ls.ToBoolean(-1) { /* is it there? */
		return 1 /* package is already loaded */
	}
	/* else must load package */
	ls.Pop(1) /* remove 'getfield' result */
	_findLoader(ls, name)
	ls.PushString(name) /* pass name as argument to module loader */
	ls.Insert(-2)       /* name is 1st argument (before search data) */
	ls.Call(2, 1)       /* run loader to load module */
	if !ls.IsNil(-1) {  /* non-nil return? */
		ls.SetField(2, name) /* LOADED[name] = returned value */
	}
	if ls.GetField(2, name) == LK_TNIL { /* module set no value? */
		ls.PushBoolean(true) /* use true as result */
		ls.PushValue(-1)     /* extra copy to be returned */
		ls.SetField(2, name) /* LOADED[name] = true */
	}
	return 1
}

func _findLoader(ls LkState, name string) {
	/* push 'package.searchers' to index 3 in the stack */
	if ls.GetField(LkUpvalueIndex(1), "searchers") != LK_TMAP {
		ls.Error2("'package.searchers' must be a table")
	}

	/* to build error message */
	errMsg := "module '" + name + "' not found:"

	/*  iterate over available searchers to find a loader */
	for i := int64(1); ; i++ {
		if ls.RawGetI(3, i) == LK_TNIL { /* no more searchers? */
			ls.Pop(1)         /* remove nil */
			ls.Error2(errMsg) /* create error message */
		}

		ls.PushString(name)
		ls.Call(1, 2)          /* call it */
		if ls.IsFunction(-2) { /* did it find a loader? */
			return /* module loader found */
		} else if ls.IsString(-2) { /* searcher returned error message? */
			ls.Pop(1)                    /* remove extra return */
			errMsg += ls.CheckString(-1) /* concatenate error message */
		} else {
			ls.Pop(2) /* remove both returns */
		}
	}
}
