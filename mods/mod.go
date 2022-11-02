package mods

import (
	"embed"
	"io/ioutil"
	"os"
	"path"
	"sync"

	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
	"github.com/tidwall/gjson"
)

var (
	//go:embed index.json files
	ModFiles embed.FS

	indexFilePath    = path.Join(consts.LkPath, "index.json")
	builtInIndexPath = "index.json"
	builtInFilesPath = "files"
)

func init() {
	if consts.LkPath == "" {
		term.Yellow("env LK_PATH not set. \nCan't use built-in modules.")
	}
}

func InitMods(wg *sync.WaitGroup) {
	wg.Add(1)
	defer wg.Done()

	if consts.LkPath == "" {
		return
	}
	if utils.Exist(indexFilePath) {
		indexBytes, err := ioutil.ReadFile(indexFilePath)
		if err != nil {
			term.Red("[mod] can't read index.json: " + err.Error())
		}
		index := gjson.ParseBytes(indexBytes).Map()
		sameVM := index["vm"].String() == consts.VERSION
		version := index["version"].Int()
		bulitInIndexBytes, err := ModFiles.ReadFile(builtInIndexPath)
		if err != nil {
			term.Red("[mod] can't read built-in index.json: " + err.Error())
		}
		builtInIndex := gjson.ParseBytes(bulitInIndexBytes).Map()
		builtInVersion := builtInIndex["version"].Int()
		if version >= builtInVersion && sameVM {
			return
		}
	}
	extract()
}

func extract() {
	index, err := ModFiles.ReadFile(builtInIndexPath)
	if err != nil {
		term.Red("[mod] can't read index.json: " + err.Error())
	}
	err = os.WriteFile(indexFilePath, index, 0644)
	if err != nil {
		term.Red("[mod] can't write index.json: " + err.Error())
	}
	files, err := ModFiles.ReadDir(builtInFilesPath)
	if err != nil {
		term.Red("[mod] can't read files: " + err.Error())
	}

	for idx := range files {
		if files[idx].IsDir() {
			continue
		}
		data, err := ModFiles.ReadFile(path.Join(builtInFilesPath, files[idx].Name()))
		if err != nil {
			term.Red("[mod] can't read file: " + err.Error())
		}
		err = os.WriteFile(path.Join(consts.LkPath, files[idx].Name()), data, 0644)
		if err != nil {
			term.Red("[mod] can't write file: " + err.Error())
		}
	}
}
