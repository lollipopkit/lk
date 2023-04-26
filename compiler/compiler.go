package compiler

import (
	"github.com/lollipopkit/lk/binchunk"
	"github.com/lollipopkit/lk/compiler/codegen"
	"github.com/lollipopkit/lk/compiler/parser"
)

func Compile(chunk, chunkName string) *binchunk.Prototype {
	ast := parser.Parse(chunk, chunkName)
	proto := codegen.GenProto(ast)
	setSource(proto, chunkName)
	return proto
}

func setSource(proto *binchunk.Prototype, chunkName string) {
	proto.Source = chunkName
	for k := range proto.Protos {
		setSource(proto.Protos[k], chunkName)
	}
}
