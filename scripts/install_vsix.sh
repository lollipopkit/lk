#!/usr/bin/env bash
# Install a packaged VSIX into every VS Code-family editor found on this
# machine (see scripts/lib/vscode_cli.sh for how they are found).
#
# Usage: scripts/install_vsix.sh [PATH_TO_VSIX]
#   Defaults to the newest VSIX under ecosystem/vsc-ext/lsp.
#
# Environment:
#   VSCODE_CLI / CODE_BIN   install with this CLI only
#   LK_VSIX_TIMEOUT         per-attempt timeout in seconds (default 180)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib/vscode_cli.sh
. "$ROOT/scripts/lib/vscode_cli.sh"

vsix="${1:-}"
if [ -z "$vsix" ]; then
  vsix="$(ls -t "$ROOT"/ecosystem/vsc-ext/lsp/*.vsix 2>/dev/null | head -n 1 || true)"
fi
if [ -z "$vsix" ] || [ ! -f "$vsix" ]; then
  echo "install_vsix: no VSIX found; run 'make vsix' first" >&2
  exit 1
fi
# VS Code's CLI resolves a relative path against its own cwd, not ours.
case "$vsix" in
  /*) ;;
  *) vsix="$(cd "$(dirname "$vsix")" && pwd)/$(basename "$vsix")" ;;
esac

run_cli() {
  if command -v timeout >/dev/null 2>&1; then
    timeout "${LK_VSIX_TIMEOUT:-180}" "$@"
  else
    "$@"
  fi
}

candidates="$(lk_vscode_cli_candidates)"
if [ -z "$candidates" ]; then
  cat >&2 <<EOF
install_vsix: no VS Code-family CLI found.

Install manually: VS Code > Extensions > ... > Install from VSIX... >
  $vsix
Or point the installer at a CLI:
  make install-vsix VSCODE_CLI=/path/to/code
EOF
  exit 1
fi

products="$(printf '%s\n' "$candidates" | cut -f1 | awk '!seen[$0]++')"
installed=()
failed=()

for product in $products; do
  ok=0
  while IFS=$'\t' read -r cand_product cand_path; do
    [ "$cand_product" = "$product" ] || continue
    echo "==> $product: $cand_path"
    # --force so an already-installed same-version extension is replaced
    # instead of refused.
    if run_cli "$cand_path" --install-extension "$vsix" --force; then
      installed+=("$product ($cand_path)")
      ok=1
      break
    fi
    echo "    failed, trying the next candidate for $product" >&2
  done <<<"$candidates"
  [ "$ok" = 1 ] || failed+=("$product")
done

echo
[ ${#installed[@]} -eq 0 ] || printf 'installed: %s\n' "${installed[@]}"
[ ${#failed[@]} -eq 0 ] || printf 'not installed: %s (no working CLI; install the VSIX from its UI)\n' "${failed[@]}" >&2

if [ ${#installed[@]} -eq 0 ]; then
  echo "install_vsix: every candidate failed for $vsix" >&2
  exit 1
fi
