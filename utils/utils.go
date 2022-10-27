package utils

import (
	"crypto/md5"
	"fmt"
	"os"
)

func Md5(data []byte) string {
	return fmt.Sprintf("%x", md5.Sum(data))
}

func Exist(path string) bool {
	_, err := os.Stat(path)
	return !os.IsNotExist(err)
}
