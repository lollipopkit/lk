package term

import (
	"errors"
	"os"
	"os/exec"
	"strconv"
	"strings"
)

type termSize struct {
	Height int
	Width  int
}

var (
	ErrTermSizeParseFailed = errors.New("term size parse failed")
)

func Size() (*termSize, error) {
	cmd := exec.Command("stty", "size")
	cmd.Stdin = os.Stdin
	out, err := cmd.Output()
	if err != nil {
		return nil, err
	}

	sizeStr := strings.Trim(string(out), "\n")
	sizeStrs := strings.Split(sizeStr, " ")
	if len(sizeStrs) != 2 {
		return nil, ErrTermSizeParseFailed
	}

	height, err := strconv.ParseInt(sizeStrs[0], 10, 32)
	if err != nil {
		return nil, err
	}

	width, err := strconv.ParseInt(sizeStrs[1], 10, 32)
	if err != nil {
		return nil, err
	}

	return &termSize{
		Height: int(height),
		Width:  int(width),
	}, nil
}
