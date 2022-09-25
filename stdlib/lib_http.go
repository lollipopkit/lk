package stdlib

import (
	"fmt"
	"io"
	"io/ioutil"
	"net/http"
	"strings"

	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
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
	headers := ls.OptString(3, "user-agent: lang-lk")
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

	for _, header := range strings.Split(headers, "\n") {
		if header == "" {
			continue
		}
		kv := strings.Split(header, ":")
		if len(kv) != 2 {
			ls.PushInteger(0)
			ls.PushString(fmt.Sprintf("invalid header: %s", header))
			return 2
		}
		request.Header.Set(strings.TrimSpace(kv[0]), strings.TrimSpace(kv[1]))
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
