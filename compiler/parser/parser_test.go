package parser

import (
	. "github.com/lollipopkit/lk/compiler/ast"
	"github.com/lollipopkit/lk/compiler/lexer"
	"testing"
)

func TestParseListMap(t *testing.T) {
	l := lexer.NewLexer("[1,2]", "")
	exp := ParseExp(l)
	list, ok := exp.(*ListConstructorExp)
	if !ok || len(list.ValExps) != 2 {
		t.Fatalf("expect list with 2 values")
	}

	l = lexer.NewLexer("{'a':1}", "")
	exp = ParseExp(l)
	m, ok := exp.(*MapConstructorExp)
	if !ok || len(m.KeyExps) != 1 {
		t.Fatalf("expect map with 1 field")
	}
}
