#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT="$ROOT/ecosystem/vsc-ext/lsp"
EXAMPLES="$ROOT/examples/lk-example-workspace"
SERVER="$ROOT/target/debug/lk-lsp"
USER_DATA_DIR="${LK_VSCODE_USER_DATA_DIR:-$(mktemp -d "${TMPDIR:-/tmp}/lk-vscode-lsp.XXXXXX")}"

# shellcheck source=lib/vscode_cli.sh
. "$ROOT/scripts/lib/vscode_cli.sh"

find_code_bin() {
  local found
  # launchable: this opens an Extension Development Host window, which the
  # server-side code-server CLI cannot do.
  found="$(lk_vscode_cli launchable)"
  if [[ -n "$found" ]]; then
    printf '%s\n' "$found"
    return
  fi

  echo "error: VS Code CLI 'code' not found" >&2
  echo "Set CODE_BIN to the VS Code CLI path, for example:" >&2
  echo "  CODE_BIN=\"/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code\" make debug-lsp-ext" >&2
  exit 1
}

CODE_CLI="$(find_code_bin)"

echo "Building lk-lsp..."
cargo build -p lk-lsp

if [[ ! -d "$EXT/node_modules" ]]; then
  echo "Installing VS Code extension dependencies..."
  npm --prefix "$EXT" install
fi

echo "Compiling VS Code extension..."
npm --prefix "$EXT" run compile

mkdir -p "$USER_DATA_DIR/User"
cat >"$USER_DATA_DIR/User/settings.json" <<EOF
{
  "lk.lsp.serverPath": "$SERVER",
  "lk.lsp.trace": "verbose",
  "lk.lsp.outputChannel.enabled": true,
  "files.associations": {
    "*.lk": "lk"
  }
}
EOF

echo "Opening VS Code Extension Development Host..."
echo "Extension: $EXT"
echo "Folder:    $EXAMPLES"
echo "Server:    $SERVER"
echo "User data: $USER_DATA_DIR"
echo "VS Code:   $CODE_CLI"

"$CODE_CLI" \
  --extensionDevelopmentPath="$EXT" \
  --user-data-dir="$USER_DATA_DIR" \
  --new-window "$EXAMPLES"
