package state

import (
	"io/ioutil"
	"strings"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/binchunk"
	"git.lolli.tech/lollipopkit/lk/compiler"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
)

func Compile(source string) *binchunk.Prototype {
	if strings.HasPrefix(source, "@") {
		source = source[1:]
	}

	if !utils.Exist(source) {
		term.Error("[compile] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[compile] can't read file: " + err.Error())
	}

	bin := compiler.Compile(string(data), source)

	compiledData, err := bin.Dump(utils.Md5(data))
	if err != nil {
		term.Error("[compile] dump file failed: " + err.Error())
	}
	err = ioutil.WriteFile(source+"c", compiledData, 0744)
	if err != nil {
		term.Error("[compile] write file failed: " + err.Error())
	}
	return bin
}

func loadlk(source string) *binchunk.Prototype {
	lkc := source + "c"
	if utils.Exist(lkc) {
		return loadlkc(lkc)
	}
	return Compile(source)
}

func loadlkc(source string) *binchunk.Prototype {
	if !utils.Exist(source) {
		term.Error("[run] file not found: " + source)
	}

	data, err := ioutil.ReadFile(source)
	if err != nil {
		term.Error("[run] can't read file: " + err.Error())
	}

	lkPath := source[:len(source)-1]
	var lkData []byte
	lkExist := utils.Exist(lkPath)
	if lkExist {
		lkData, err = ioutil.ReadFile(lkPath)
		if err != nil {
			term.Error("[run] can't read file: " + err.Error())
		}
	}

	proto, err := binchunk.Verify(data, lkData)
	if err != nil {
		if err == binchunk.ErrMismatchedHash {
			if lkExist {
				term.Info("[run] source changed, recompiling " + lkPath)
				proto = Compile(lkPath)
			} else {
				term.Warn("[run] source not found: " + lkPath)
			}
		} else if strings.HasPrefix(err.Error(), binchunk.MismatchVersionPrefix) {
			if lkExist {
				term.Info("[run] mismatch version, recompiling " + lkPath)
				proto = Compile(lkPath)
			} else {
				term.Error("[run] mismatch version and source not found: " + lkPath)
			}
		} else {
			term.Error("[run] chunk verify failed: " + err.Error())
		}
	}

	return proto
}

func load(file string) *binchunk.Prototype {
	if strings.HasPrefix(file, "@") {
		file = file[1:]
	}
	if strings.HasSuffix(file, ".lk") {
		return loadlk(file)
	} else if strings.HasSuffix(file, ".lkc") {
		return loadlkc(file)
	}
	term.Error("[run] unknown file type: " + file)
	return nil
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_load
func (self *lkState) Load(chunk []byte, chunkName, mode string) int {
	var proto *binchunk.Prototype
	if chunkName == "stdin" {
		proto = compiler.Compile(string(chunk), chunkName)
	} else {
		proto = load(chunkName)
	}

	c := newLuaClosure(proto)
	self.stack.push(c)
	if len(proto.Upvalues) > 0 {
		env := self.registry.get(LUA_RIDX_GLOBALS)
		c.upvals[0] = &env
	}
	return LUA_OK
}
