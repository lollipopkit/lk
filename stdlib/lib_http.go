package stdlib

import (
	"io"
	"io/ioutil"
	"net/http"
	"strconv"
	"strings"

	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
	"git.lolli.tech/lollipopkit/go-lang-lk/binchunk"
)

var (
	client  = http.Client{}
	httpLib = map[string]GoFunction{
		"req": httpReq,
	}
)

func OpenHttpLib(ls LkState) int {
	ls.NewLib(httpLib)
	return 1
}

// http.req (method, url [, headers, body])
// return code, data
func httpReq(ls LkState) int {
	method := ls.CheckString(1)
	url := ls.CheckString(2)
	headers := OptTable(ls, 3, map[string]any{
		"User-Agent": "lk/"+strconv.FormatFloat(binchunk.VERSION, 'f', 1, 64),
	})
	bodyStr := ls.OptString(4, "")

	body := func() io.Reader {
		if bodyStr != "" {
			return strings.NewReader(bodyStr)
		}
		return nil
	}()

	request, err := http.NewRequest(strings.ToUpper(method), url, body)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}

	for k, v := range headers {
		request.Header.Set(k, v.(string))
	}

	resp, err := client.Do(request)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}

	defer resp.Body.Close()

	data, err := ioutil.ReadAll(resp.Body)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}

	ls.PushInteger(int64(resp.StatusCode))
	ls.PushString(string(data))
	return 2
}
