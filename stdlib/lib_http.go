package stdlib

import (
	"io"
	"io/ioutil"
	"net/http"
	"strings"

	. "git.lolli.tech/lollipopkit/go-lang-lk/api"
	"git.lolli.tech/lollipopkit/go-lang-lk/consts"
	jsoniter "github.com/json-iterator/go"
)

var (
	client  = http.Client{}
	json    = jsoniter.ConfigCompatibleWithStandardLibrary
	httpLib = map[string]GoFunction{
		"req":    httpReq,
		"listen": httpListen,
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
	headers := OptTable(ls, 3, map[string]any{})
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

	request.Header.Set("user-agent", "lk-http/" + string(consts.VERSION))
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

func genReqTable(r *http.Request) (map[string]any, error) {
	body, err := ioutil.ReadAll(r.Body)
	if err != nil {
		return nil, err
	}
	headers := map[string]any{}
	for k, v := range r.Header {
		headers[k] = v
	}
	return map[string]any{
		"method":  r.Method,
		"url":     r.URL.String(),
		"proto":   r.Proto,
		"headers": headers,
		"body":    string(body),
	}, nil
}

// Lua eg:
// http.listen(port, fn(req) {rt code, data})
// return err
func httpListen(ls LkState) int {
	addr := ls.CheckString(1)
	ls.CheckType(2, LUA_TFUNCTION)
	err := http.ListenAndServe(addr, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		req, err := genReqTable(r)
		if err != nil {
			w.WriteHeader(http.StatusInternalServerError)
			w.Write([]byte(err.Error()))
			return
		}
		ls.PushValue(-1)
		pushTable(ls, req)
		ls.Call(1, 2)
		code := ls.ToInteger(-2)
		data := ls.ToString(-1)
		w.WriteHeader(int(code))
		w.Write([]byte(data))
		ls.Pop(2)
	}))
	if err != nil {
		ls.PushString(err.Error())
		return 1
	}
	ls.PushNil()
	return 1
}
