package binchunk

import (
	"bytes"
	"math"

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

	VERSION   = 0.1
	SIGNATURE = `LANG_LK`
)

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
	if len(data) < 9 {
		return false, nil
	}
	if !bytes.HasPrefix(data, []byte{'\x1b'}) {
		return false, nil
	}
	if data[1] != byte(math.Float64bits(VERSION)) {
		panic("version not match!")
	}
	data = data[9:]
	var proto Prototype
	err := json.Unmarshal(data, &proto)
	return err == nil, &proto
}

func (proto *Prototype) Dump() ([]byte, error) {
	data, err := json.Marshal(proto)
	if err != nil {
		return nil, err
	}

	v := math.Float64bits(VERSION)
	by := []byte{'\x1b'}
	by = append(by, byte(v))
	by = append(by, bytes.NewBufferString(SIGNATURE).Bytes()...)
	data = append(by, data...)
	return data, err
}
