#!/bin/bash
# Builds build/GTube (a self-contained folder: bin\gtube.exe with its DLLs,
# GTK's data in share\ and lib\) and build/GTube-windows-x86_64.zip from
# target/release/gtube.exe. Run in an MSYS2 UCRT64 shell with
#   mingw-w64-ucrt-x86_64-{gtk4,libadwaita,mpv,adwaita-icon-theme,librsvg,imagemagick} unzip zip
# packaging/windows/gtube.iss then makes the installer from the folder.
set -euo pipefail
cd "$(dirname "$0")/../.."

APP_ID=io.github.gtube
PREFIX=${MINGW_PREFIX:-/ucrt64}
BUILD=$PWD/build
OUT=$BUILD/GTube
PIX=lib/gdk-pixbuf-2.0/2.10.0

rm -rf "$OUT"
mkdir -p "$OUT/bin" "$OUT/share/glib-2.0/schemas" "$OUT/share/icons" "$OUT/$PIX/loaders"
cp target/release/gtube.exe "$OUT/bin/"

# Image loaders (SVG icons need librsvg's).
cp "$PREFIX/$PIX"/loaders/*.dll "$OUT/$PIX/loaders/"

# Every DLL from MSYS2 that the program and the loaders need (ldd lists
# them recursively). GTK finds its data relative to its DLL: bin\..\share.
copy_dlls() {
    ldd "$@" | awk '{print $3}' | grep -i "^$PREFIX/bin/" | sort -u | while read -r dll; do
        [ -e "$OUT/bin/$(basename "$dll")" ] || cp "$dll" "$OUT/bin/"
    done
}
copy_dlls "$OUT/bin/gtube.exe" "$OUT/$PIX"/loaders/*.dll
# libmpv's own plugins load more at runtime; make sure nothing is missing.
copy_dlls "$OUT"/bin/*.dll

# The loaders' list, with paths relative to the folder (gdk-pixbuf resolves
# them against its own location, so the folder can live anywhere).
GDK_PIXBUF_MODULEDIR="$OUT/$PIX/loaders" gdk-pixbuf-query-loaders \
    | sed -E 's#^"[^"]*[/\\](lib[/\\]gdk-pixbuf-2\.0[/\\]2\.10\.0[/\\]loaders[/\\][^"/\\]+)"#"\1"#' > "$OUT/$PIX/loaders.cache"
grep -q svg "$OUT/$PIX/loaders.cache" || { echo "the SVG image loader is missing" >&2; exit 1; }
if grep -E '^"[A-Za-z]:|^"/' "$OUT/$PIX/loaders.cache"; then
    echo "absolute paths left in loaders.cache" >&2
    exit 1
fi

# Settings schemas and icon themes.
cp "$PREFIX"/share/glib-2.0/schemas/org.gtk.gtk4.*.xml "$OUT/share/glib-2.0/schemas/"
glib-compile-schemas "$OUT/share/glib-2.0/schemas"
cp -r "$PREFIX/share/icons/Adwaita" "$OUT/share/icons/"
mkdir -p "$OUT/share/icons/hicolor/scalable/apps"
cp "$PREFIX/share/icons/hicolor/index.theme" "$OUT/share/icons/hicolor/"
cp "data/icons/scalable/apps/$APP_ID.svg" "$OUT/share/icons/hicolor/scalable/apps/"
for theme in Adwaita hicolor; do
    gtk4-update-icon-cache -q -t -f "$OUT/share/icons/$theme"
done

# Icon for the installer and shortcuts.
for s in 16 32 48 64 128 256; do
    rsvg-convert -w $s -h $s "data/icons/scalable/apps/$APP_ID.svg" -o "$BUILD/icon-$s.png"
done
magick "$BUILD"/icon-{16,32,48,64,128,256}.png "$OUT/gtube.ico"

packaging/fetch-tools.sh windows "$OUT/bin"

rm -f "$BUILD/GTube-windows-x86_64.zip"
(cd "$BUILD" && zip -q -r -9 GTube-windows-x86_64.zip GTube)
echo "Built $OUT and $BUILD/GTube-windows-x86_64.zip"
