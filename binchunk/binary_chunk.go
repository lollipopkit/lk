package binchunk

import (
	"errors"
	"strings"

	"git.lolli.tech/lollipopkit/lk/consts"
	"git.lolli.tech/lollipopkit/lk/utils"
	jsoniter "github.com/json-iterator/go"
)

const (
	MismatchVersionPrefix = "mismatch LK VM version: "
)

var (
	json                    = jsoniter.ConfigCompatibleWithStandardLibrary
	ErrInvalidVersionFormat = errors.New("invalid version format")
	ErrMismatchedHash       = errors.New("mismatched hash")
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
	Version string     `json:"v"`
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

func Verify(data, sourceData []byte) (*Prototype, error) {
	var bin binaryChunk
	err := json.Unmarshal(data, &bin)
	if err != nil {
		return nil, err
	}
	if bin.Sign != consts.SIGNATURE {
		return nil, errors.New("invalid signature: " + bin.Sign)
	}
	if sourceData != nil && bin.Md5 != utils.Md5(sourceData) {
		return nil, ErrMismatchedHash
	}

	return bin.Proto, passVersion(bin.Version)
}

func passVersion(v string) error {
	vs := strings.Split(v, ".")
	if len(vs) != 3 {
		return ErrInvalidVersionFormat
	}

	if strings.Compare(v, consts.VERSION) >= 0 {
		return nil
	}
	return errors.New(MismatchVersionPrefix + consts.VERSION + " is required, but " + v + " is provided")
}

func (proto *Prototype) Dump(md5 string) ([]byte, error) {
	bin := &binaryChunk{
		Version: consts.VERSION,
		Sign:    consts.SIGNATURE,
		Proto:   proto,
		Md5:     md5,
	}
	return json.Marshal(bin)
}
