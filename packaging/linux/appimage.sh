#!/bin/bash
# Builds build/GTube-x86_64.AppImage from target/release/gtube.
#
# Run on the oldest distribution the AppImage should support that still has
# GTK >= 4.16 and libadwaita >= 1.7 (CI uses Debian 13), with: the -dev
# packages GTube builds against, file, unzip, curl, librsvg2-common and
# adwaita-icon-theme. The GTK runtime, libmpv and their libraries go into the
# AppImage; glibc and the graphics drivers come from the system.
set -euo pipefail
cd "$(dirname "$0")/../.."

APP_ID=io.github.gtube
BUILD=$PWD/build
APPDIR=$BUILD/AppDir
TOOLS=$BUILD/appimage-tools
export APPIMAGE_EXTRACT_AND_RUN=1 # no FUSE in containers
export ARCH=x86_64

rm -rf "$APPDIR"
mkdir -p "$APPDIR/usr/bin" "$APPDIR/usr/share/icons" "$TOOLS"

fetch() { [ -s "$TOOLS/$1" ] || curl -fsSL --retry 3 -o "$TOOLS/$1" "$2"; chmod +x "$TOOLS/$1"; }
fetch linuxdeploy-x86_64.AppImage https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
fetch linuxdeploy-plugin-gtk.sh https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/master/linuxdeploy-plugin-gtk.sh
fetch appimagetool-x86_64.AppImage https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage

install -m755 target/release/gtube "$APPDIR/usr/bin/gtube"
# The symbolic icons GTube and libadwaita use.
cp -r /usr/share/icons/Adwaita "$APPDIR/usr/share/icons/"
install -Dm644 /usr/share/icons/hicolor/index.theme "$APPDIR/usr/share/icons/hicolor/index.theme"

# GTube's libraries (GTK, libadwaita, libmpv and everything below them),
# GTK's modules, settings schemas and image loaders, and the AppRun that
# points GTK at them.
PATH="$TOOLS:$PATH" DEPLOY_GTK_VERSION=4 "$TOOLS/linuxdeploy-x86_64.AppImage" \
    --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/gtube" \
    --desktop-file "data/$APP_ID.desktop" \
    --icon-file "data/icons/scalable/apps/$APP_ID.svg" \
    --plugin gtk

# The GTK plugin forces X11 and a GTK theme; libadwaita styles itself and
# runs fine on Wayland.
hook="$APPDIR/apprun-hooks/linuxdeploy-plugin-gtk.sh"
sed -i -e '/GDK_BACKEND=/d' -e '/GTK_THEME=/d' "$hook"
grep -q GDK_BACKEND "$hook" && { echo "could not remove GDK_BACKEND from $hook" >&2; exit 1; }

# yt-dlp and deno go in after linuxdeploy, which would otherwise patch
# them (and break yt-dlp's self-extracting archive).
packaging/fetch-tools.sh linux "$APPDIR/usr/bin"

"$TOOLS/appimagetool-x86_64.AppImage" --no-appstream "$APPDIR" "$BUILD/GTube-x86_64.AppImage"
echo "Built $BUILD/GTube-x86_64.AppImage"
