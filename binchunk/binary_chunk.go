package binchunk

import (
	"git.lolli.tech/lollipopkit/go-lang-lk/consts"
	jsoniter "github.com/json-iterator/go"
)

var (
	json = jsoniter.ConfigCompatibleWithStandardLibrary
)

const (
	TAG_NIL       = 0x00
	TAG_BOOLEAN   = 0x01
	TAG_NUMBER    = 0x03
	TAG_INTEGER   = 0x13
	TAG_SHORT_STR = 0x04
	TAG_LONG_STR  = 0x14
)

type binaryChunk struct {
	Version string		`json:"v"`
	Sign string		`json:"si"`
	Hash string			`json:"h"`
	Proto *Prototype	`json:"p"`
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

func IsJsonChunk(data []byte) (bool, *Prototype) {
	var bin binaryChunk
	err := json.Unmarshal(data, &bin)
	if err != nil {
		return false, nil
	}
	if bin.Sign != consts.SIGNATURE || bin.Version != consts.VERSION {
		return false, nil
	}
	return err == nil, bin.Proto
}

func (proto *Prototype) Dump() ([]byte, error) {
	bin := &binaryChunk{
		Version: consts.VERSION,
		Sign: consts.SIGNATURE,
		Proto: proto,
	}
	return json.Marshal(bin)
}
