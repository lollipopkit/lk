package mods

import (
	"embed"
	"io/ioutil"
	"os"
	"path"

	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/term"
	"git.lolli.tech/lollipopkit/lk/utils"
	"github.com/tidwall/gjson"
)

var (
	//go:embed index.json files
	ModFiles embed.FS
	LkEnv    = os.Getenv("LK_PATH")

	indexFilePath    = path.Join(LkEnv, "index.json")
	builtInIndexPath = "index.json"
	builtInFilesPath = "files"
)

func init() {
	if LkEnv == "" {
		term.Warn("env LK_PATH not set. \nCan't use built-in modules.")
	}
}

func InitMods() {
	if LkEnv == "" {
		return
	}
	if utils.Exist(indexFilePath) {
		indexBytes, err := ioutil.ReadFile(indexFilePath)
		if err != nil {
			term.Error("can't read index.json: " + err.Error())
		}
		index := gjson.ParseBytes(indexBytes).Map()
		sameVM := index["vm"].String() == consts.VERSION
		version := index["version"].Int()
		bulitInIndexBytes, err := ModFiles.ReadFile(builtInIndexPath)
		if err != nil {
			term.Error("can't read built-in index.json: " + err.Error())
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
	term.Info("Extracting built-in modules...")
	index, err := ModFiles.ReadFile(builtInIndexPath)
	if err != nil {
		term.Error("can't read index.json: " + err.Error())
	}
	err = os.WriteFile(indexFilePath, index, 0644)
	if err != nil {
		term.Error("can't write index.json: " + err.Error())
	}
	files, err := ModFiles.ReadDir(builtInFilesPath)
	if err != nil {
		term.Error("can't read files: " + err.Error())
	}

	for idx := range files {
		if files[idx].IsDir() {
			continue
		}
		data, err := ModFiles.ReadFile(path.Join(builtInFilesPath, files[idx].Name()))
		if err != nil {
			term.Error("can't read file: " + err.Error())
		}
		err = os.WriteFile(path.Join(LkEnv, files[idx].Name()), data, 0644)
		if err != nil {
			term.Error("can't write file: " + err.Error())
		}
	}
}
