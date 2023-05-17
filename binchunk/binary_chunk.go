package binchunk

import (
	"errors"

	"github.com/lollipopkit/lk/consts"
	. "github.com/lollipopkit/lk/json"
)

type binaryChunk struct {
	Sign    string     `json:"si"`
	Md5     string     `json:"m"`
	Proto   *Prototype `json:"p"`
}

// function prototype
type Prototype struct {
	Source          string        `json:"s"` // debug
	LineDefined     uint32        `json:"ld"`
	LastLineDefined uint32        `json:"lld"`
	NumParams       byte          `json:"np"`
	IsVararg        byte          `json:"iv"`
	MaxStackSize    byte          `json:"ms"`
	Code            []uint32      `json:"c"`
	Constants       []interface{} `json:"cs"`
	Upvalues        []Upvalue     `json:"us"`
	Protos          []*Prototype  `json:"ps"`
	LineInfo        []uint32      `json:"li"`  // debug
	LocVars         []LocVar      `json:"lvs"` // debug
	UpvalueNames    []string      `json:"uns"` // debug
}

type Upvalue struct {
	Instack byte `json:"is"`
	Idx     byte `json:"idx"`
}

type LocVar struct {
	VarName string `json:"vn"`
	StartPC uint32 `json:"spc"`
	EndPC   uint32 `json:"epc"`
}

func Load(data []byte) (*Prototype, error) {
	var bin binaryChunk
	err := Json.Unmarshal(data, &bin)
	if err != nil {
		return nil, err
	}
	if bin.Sign != consts.SIGNATURE {
		return nil, errors.New("invalid signature: " + bin.Sign)
	}

	return bin.Proto, nil
}

func (proto *Prototype) Dump(md5 string) ([]byte, error) {
	bin := &binaryChunk{
		Sign:    consts.SIGNATURE,
		Proto:   proto,
		Md5:     md5,
	}
	return Json.Marshal(bin)
}
