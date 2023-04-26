package stdlib

import (
	"regexp"

	glc "github.com/lollipopkit/go-lru-cacher"
	. "github.com/lollipopkit/lk/api"
)

var (
	reCacher = glc.NewCacher[*regexp.Regexp](10)
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
	cache, ok := reCacher.Get(pattern)
	if ok {
		return *cache
	}
	exp := regexp.MustCompile(pattern)
	reCacher.Set(pattern, &exp)
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
	pushList(ls, matches)
	return 1
}
