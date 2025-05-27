package stdlib

import (
	"io"
	"net/http"
	"strings"

	. "github.com/lollipopkit/lk/api"
)

var (
	client  = http.Client{}
	httpLib = map[string]GoFunction{
		"req":    httpReq,
		"listen": httpListen,
	}
)

func OpenHttpLib(ls LkState) int {
	ls.NewLib(httpLib)
	return 1
}

func httpReq(ls LkState) int {
	method := strings.ToUpper(ls.CheckString(1))
	url := ls.CheckString(2)
	headers := make(map[string]string)
	ls.PushNil()
	for ls.Next(3) {
		key := ls.ToString(-2)
		val := ls.ToString(-1)
		headers[key] = val
		ls.Pop(1)
	}

	bodyStr := ls.ToString2(4)
	var body io.Reader
	if bodyStr != "" {
		body = strings.NewReader(bodyStr)
	}

	req, err := http.NewRequest(method, url, body)
	if err != nil {
		ls.PushNil()
		ls.Push(-1)
		ls.PushString(err.Error())
		return 3
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}

	resp, err := client.Do(req)
	if err != nil {
		ls.PushNil()
		ls.Push(-1)
		ls.PushString(err.Error())
		return 3
	}
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		ls.PushNil()
		ls.Push(resp.StatusCode)
		ls.PushString(err.Error())
		return 3
	}

	ls.PushString(string(data))
	ls.Push(resp.StatusCode)
	ls.PushNil()
	return 3
}

// eg:
// http.listen(addr, fn(req) {rt code, data})
// return err
func httpListen(ls LkState) int {
	addr := ls.CheckString(1)
	ls.CheckType(2, LK_TFUNCTION)
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

func genHeaderMap(h *http.Header) lkMap {
	m := lkMap{}
	for k := range *h {
		v := strings.Join((*h)[k], ";")
		m[k] = v
	}
	return m
}

func genReqTable(r *http.Request) (lkMap, error) {
	body, err := io.ReadAll(r.Body)
	if err != nil {
		return nil, err
	}
	headers := genHeaderMap(&r.Header)
	return lkMap{
		"method":  r.Method,
		"url":     r.URL.String(),
		"headers": headers,
		"body":    string(body),
	}, nil
}
