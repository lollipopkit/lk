package stdlib

import (
	"regexp"

	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
	glc "git.lolli.tech/lollipopkit/go_lru_cacher"
)

var (
	reCacher = glc.NewCacher(10)
	reLib = map[string]GoFunction{
		"have": reFound,
		"find":  reFind,
	}
)

func OpenReLib(ls LkState) int {
	ls.NewLib(reLib)
	return 1
}

func getExp(pattern string) *regexp.Regexp {
	var exp *regexp.Regexp
	cache, ok := reCacher.Get(pattern)
	if ok {
		exp, ok = cache.(*regexp.Regexp)
		if ok {
			goto END
		}
		
	}
	exp = regexp.MustCompile(pattern)
	reCacher.Set(pattern, exp)
	END:
	return exp
}

func reFound(ls LkState) int {
	pattern := ls.CheckString(1)
	text := ls.CheckString(2)
	ls.PushBoolean(getExp(pattern).MatchString(text))
	return 1
}

func reFind(ls LkState) int {
	pattern := ls.CheckString(1)
	text := ls.CheckString(2)
	matches := getExp(pattern).FindStringSubmatch(text)
	ms := make([]any, len(matches))
	for idx := 0; idx < len(matches); idx++ {
		ms[idx] = matches[idx]
	}
	pushList(ls, ms)
	return 1
}