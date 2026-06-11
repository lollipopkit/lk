const root = require("path").join(__dirname, "..", "..");

module.exports = require("node-addon-api").init({
  "target": "tree_sitter_lk_binding",
  "addon": {
    "sources": [root + "/src/parser.c", root + "/src/scanner.c"],
    "include_dirs": [root + "/src", root],
  },
});