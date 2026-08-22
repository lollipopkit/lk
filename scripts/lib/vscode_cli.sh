#!/usr/bin/env bash
# Shared discovery of VS Code-family CLIs, used by install_vsix.sh and
# debug-vscode-lsp.sh.
#
# Two things make this more than `command -v code`:
#
#   1. Remote windows. Under WSL / SSH remote / devcontainers the extension has
#      to be installed on the *remote* side (that is where lk-lsp runs), so the
#      remote server's own CLI is preferred over anything on PATH. VS Code's
#      integrated terminal puts `remote-cli/code` on PATH, but a plain WSL shell
#      does not, and `remote-cli/code` only works while a window is attached
#      (it talks over $VSCODE_IPC_HOOK_CLI). The server ships a second CLI —
#      `server/bin/code-server` — that installs offline into
#      ~/.vscode-server/extensions with no window at all, so both are emitted
#      and the caller tries them in order.
#   2. Forks and channels. VS Code, Insiders, VSCodium, Cursor and Windsurf are
#      separate installs with separate extension directories; "install
#      everywhere" means one install per product, not one install total.
#
# Entry point: lk_vscode_cli_candidates, which prints "product<TAB>path" lines,
# best candidate first, deduplicated by path.

lk_vscode_os() {
  if [ -z "${_LK_VSCODE_OS:-}" ]; then
    case "$(uname -s)" in
      Darwin) _LK_VSCODE_OS=macos ;;
      Linux) _LK_VSCODE_OS=linux ;;
      MINGW* | MSYS* | CYGWIN*) _LK_VSCODE_OS=windows ;;
      *) _LK_VSCODE_OS=unknown ;;
    esac
  fi
  printf '%s\n' "$_LK_VSCODE_OS"
}

# _lk_vscode_emit PRODUCT PATH — print the candidate if it looks runnable.
_lk_vscode_emit() {
  local product="$1" path="$2"
  [ -n "$path" ] || return 0
  if [ -x "$path" ] || { [ "$(lk_vscode_os)" = windows ] && [ -f "$path" ]; }; then
    printf '%s\t%s\n' "$product" "$path"
  fi
}

# Remote server installs: ~/.vscode-server & friends.
_lk_vscode_remote_candidates() {
  local entry root product bin_dir cli
  for entry in \
    "$HOME/.vscode-server:vscode" \
    "$HOME/.vscode-server-insiders:vscode-insiders" \
    "$HOME/.vscodium-server:vscodium" \
    "$HOME/.cursor-server:cursor" \
    "$HOME/.windsurf-server:windsurf"; do
    root="${entry%:*}"
    product="${entry##*:}"
    [ -d "$root" ] || continue
    # Newest server build first; both the current (cli/servers/*) and the older
    # (bin/*) layouts. Capped so a long-lived machine with a dozen stale server
    # versions does not turn one failure into a dozen timeouts.
    while IFS= read -r bin_dir; do
      [ -d "$bin_dir" ] || continue
      local remote_cli='' server_cli=''
      for cli in "$bin_dir"/remote-cli/*; do
        if [ -x "$cli" ]; then
          remote_cli="$cli"
          break
        fi
      done
      # code-server can install extensions offline but cannot open a window, so
      # it is not a candidate for launch-mode callers.
      if [ "${_lk_vscode_launch_only:-0}" != 1 ] && [ -x "$bin_dir/code-server" ]; then
        server_cli="$bin_dir/code-server"
      fi
      # remote-cli needs an attached window; prefer it only when one is there.
      if [ -n "${VSCODE_IPC_HOOK_CLI:-}" ]; then
        _lk_vscode_emit "$product" "$remote_cli"
        _lk_vscode_emit "$product" "$server_cli"
      else
        _lk_vscode_emit "$product" "$server_cli"
        _lk_vscode_emit "$product" "$remote_cli"
      fi
    done < <(
      {
        ls -1dt "$root"/cli/servers/*/server/bin 2>/dev/null
        ls -1dt "$root"/bin/*/bin 2>/dev/null
      } | head -n 4
    )
  done
}

_lk_vscode_path_candidates() {
  local entry name product found
  for entry in \
    "code:vscode" \
    "code-insiders:vscode-insiders" \
    "codium:vscodium" \
    "vscodium:vscodium" \
    "code-oss:code-oss" \
    "cursor:cursor" \
    "windsurf:windsurf"; do
    name="${entry%:*}"
    product="${entry##*:}"
    found="$(command -v "$name" 2>/dev/null)" || continue
    _lk_vscode_emit "$product" "$found"
  done
}

_lk_vscode_macos_candidates() {
  local dir entry app product
  for dir in "/Applications" "$HOME/Applications"; do
    for entry in \
      "Visual Studio Code.app/Contents/Resources/app/bin/code:vscode" \
      "Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code:vscode-insiders" \
      "Visual Studio Code - Insiders.app/Contents/Resources/app/bin/code-insiders:vscode-insiders" \
      "VSCodium.app/Contents/Resources/app/bin/codium:vscodium" \
      "Cursor.app/Contents/Resources/app/bin/cursor:cursor" \
      "Windsurf.app/Contents/Resources/app/bin/windsurf:windsurf"; do
      app="${entry%:*}"
      product="${entry##*:}"
      _lk_vscode_emit "$product" "$dir/$app"
    done
  done
}

_lk_vscode_linux_candidates() {
  local entry path product
  for entry in \
    "/usr/share/code/bin/code:vscode" \
    "/usr/lib/code/bin/code:vscode" \
    "/opt/visual-studio-code/bin/code:vscode" \
    "/snap/bin/code:vscode" \
    "/var/lib/flatpak/exports/bin/com.visualstudio.code:vscode" \
    "$HOME/.local/share/flatpak/exports/bin/com.visualstudio.code:vscode" \
    "/usr/share/code-insiders/bin/code-insiders:vscode-insiders" \
    "/snap/bin/code-insiders:vscode-insiders" \
    "/usr/share/codium/bin/codium:vscodium" \
    "/opt/vscodium-bin/bin/codium:vscodium" \
    "/var/lib/flatpak/exports/bin/com.vscodium.codium:vscodium" \
    "$HOME/.local/share/flatpak/exports/bin/com.vscodium.codium:vscodium" \
    "/usr/share/code-oss/bin/code-oss:code-oss" \
    "/usr/lib/code-oss/bin/code-oss:code-oss" \
    "/opt/cursor/bin/cursor:cursor" \
    "/usr/share/cursor/bin/cursor:cursor" \
    "/opt/windsurf/bin/windsurf:windsurf"; do
    path="${entry%:*}"
    product="${entry##*:}"
    _lk_vscode_emit "$product" "$path"
  done
}

_lk_vscode_windows_candidates() {
  local roots=() root entry rel product
  # In Git Bash/MSYS these are Windows paths (C:\Users\...); cygpath makes them
  # usable from the shell, and bash can execute .cmd wrappers directly.
  for root in "${LOCALAPPDATA:-}" "${ProgramFiles:-}" "${ProgramW6432:-}" "${PROGRAMFILES:-}"; do
    [ -n "$root" ] || continue
    if command -v cygpath >/dev/null 2>&1; then
      root="$(cygpath -u "$root" 2>/dev/null || printf '%s' "$root")"
    fi
    roots+=("$root")
  done
  roots+=("/c/Program Files" "/c/Program Files (x86)")
  for root in "${roots[@]}"; do
    [ -d "$root" ] || continue
    for entry in \
      "Programs/Microsoft VS Code/bin/code.cmd:vscode" \
      "Microsoft VS Code/bin/code.cmd:vscode" \
      "Programs/Microsoft VS Code Insiders/bin/code-insiders.cmd:vscode-insiders" \
      "Microsoft VS Code Insiders/bin/code-insiders.cmd:vscode-insiders" \
      "Programs/VSCodium/bin/codium.cmd:vscodium" \
      "VSCodium/bin/codium.cmd:vscodium" \
      "Programs/cursor/resources/app/bin/cursor.cmd:cursor" \
      "Programs/Windsurf/bin/windsurf.cmd:windsurf"; do
      rel="${entry%:*}"
      product="${entry##*:}"
      _lk_vscode_emit "$product" "$root/$rel"
    done
  done
}

# lk_vscode_cli_candidates [launchable]
# Prints "product<TAB>path", best first, deduplicated by path. Pass "launchable"
# to exclude CLIs that can install extensions but cannot open a window.
lk_vscode_cli_candidates() {
  local _lk_vscode_launch_only=0
  [ "${1:-}" = launchable ] && _lk_vscode_launch_only=1
  {
    # An explicit override is the whole answer: do not fan out to other editors
    # when the caller named one.
    local override="${VSCODE_CLI:-${CODE_BIN:-}}"
    if [ -n "$override" ]; then
      if [ -x "$override" ] || command -v "$override" >/dev/null 2>&1; then
        printf '%s\t%s\n' "override" "$override"
      else
        printf 'lk: VSCODE_CLI/CODE_BIN is set to %s, which is not executable\n' "$override" >&2
      fi
    else
      _lk_vscode_remote_candidates
      _lk_vscode_path_candidates
      case "$(lk_vscode_os)" in
        macos) _lk_vscode_macos_candidates ;;
        linux) _lk_vscode_linux_candidates ;;
        windows) _lk_vscode_windows_candidates ;;
      esac
    fi
  } | awk -F'\t' '!seen[$2]++'
}

# First candidate only — for callers that just need one CLI (debug host).
lk_vscode_cli() {
  lk_vscode_cli_candidates "${1:-}" | head -n 1 | cut -f2
}
