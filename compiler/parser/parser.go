package parser

import (
	. "github.com/lollipopkit/lk/compiler/ast"
	. "github.com/lollipopkit/lk/compiler/lexer"
)

/* recursive descent parser */

func Parse(chunk, chunkName string) *Block {
	lexer := NewLexer(chunk, chunkName)
	block := ParseBlock(lexer)

	lexer.NextTokenOfKind(TOKEN_EOF)
	return block
}
