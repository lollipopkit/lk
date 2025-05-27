package state

import (
	"os"
	"strings"

	. "github.com/lollipopkit/lk/api"
	"github.com/lollipopkit/lk/binchunk"
	"github.com/lollipopkit/lk/compiler"
	"github.com/lollipopkit/lk/term"
	"github.com/lollipopkit/lk/utils"
)

func Compile(source string) *binchunk.Prototype {
	if !utils.Exist(source) {
		term.Red("[compile] file not found: " + source)
		os.Exit(2)
	}

	data, err := os.ReadFile(source)
	if err != nil {
		term.Red("[compile] can't read file: " + err.Error())
		os.Exit(2)
	}

	bin := compiler.Compile(string(data), source)

	compiledData, err := bin.Dump(utils.Md5(data))
	if err != nil {
		term.Red("[compile] dump file failed: " + err.Error())
		os.Exit(2)
	}
	err = os.WriteFile(source+"c", compiledData, 0744)
	if err != nil {
		term.Red("[compile] write file failed: " + err.Error())
		os.Exit(2)
	}
	return bin
}

// [-0, +1, –]
// http://www.lua.org/manual/5.3/manual.html#lua_load
func (self *lkState) Load(chunk []byte, chunkName, mode string) LkStatus {
	var proto *binchunk.Prototype
	if chunkName == "stdin" || strings.HasSuffix(chunkName, ".lk") {
		proto = compiler.Compile(string(chunk), chunkName)
	} else {
		var err error
		proto, err = binchunk.Load(chunk)
		if err != nil {
			term.Red("[load] load chunk failed: " + err.Error())
			os.Exit(2)
		}
	}

	c := newLuaClosure(proto)
	self.stack.push(c)
	if len(proto.Upvalues) > 0 {
		env := self.registry.get(LK_RIDX_GLOBALS)
		c.upVals[0] = &env
	}
	return LK_OK
}
