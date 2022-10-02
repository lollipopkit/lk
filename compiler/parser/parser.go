package parser

import (
	"encoding/json"
	"io/ioutil"
	"regexp"

	. "git.lolli.tech/lollipopkit/lk/compiler/ast"
	. "git.lolli.tech/lollipopkit/lk/compiler/lexer"
	"git.lolli.tech/lollipopkit/lk/consts"
)

var (
	replaceRules = map[string]*regexp.Regexp{
		// for in：自动添加range
		"for $1 in range($3) {": consts.ForInRe,
		"$1 = $1 + 1":           consts.NameExpPPRe,
		"$1 = $1 - 1":           consts.NameExpMMRe,
		"$1 = $1 + $2":          consts.NameExpAddRe,
		"$1 = $1 - $2":          consts.NameExpSubRe,
		"$1 = $1 * $2":          consts.NameExpMulRe,
		"$1 = $1 / $2":          consts.NameExpDivRe,
		"$1 = $1 % $2":          consts.NameExpModRe,
		"$1 = $1 ^ $2":          consts.NameExpPowRe,
		"$1 = $1 & $2":          consts.NameExpAndRe,
		"$1 = $1 | $2":          consts.NameExpOrRe,
		"$1 = $1 << $2":         consts.NameExpLShiftRe,
		"$1 = $1 >> $2":         consts.NameExpRShiftRe,
	}
)

/* recursive descent parser */

func Parse(chunk, chunkName string) *Block {
	chunk = beforeParse(chunk)

	lexer := NewLexer(chunk, chunkName)
	block := parseBlock(lexer)

	if consts.Debug {
		data, err := json.MarshalIndent(block, "", "  ")
		if err != nil {
			panic(err)
		}
		ioutil.WriteFile(chunkName+".ast.json", data, 0644)
	}

	lexer.NextTokenOfKind(TOKEN_EOF)
	return block
}

func beforeParse(chunk string) string {
	for k := range replaceRules {
		chunk = replaceRules[k].ReplaceAllString(chunk, k)
	}
	return chunk
}
