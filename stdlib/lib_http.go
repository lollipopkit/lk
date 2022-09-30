package stdlib

import (
	"io"
	"io/ioutil"
	"net/http"
	"strings"

	. "git.lolli.tech/lollipopkit/lk/api"
	"git.lolli.tech/lollipopkit/lk/consts"
	jsoniter "github.com/json-iterator/go"
)

type luaMap map[string]any

var (
	client  = http.Client{}
	json    = jsoniter.ConfigCompatibleWithStandardLibrary
	httpLib = map[string]GoFunction{
		"req":    httpReq,
		"get":    httpGet,
		"post":   httpPost,
		"listen": httpListen,
	}
)

func OpenHttpLib(ls LkState) int {
	ls.NewLib(httpLib)
	return 1
}

func httpDo(method, url string, headers luaMap, body io.Reader) (int, string, error) {
	request, err := http.NewRequest(method, url, body)
	if err != nil {
		return 0, "", err
	}

	request.Header.Set("user-agent", "lk-http/"+consts.VERSION)
	// 仅遍历下标，性能更佳
	for k := range headers {
		request.Header.Set(k, headers[k].(string))
	}

	resp, err := client.Do(request)
	if err != nil {
		return 0, "", err
	}
	respBody, err := ioutil.ReadAll(resp.Body)
	if err != nil {
		return 0, "", err
	}
	defer resp.Body.Close()
	return resp.StatusCode, string(respBody), nil
}

func httpGet(ls LkState) int {
	url := ls.CheckString(1)
	headers := OptTable(ls, 2, luaMap{})
	code, data, err := httpDo("GET", url, headers, nil)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}
	ls.PushInteger(int64(code))
	ls.PushString(data)
	return 2
}

func httpPost(ls LkState) int {
	url := ls.CheckString(1)
	headers := OptTable(ls, 2, luaMap{})
	bodyStr := ls.OptString(3, "")

	body := func() io.Reader {
		if bodyStr != "" {
			return strings.NewReader(bodyStr)
		}
		return nil
	}()

	code, data, err := httpDo("POST", url, headers, body)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}
	ls.PushInteger(int64(code))
	ls.PushString(data)
	return 2
}

// http.req (method, url [, headers, body])
// return code, data
func httpReq(ls LkState) int {
	method := strings.ToUpper(ls.CheckString(1))
	url := ls.CheckString(2)
	headers := OptTable(ls, 3, luaMap{})
	bodyStr := ls.OptString(4, "")

	body := func() io.Reader {
		if bodyStr != "" {
			return strings.NewReader(bodyStr)
		}
		return nil
	}()

	code, data, err := httpDo(method, url, headers, body)
	if err != nil {
		ls.PushInteger(0)
		ls.PushString(err.Error())
		return 2
	}

	ls.PushInteger(int64(code))
	ls.PushString(data)
	return 2
}

func genReqTable(r *http.Request) (luaMap, error) {
	body, err := ioutil.ReadAll(r.Body)
	if err != nil {
		return nil, err
	}
	headers := luaMap{}
	for k := range r.Header {
		headers[k] = r.Header[k]
	}
	return luaMap{
		"method":  r.Method,
		"url":     r.URL.String(),
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
