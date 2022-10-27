package stdlib

import (
	"regexp"

	glc "git.lolli.tech/lollipopkit/go_lru_cacher"
	. "git.lolli.tech/lollipopkit/lk/api"
)

var (
	reCacher = glc.NewCacher(10)
	reLib    = map[string]GoFunction{
		"have": reFound,
		"find": reFind,
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
	if len(matches) == 0 {
		ls.PushNil()
		return 1
	}
	tb := make(map[string]string, len(matches))
	names := getExp(pattern).SubexpNames()
	for idx := range names {
		tb[names[idx]] = matches[idx]
	}
	pushTable(&ls, tb)
	return 1
}
