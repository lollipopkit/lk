#!/usr/bin/env sh
set -eu

repo="${LK_REPO:-lollipopkit/lk}"
install_dir="${LK_INSTALL_DIR:-$HOME/.local/bin}"
version="${LK_VERSION:-}"

need() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

detect_os() {
  case "$(uname -s)" in
    Linux) echo "linux" ;;
    Darwin) echo "macos" ;;
    MINGW*|MSYS*|CYGWIN*) echo "windows" ;;
    *)
      echo "error: unsupported OS: $(uname -s)" >&2
      exit 1
      ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) echo "x86_64" ;;
    arm64|aarch64) echo "aarch64" ;;
    *)
      echo "error: unsupported architecture: $(uname -m)" >&2
      exit 1
      ;;
  esac
}

github_api() {
  if [ -n "${GITHUB_TOKEN:-}" ]; then
    curl -fsSL \
      -H "Accept: application/vnd.github+json" \
      -H "Authorization: Bearer $GITHUB_TOKEN" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "$1"
  else
    curl -fsSL \
      -H "Accept: application/vnd.github+json" \
      -H "X-GitHub-Api-Version: 2022-11-28" \
      "$1"
  fi
}

latest_version() {
  github_api "https://api.github.com/repos/$repo/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

need curl
need sed
need mktemp

os="${LK_OS:-$(detect_os)}"
arch="${LK_ARCH:-$(detect_arch)}"

case "$os" in
  linux|macos)
    archive_ext="tar.gz"
    bin_ext=""
    need tar
    ;;
  windows)
    archive_ext="zip"
    bin_ext=".exe"
    need unzip
    ;;
  *)
    echo "error: unsupported LK_OS: $os" >&2
    exit 1
    ;;
esac

if [ -z "$version" ]; then
  version="$(latest_version)"
fi

if [ -z "$version" ]; then
  echo "error: could not resolve latest release tag for $repo" >&2
  exit 1
fi

asset="lk-$version-$os-$arch.$archive_ext"
url="https://github.com/$repo/releases/download/$version/$asset"
tmp="$(mktemp -d)"

cleanup() {
  rm -rf "$tmp"
}
trap cleanup EXIT INT TERM

echo "Downloading $url"
curl -fL "$url" -o "$tmp/$asset"

case "$archive_ext" in
  tar.gz) tar -xzf "$tmp/$asset" -C "$tmp" ;;
  zip) unzip -q "$tmp/$asset" -d "$tmp" ;;
esac

payload="$tmp/lk-$version-$os-$arch"
if [ ! -d "$payload" ]; then
  payload="$(find "$tmp" -type f -name "lk$bin_ext" -exec dirname {} \; | head -n 1)"
fi

if [ -z "$payload" ] || [ ! -f "$payload/lk$bin_ext" ] || [ ! -f "$payload/lk-lsp$bin_ext" ]; then
  echo "error: archive does not contain lk$bin_ext and lk-lsp$bin_ext" >&2
  exit 1
fi

mkdir -p "$install_dir"
cp "$payload/lk$bin_ext" "$install_dir/"
cp "$payload/lk-lsp$bin_ext" "$install_dir/"
chmod +x "$install_dir/lk$bin_ext" "$install_dir/lk-lsp$bin_ext"

echo "Installed LK $version to $install_dir"
echo "Binaries:"
echo "  $install_dir/lk$bin_ext"
echo "  $install_dir/lk-lsp$bin_ext"
