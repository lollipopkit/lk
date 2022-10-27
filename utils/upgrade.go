package utils

import (
	"errors"
	"io/ioutil"
	"net/http"
	"os"
	"path"
	"strings"
	"sync"
	"time"

	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/term"
	"github.com/tidwall/gjson"
)

const (
	checkFailedPrefix = "Check upgrade failed:\n"
)

var (
	ErrInvalidVersion = errors.New("invalid version")

	remoteVersionFilePath = path.Join(consts.LkPath, "remote_version")

	client = http.Client{
		Timeout: time.Duration(500 * time.Millisecond),
	}
)

func CheckUpgrade(wg *sync.WaitGroup) {
	wg.Add(1)
	defer wg.Done()

	if consts.LkPath == "" {
		return
	}

	stat, err := os.Stat(remoteVersionFilePath)
	if err == nil && stat.ModTime().After(time.Now().Add(-time.Hour*96)) {
		remoteVersionBytes, err := ioutil.ReadFile(remoteVersionFilePath)
		if err != nil {
			return
		}
		IsVersionNewer(string(remoteVersionBytes))
		return
	}

	resp, err := client.Get(consts.ReleaseApiUrl)
	if err != nil {
		return
	}
	defer resp.Body.Close()

	if resp.StatusCode != 200 {
		return
	}

	body, err := ioutil.ReadAll(resp.Body)
	if err != nil {
		term.Warn(checkFailedPrefix + err.Error())
		return
	}

	newest := gjson.ParseBytes(body).Map()["tag_name"].String()
	if newest == "" {
		term.Warn(checkFailedPrefix + "can't get newest version")
		return
	}

	IsVersionNewer(newest[1:])
	err = ioutil.WriteFile(remoteVersionFilePath, []byte(newest[1:]), 0644)
	if err != nil {
		term.Warn(checkFailedPrefix + err.Error())
	}
}

func IsVersionNewer(get string) {
	if strings.Count(get, ".") != 2 {
		term.Warn(checkFailedPrefix + ErrInvalidVersion.Error())
	}

	nowArr := strings.Split(consts.VERSION, ".")
	getArr := strings.Split(get, ".")
	for i := 0; i < 3; i++ {
		if nowArr[i] == getArr[i] {
			continue
		}
		if nowArr[i] > getArr[i] {
			return
		}
		term.Info("New version available: " + get)
	}
	return
}
