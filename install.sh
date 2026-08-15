#!/bin/sh
#
# install.sh -- fetch the latest winfacl release binary for this machine.
#
#   curl -fsSL https://raw.githubusercontent.com/suchattai-labs/winfacl-rs/master/install.sh | sh
#
# Environment:
#   PREFIX   install directory (default: /usr/local/bin if writable,
#            otherwise ~/.local/bin)
#
# Plain POSIX sh; needs curl or wget, nothing else.

set -u

REPO="suchattai-labs/winfacl-rs"

case "$(uname -s)" in
Linux) ;;
*)
    echo "winfacl release binaries are Linux-only (got: $(uname -s))" >&2
    exit 1
    ;;
esac

case "$(uname -m)" in
x86_64 | amd64) arch=x86_64 ;;
aarch64 | arm64) arch=aarch64 ;;
*)
    echo "no prebuilt binary for architecture '$(uname -m)'" >&2
    echo "build from source instead: cargo build --release" >&2
    exit 1
    ;;
esac

url="https://github.com/$REPO/releases/latest/download/winfacl-linux-$arch"

if [ -n "${PREFIX:-}" ]; then
    dest_dir="$PREFIX"
elif [ -w /usr/local/bin ]; then
    dest_dir=/usr/local/bin
else
    dest_dir="$HOME/.local/bin"
fi
mkdir -p "$dest_dir" || exit 1
dest="$dest_dir/winfacl"

tmp="$dest.tmp.$$"
trap 'rm -f "$tmp"' EXIT

echo "Fetching winfacl (linux-$arch) ..."
if command -v curl >/dev/null 2>&1; then
    curl -fsSL --proto '=https' -o "$tmp" "$url" || exit 1
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$tmp" "$url" || exit 1
else
    echo "need curl or wget" >&2
    exit 1
fi

chmod 0755 "$tmp" || exit 1
mv "$tmp" "$dest" || exit 1
trap - EXIT

ver=$("$dest" --version 2>/dev/null) || {
    echo "downloaded binary does not run on this system" >&2
    rm -f "$dest"
    exit 1
}
echo "Installed $ver to $dest"

case ":$PATH:" in
*":$dest_dir:"*) ;;
*) echo "note: $dest_dir is not on your PATH" ;;
esac
