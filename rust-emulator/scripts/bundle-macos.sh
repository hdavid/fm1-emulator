#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-only
# Build "FM-1 Emulator.app" around a release fm1-ui binary.
#   scripts/bundle-macos.sh PATH/TO/fm1-ui OUTPUT_DIR
# The bundle is OUTPUT_DIR/FM-1 Emulator.app: Info.plist (version from
# Cargo.toml), an icon made from macos/icon.png (or the placeholder), the binary
# in Contents/MacOS, and an ad-hoc signature (the app is not notarized).
# Only /usr/bin tools are used (sed, sips, iconutil, codesign).
set -eu
if [ "$#" -ne 2 ]; then
    echo 'usage: bundle-macos.sh PATH/TO/fm1-ui OUTPUT_DIR' >&2
    exit 2
fi
binary=$1
out=$2
crate=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
[ -f "$binary" ] || { echo "bundle-macos.sh: no binary at $binary" >&2; exit 1; }
version=$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -n 1)
[ -n "$version" ] || { echo 'bundle-macos.sh: no version in Cargo.toml' >&2; exit 1; }
icon=$crate/macos/icon.png
[ -f "$icon" ] || icon=$crate/macos/icon-placeholder.png
mkdir -p "$out"
bundle=$out/FM-1\ Emulator.app
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
sed "s/@VERSION@/$version/g" "$crate/macos/Info.plist" > "$bundle/Contents/Info.plist"
iconset=$(mktemp -d)/AppIcon.iconset
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$icon" --out "$iconset/icon_${size}x${size}.png" > /dev/null
    sips -z "$((size * 2))" "$((size * 2))" "$icon" --out "$iconset/icon_${size}x${size}@2x.png" > /dev/null
done
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/AppIcon.icns"
rm -rf "$(dirname "$iconset")"
cp "$binary" "$bundle/Contents/MacOS/fm1-ui"
chmod +x "$bundle/Contents/MacOS/fm1-ui"
codesign --force --sign - "$bundle"
echo "$bundle (version $version)"
