package lexer

import (
	"reflect"
	"testing"
)

func TestListAndMapTokens(t *testing.T) {
	l := NewLexer("[1, 2]", "")
	var kinds []int
	for {
		_, k, _ := l.NextToken()
		kinds = append(kinds, k)
		if k == TOKEN_EOF {
			break
		}
	}
	expect := []int{TOKEN_SEP_LBRACK, TOKEN_NUMBER, TOKEN_SEP_COMMA, TOKEN_NUMBER, TOKEN_SEP_RBRACK, TOKEN_EOF}
	if !reflect.DeepEqual(kinds, expect) {
		t.Fatalf("list tokens %v", kinds)
	}

	l = NewLexer("{'a':1}", "")
	kinds = []int{}
	for {
		_, k, _ := l.NextToken()
		kinds = append(kinds, k)
		if k == TOKEN_EOF {
			break
		}
	}
	expect = []int{TOKEN_SEP_LCURLY, TOKEN_STRING, TOKEN_SEP_COLON, TOKEN_NUMBER, TOKEN_SEP_RCURLY, TOKEN_EOF}
	if !reflect.DeepEqual(kinds, expect) {
		t.Fatalf("map tokens %v", kinds)
	}
}
