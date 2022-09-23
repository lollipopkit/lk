package stdlib

import (
	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
	glc "git.lolli.tech/lollipopkit/go_lru_cacher"
	"github.com/tidwall/gjson"
)

var (
	jsonLib = map[string]GoFunction{
		"get": jsonGet,
	}
	// 缓存gjson加载结果
	gjsonCacher = glc.NewCacher(10)
)

func OpenJsonLib(ls LkState) int {
	ls.NewLib(jsonLib)
	return 1
}

// json.get (source, path)
// return bool, result
func jsonGet(ls LkState) int {
	source := ls.CheckString(1)
	path := ls.CheckString(2)

	// 从缓存中获取gjson.Result
	var gjsonResult gjson.Result
	gjsonCache, ok := gjsonCacher.Get(source)
	if !ok {
		gjsonResult = gjson.Parse(source)
		gjsonCacher.Set(source, gjsonResult)
	} else {
		gjsonResult, ok = gjsonCache.(gjson.Result)
		if !ok {
			ls.PushString("gjson cache type convert error")
			return 1
		}
	}

	// 从gjson.Result中获取结果
	result := gjsonResult.Get(path)
	if !result.Exists() {
		ls.PushBoolean(false)
		ls.PushString("")
		return 2
	}
	ls.PushBoolean(true)
	ls.PushString(result.String())
	return 2
}
