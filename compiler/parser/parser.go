package parser

import (
	. "git.lolli.tech/lollipopkit/lk/compiler/ast"
	. "git.lolli.tech/lollipopkit/lk/compiler/lexer"
)

/* recursive descent parser */

func Parse(chunk, chunkName string) *Block {
	lexer := NewLexer(chunk, chunkName)
	block := parseBlock(lexer)

	lexer.NextTokenOfKind(TOKEN_EOF)
	return block
}
