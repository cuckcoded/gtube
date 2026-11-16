#!/bin/bash
# Builds build/GTube.app (and build/GTube-macos-arm64.zip) from
# target/release/gtube, with GTK, libadwaita and libmpv from Homebrew:
#   brew install gtk4 libadwaita mpv adwaita-icon-theme librsvg dylibbundler
#
# The app is signed ad hoc (not notarized): after downloading it, open it
# with right-click → Open, or run: xattr -dr com.apple.quarantine GTube.app
set -Eeuo pipefail
trap 'echo "${BASH_SOURCE[0]}:$LINENO: failed: $BASH_COMMAND" >&2' ERR
cd "$(dirname "$0")/../.."

APP_ID=io.github.gtube
BREW=$(brew --prefix)
BUILD=$PWD/build
APP=$BUILD/GTube.app
C=$APP/Contents
RES=$C/Resources
PIX=$RES/lib/gdk-pixbuf-2.0/2.10.0
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n1)
MIN_MACOS=$(sw_vers -productVersion | cut -d. -f1).0

rm -rf "$APP"
mkdir -p "$C/MacOS" "$C/Frameworks" "$RES/share/icons" "$RES/share/glib-2.0/schemas" "$PIX/loaders"
cp target/release/gtube "$C/MacOS/gtube"

cat > "$C/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key><string>GTube</string>
    <key>CFBundleDisplayName</key><string>GTube</string>
    <key>CFBundleIdentifier</key><string>$APP_ID</string>
    <key>CFBundleExecutable</key><string>gtube</string>
    <key>CFBundleIconFile</key><string>gtube</string>
    <key>CFBundlePackageType</key><string>APPL</string>
    <key>CFBundleVersion</key><string>$VERSION</string>
    <key>CFBundleShortVersionString</key><string>$VERSION</string>
    <key>LSMinimumSystemVersion</key><string>$MIN_MACOS</string>
    <key>LSApplicationCategoryType</key><string>public.app-category.video</string>
    <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
EOF

# App icon.
iconset=$BUILD/gtube.iconset
rm -rf "$iconset" && mkdir -p "$iconset"
for s in 16 32 128 256 512; do
    rsvg-convert -w $s -h $s "data/icons/scalable/apps/$APP_ID.svg" -o "$iconset/icon_${s}x${s}.png"
    rsvg-convert -w $((s * 2)) -h $((s * 2)) "data/icons/scalable/apps/$APP_ID.svg" -o "$iconset/icon_${s}x${s}@2x.png"
done
iconutil -c icns "$iconset" -o "$RES/gtube.icns"

# GTK's data: settings schemas, icon themes (src/bundle.rs points GTK here).
cp "$BREW"/share/glib-2.0/schemas/org.gtk.gtk4.*.xml "$RES/share/glib-2.0/schemas/"
glib-compile-schemas "$RES/share/glib-2.0/schemas"
cp -RL "$BREW/share/icons/Adwaita" "$RES/share/icons/"
mkdir -p "$RES/share/icons/hicolor/scalable/apps"
cp "$BREW/share/icons/hicolor/index.theme" "$RES/share/icons/hicolor/"
cp "data/icons/scalable/apps/$APP_ID.svg" "$RES/share/icons/hicolor/scalable/apps/"
for theme in Adwaita hicolor; do
    gtk4-update-icon-cache -q -t -f "$RES/share/icons/$theme"
done

# Image loaders (SVG icons need librsvg's). Their list names them by absolute
# path; src/bundle.rs writes the real one at startup from this template.
# They are listed where Homebrew keeps them (their libraries are found
# relative to there), with the paths then pointed into the bundle.
originals=()
for dir in "$BREW/lib/gdk-pixbuf-2.0/2.10.0/loaders" "$(brew --prefix librsvg)/lib/gdk-pixbuf-2.0/2.10.0/loaders"; do
    [ -d "$dir" ] || continue
    for f in "$dir"/*.so; do
        [ -e "$f" ] || continue
        name=$(basename "$f")
        [ -e "$PIX/loaders/$name" ] && continue # the same loader, linked into both places
        cp -L "$f" "$PIX/loaders/$name"
        chmod u+w "$PIX/loaders/$name"
        originals+=("$(realpath "$f")")
    done
done
[ ${#originals[@]} -gt 0 ] || { echo "no image loaders found" >&2; exit 1; }
gdk-pixbuf-query-loaders "${originals[@]}" \
    | sed -E 's#^"[^"]*/([^/"]+\.so)"#"@RESOURCES@/lib/gdk-pixbuf-2.0/2.10.0/loaders/\1"#' > "$PIX/loaders.cache.in"
grep -q '"@RESOURCES@/lib/gdk-pixbuf-2.0/2.10.0/loaders/[^"]*svg[^"]*\.so"' "$PIX/loaders.cache.in" \
    || { cat "$PIX/loaders.cache.in"; echo "the SVG image loader is missing" >&2; exit 1; }

# Every Homebrew library the program and the loaders use, into Frameworks.
loader_args=()
for f in "$PIX"/loaders/*.so; do loader_args+=(-x "$f"); done
# (stdin closed: dylibbundler asks on the terminal when it can't find a library)
dylibbundler -of -b -x "$C/MacOS/gtube" "${loader_args[@]}" -d "$C/Frameworks" -p @executable_path/../Frameworks/ \
    -s "$BREW/lib" </dev/null
if otool -L "$C/MacOS/gtube" "$C"/Frameworks/*.dylib "$PIX"/loaders/*.so | grep -E "^[[:space:]]+($BREW|/usr/local/(opt|Cellar))"; then
    echo "libraries still point into Homebrew" >&2
    exit 1
fi

packaging/fetch-tools.sh macos "$C/MacOS"

# dylibbundler's edits invalidate signatures; Apple Silicon won't load
# unsigned code, so sign what was changed (ad hoc).
codesign --force --sign - "$C"/Frameworks/*.dylib "$PIX"/loaders/*.so
codesign --force --sign - "$C/MacOS/gtube"

rm -f "$BUILD/GTube-macos-arm64.zip"
ditto -c -k --keepParent "$APP" "$BUILD/GTube-macos-arm64.zip"
echo "Built $APP and $BUILD/GTube-macos-arm64.zip"
