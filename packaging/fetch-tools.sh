#!/bin/bash
# Downloads the tools GTube's packages ship next to the program:
#   yt-dlp (finds the streams) and deno (the JavaScript runtime yt-dlp needs
#   for YouTube), both the latest release, checksums verified.
# A yt-dlp installed on the system is still preferred at runtime.
#
# usage: fetch-tools.sh linux|windows|macos DEST_DIR
set -Eeuo pipefail
trap 'echo "${BASH_SOURCE[0]}:$LINENO: failed: $BASH_COMMAND" >&2' ERR

case "${1:-}" in
    linux)   ytdlp=yt-dlp_linux; ytdlp_out=yt-dlp;     deno=deno-x86_64-unknown-linux-gnu.zip; deno_bin=deno ;;
    windows) ytdlp=yt-dlp.exe;   ytdlp_out=yt-dlp.exe; deno=deno-x86_64-pc-windows-msvc.zip;   deno_bin=deno.exe ;;
    macos)   ytdlp=yt-dlp_macos; ytdlp_out=yt-dlp;     deno=deno-aarch64-apple-darwin.zip;    deno_bin=deno ;;
    *) echo "usage: $0 linux|windows|macos DEST_DIR" >&2; exit 2 ;;
esac
dest=${2:?destination directory}
mkdir -p "$dest"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fetch() { curl -fsSL --retry 3 --retry-delay 5 -o "$1" "$2"; }
sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | awk '{print tolower($1)}'
}
check() { # FILE EXPECTED
    local got
    got=$(sha256 "$1")
    if [ "$got" != "$2" ]; then
        echo "Checksum mismatch for $(basename "$1"): got $got, expected $2" >&2
        exit 1
    fi
}

yt=https://github.com/yt-dlp/yt-dlp/releases/latest/download
fetch "$tmp/$ytdlp" "$yt/$ytdlp"
fetch "$tmp/SHA2-256SUMS" "$yt/SHA2-256SUMS"
check "$tmp/$ytdlp" "$(awk -v f="$ytdlp" '$2 == f {print tolower($1)}' "$tmp/SHA2-256SUMS")"
install -m755 "$tmp/$ytdlp" "$dest/$ytdlp_out"

dn=https://github.com/denoland/deno/releases/latest/download
fetch "$tmp/$deno" "$dn/$deno"
fetch "$tmp/$deno.sha256sum" "$dn/$deno.sha256sum"
# The checksum files differ in format between platforms; the hash is the
# only 64-hex-digit word in each.
check "$tmp/$deno" "$(grep -oiE '[0-9a-f]{64}' "$tmp/$deno.sha256sum" | head -n1 | tr 'A-F' 'a-f')"
unzip -q -o "$tmp/$deno" -d "$tmp/deno"
install -m755 "$tmp/deno/$deno_bin" "$dest/$deno_bin"

echo "yt-dlp $("$dest/$ytdlp_out" --version 2>/dev/null || echo '(not runnable here)'), $("$dest/$deno_bin" --version 2>/dev/null | head -n1 || true) in $dest"
