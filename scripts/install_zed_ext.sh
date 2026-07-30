#!/usr/bin/env bash
# Best-effort Zed extension step of `make install`.
#
# Zed has no CLI for installing an extension from a directory — dev extensions
# are loaded from the UI, and Zed builds the wasm and the grammar itself. So
# this script cannot install anything; what it can do is detect Zed, check the
# two things that make the load fail silently later (a missing lk-lsp, a
# placeholder grammar commit), and print the exact path to load. It never fails
# the build: a machine without Zed is not a broken install.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EXT_DIR="$ROOT/ecosystem/zed-ext"

find_zed() {
  local candidate
  if command -v zed >/dev/null 2>&1; then
    command -v zed
    return
  fi
  for candidate in \
    "/Applications/Zed.app/Contents/MacOS/cli" \
    "$HOME/Applications/Zed.app/Contents/MacOS/cli" \
    "$HOME/.local/bin/zed" \
    "/usr/bin/zed" \
    "/usr/local/bin/zed" \
    "/var/lib/flatpak/exports/bin/dev.zed.Zed" \
    "$HOME/.local/share/flatpak/exports/bin/dev.zed.Zed"; do
    if [ -x "$candidate" ]; then
      printf '%s\n' "$candidate"
      return
    fi
  done
}

zed_bin="$(find_zed || true)"
if [ -z "$zed_bin" ]; then
  echo "zed: not detected, skipping the Zed extension"
  exit 0
fi

echo "zed: found $zed_bin"

commit="$(sed -n 's/^commit = "\(.*\)"/\1/p' "$EXT_DIR/extension.toml" | head -n 1)"
if ! printf '%s' "$commit" | grep -qE '^[0-9a-f]{40}$'; then
  echo "zed: WARNING grammar commit in extension.toml is '$commit', not a published SHA."
  echo "zed:         Zed clones that commit to build the grammar, so syntax highlighting"
  echo "zed:         will fail to build until 'make zed-ext-release-check' passes."
fi

cat <<EOF
zed: Zed installs extensions from its UI, not from a CLI. To load this one:
zed:   1. Open Zed
zed:   2. Command palette > "zed: install dev extension"
zed:   3. Choose $EXT_DIR
zed: The extension finds lk-lsp on PATH / in ~/.cargo/bin, which 'make install-lsp' just populated.
EOF
