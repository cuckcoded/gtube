#!/usr/bin/env bash
# Install or uninstall GTube.
#
#   ./install.sh                     build and install for your user (~/.local)
#   ./install.sh uninstall           remove it (keeps settings, session, downloads)
#   ./install.sh uninstall --purge   also delete settings, session and caches
#
# System-wide: cargo build --release && sudo PREFIX=/usr/local ./install.sh
set -euo pipefail

APP_ID=io.github.gtube
PREFIX="${PREFIX:-$HOME/.local}"
BIN="$PREFIX/bin/gtube"
DESKTOP="$PREFIX/share/applications/$APP_ID.desktop"
ICONS="$PREFIX/share/icons/hicolor"
ICON="$ICONS/scalable/apps/$APP_ID.svg"

cd "$(dirname "$0")"

say() { printf '\033[1m%s\033[0m\n' "$*"; }

refresh_caches() {
    command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null || true
    # Only refresh an existing icon cache: creating one in ~/.local would go
    # stale and hide icons other apps install later.
    if [ -f "$ICONS/icon-theme.cache" ] && command -v gtk-update-icon-cache >/dev/null; then
        gtk-update-icon-cache -q -t -f "$ICONS" 2>/dev/null || true
    fi
}

check_deps() {
    local missing=()
    command -v yt-dlp >/dev/null || missing+=("yt-dlp (needed to play and download)")
    command -v deno >/dev/null || command -v node >/dev/null || command -v bun >/dev/null \
        || missing+=("deno or node (yt-dlp needs a JS runtime for YouTube)")
    command -v ffmpeg >/dev/null || missing+=("ffmpeg (needed for downloads)")
    if [ ${#missing[@]} -gt 0 ]; then
        say "Missing runtime dependencies:"
        printf '  - %s\n' "${missing[@]}"
    fi
}

install_app() {
    if [ "$(id -u)" -eq 0 ]; then
        # Don't build as root: it would leave root-owned files in target/.
        [ -x target/release/gtube ] || { echo "Build first as your user: cargo build --release" >&2; exit 1; }
    else
        say "Building…"
        cargo build --release
    fi

    say "Installing to $PREFIX…"
    install -Dm755 target/release/gtube "$BIN"
    install -Dm644 "data/icons/scalable/apps/$APP_ID.svg" "$ICON"
    mkdir -p "$(dirname "$DESKTOP")"
    # Absolute path: the desktop session's PATH may not include $PREFIX/bin.
    sed "s|^Exec=gtube|Exec=\"$BIN\"|" "data/$APP_ID.desktop" > "$DESKTOP"
    chmod 644 "$DESKTOP"
    refresh_caches

    say "Installed. Launch GTube from your app menu, or run: $BIN"
    case ":$PATH:" in *":$PREFIX/bin:"*) ;; *) echo "(Note: $PREFIX/bin is not in your PATH.)" ;; esac
    check_deps
}

uninstall_app() {
    local purge=${1:-}
    say "Removing GTube from $PREFIX…"
    rm -fv "$BIN" "$DESKTOP" "$ICON"
    refresh_caches

    if [ "$purge" = "--purge" ]; then
        if [ "$(id -u)" -eq 0 ]; then
            echo "Run '--purge' as your own user, not root, to remove your data." >&2
            exit 1
        fi
        say "Deleting settings, session and caches…"
        rm -rfv "${XDG_CONFIG_HOME:-$HOME/.config}/gtube" \
                "${XDG_DATA_HOME:-$HOME/.local/share}/gtube" \
                "${XDG_CACHE_HOME:-$HOME/.cache}/gtube"
    else
        echo "Kept your settings and session (${XDG_CONFIG_HOME:-$HOME/.config}/gtube); use 'uninstall --purge' to delete them."
    fi
    echo "Downloaded videos are never touched."
    say "Uninstalled."
}

case "${1:-install}" in
    install) install_app ;;
    uninstall) uninstall_app "${2:-}" ;;
    -h|--help|help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//' ;;
    *) echo "Usage: $0 [install | uninstall [--purge]]" >&2; exit 2 ;;
esac
