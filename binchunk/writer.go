package binchunk

import (
	"bytes"
	"math"
)

func int64ToBytes(i int64) []byte {
	bytes := make([]byte, 8)
	for j := 0; j < 8; j++ {
		bytes[j] = byte(i & 0xFF)
		i >>= 8
	}
	return bytes
}

func float64ToBytes(f float64) []byte {
	return int64ToBytes(int64(math.Float64bits(f)))
}

func int32ToBytes(i int32) []byte {
	bytes := make([]byte, 4)
	for j := 0; j < 4; j++ {
		bytes[j] = byte(i & 0xFF)
		i >>= 8
	}
	return bytes
}

func writeHeader(writer *bytes.Buffer) {
	writer.WriteString(LUA_SIGNATURE)
	writer.WriteByte(LUAC_VERSION)
	writer.WriteByte(LUAC_FORMAT)
	writer.WriteString(LUAC_DATA)
	writer.WriteByte(CINT_SIZE)
	writer.WriteByte(CSIZET_SIZE)
	writer.WriteByte(INSTRUCTION_SIZE)
	writer.WriteByte(LUA_INTEGER_SIZE)
	writer.WriteByte(LUA_NUMBER_SIZE)
	writer.Write(int64ToBytes(LUAC_INT))
	writer.Write(float64ToBytes(LUAC_NUM))
	writer.WriteByte(0) // size_upvalues
}

func writeProto(writer *bytes.Buffer, proto *Prototype) {
	writer.WriteString(proto.Source)
	writer.WriteByte(byte(proto.LineDefined))
	writer.WriteByte(byte(proto.LastLineDefined))
	writer.WriteByte(byte(proto.NumParams))
	writer.WriteByte(byte(proto.IsVararg))
	writer.WriteByte(byte(proto.MaxStackSize))
	writeCode(writer, proto.Code)
	writeConstants(writer, proto.Constants)
	writeUpvalues(writer, proto.Upvalues)
	for _, p := range proto.Protos {
		writeProto(writer, p)
	}
}

func writeCode(writer *bytes.Buffer, code []uint32) {
	writer.Write(int32ToBytes(int32(len(code))))
	for _, c := range code {
		writer.Write(int32ToBytes(int32(c)))
	}
}

func writeConstants(writer *bytes.Buffer, constants []interface{}) {
	writer.Write(int32ToBytes(int32(len(constants))))
	for _, c := range constants {
		switch c.(type) {
		case nil:
			writer.WriteByte(TAG_NIL)
		case bool:
			writer.WriteByte(TAG_BOOLEAN)
			if c.(bool) {
				writer.WriteByte(1)
			} else {
				writer.WriteByte(0)
			}
		case int64:
			writer.WriteByte(TAG_INTEGER)
			writer.Write(int64ToBytes(c.(int64)))
		case float64:
			writer.WriteByte(TAG_NUMBER)
			writer.Write(float64ToBytes(c.(float64)))
		case string:
			writer.WriteByte(TAG_SHORT_STR)
			writer.Write(int32ToBytes(int32(len(c.(string)))))
			writer.WriteString(c.(string))
		}
	}
}

func writeUpvalues(writer *bytes.Buffer, upvalues []Upvalue) {
	writer.Write(int32ToBytes(int32(len(upvalues))))
	for _, u := range upvalues {
		writer.WriteByte(u.Instack)
		writer.WriteByte(u.Idx)
	}
}

func (proto *Prototype) Dump() []byte {
	var writer bytes.Buffer
	writeHeader(&writer)
	writeProto(&writer, proto)
	return writer.Bytes()
}
