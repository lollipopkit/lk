// Minimal external scanner for tree-sitter-lkr.
// Since comments are handled via regex extras in the grammar,
// this scanner is a no-op stub required by the tree-sitter ABI.

#include <tree_sitter/parser.h>

// Empty scanner state
typedef struct {
  int dummy;
} Scanner;

void *tree_sitter_lkr_external_scanner_create(void) {
  Scanner *s = (Scanner *)calloc(1, sizeof(Scanner));
  return s;
}

void tree_sitter_lkr_external_scanner_destroy(void *payload) {
  free(payload);
}

unsigned tree_sitter_lkr_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_lkr_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

bool tree_sitter_lkr_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid_symbols) {
  (void)payload;
  (void)lexer;
  (void)valid_symbols;
  return false;
}