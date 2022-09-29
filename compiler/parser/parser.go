package parser

import (
	"regexp"

	. "git.lolli.tech/lollipopkit/go-lang-lk/compiler/ast"
	. "git.lolli.tech/lollipopkit/go-lang-lk/compiler/lexer"
	"git.lolli.tech/lollipopkit/go-lang-lk/consts"
)



var (
	replaceRules = map[string]*regexp.Regexp{
		// for in：自动添加range
		"for $1 in range($3) {": consts.ForInRe,
	}
)

/* recursive descent parser */

func Parse(chunk, chunkName string) *Block {
	chunk = beforeParse(chunk)

	lexer := NewLexer(chunk, chunkName)
	block := parseBlock(lexer)
	lexer.NextTokenOfKind(TOKEN_EOF)
	return block
}

func beforeParse(chunk string) string {
	for k := range replaceRules {
		chunk = replaceRules[k].ReplaceAllString(chunk, k)
	}
	return chunk
}
