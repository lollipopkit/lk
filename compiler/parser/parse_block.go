package parser

import (
	. "github.com/lollipopkit/lk/compiler/ast"
	. "github.com/lollipopkit/lk/compiler/lexer"
)

// block ::= {stat} [retstat]
func ParseBlock(lexer *Lexer) *Block {
	return &Block{
		Stats:    ParseStats(lexer),
		RetExps:  parseRetExps(lexer),
		LastLine: lexer.Line(),
	}
}

func ParseStats(lexer *Lexer) []Stat {
	stats := make([]Stat, 0, 8)
	for !_isReturnOrBlockEnd(lexer.LookAhead()) {
		stat := ParseStat(lexer)
		if _, ok := stat.(*EmptyStat); !ok {
			stats = append(stats, stat)
		}
	}
	return stats
}

func _isReturnOrBlockEnd(tokenKind int) bool {
	switch tokenKind {
	case TOKEN_KW_RETURN, TOKEN_EOF, TOKEN_SEP_RCURLY:
		return true
	}
	return false
}

// retstat ::= return [explist] [‘;’]
// explist ::= exp {‘,’ exp}
func parseRetExps(lexer *Lexer) []Exp {
	if lexer.LookAhead() != TOKEN_KW_RETURN {
		return nil
	}

	lexer.NextToken()
	switch lexer.LookAhead() {
	case TOKEN_EOF, TOKEN_SEP_RCURLY:
		return []Exp{}
	case TOKEN_SEP_SEMI:
		lexer.NextToken()
		return []Exp{}
	default:
		exps := parseExpList(lexer)
		if lexer.LookAhead() == TOKEN_SEP_SEMI {
			lexer.NextToken()
		}
		return exps
	}
}
