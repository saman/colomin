#!/bin/bash
#
# Regenerate all icon assets from assets/icon.svg (the source of truth).
#
# Outputs:
#   assets/Colomin.icns       — multi-resolution macOS bundle icon (16→1024)
#   assets/app_icon_256.png   — 256x256 PNG embedded in the binary (src/main.rs)
#   docs/images/icon.svg      — copy for the website
#
# Requires: sips, iconutil (both ship with macOS).

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$PROJECT_DIR/assets/icon.svg"

if [ ! -f "$SRC" ]; then
    echo "Error: source SVG not found at $SRC" >&2
    exit 1
fi

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

ICONSET="$TMPDIR/icon.iconset"
mkdir -p "$ICONSET"

echo "Removing previous outputs..."
rm -f \
    "$PROJECT_DIR/assets/Colomin.icns" \
    "$PROJECT_DIR/assets/app_icon_256.png" \
    "$PROJECT_DIR/docs/images/icon.svg"

echo "Rasterizing $SRC at 1024×1024..."
sips -s format png "$SRC" --resampleHeightWidth 1024 1024 \
    --out "$TMPDIR/icon_1024.png" >/dev/null

echo "Generating iconset sizes..."
# size:filename pairs for a complete macOS iconset.
for entry in \
    "16:icon_16x16.png" \
    "32:icon_16x16@2x.png" \
    "32:icon_32x32.png" \
    "64:icon_32x32@2x.png" \
    "128:icon_128x128.png" \
    "256:icon_128x128@2x.png" \
    "256:icon_256x256.png" \
    "512:icon_256x256@2x.png" \
    "512:icon_512x512.png" \
    "1024:icon_512x512@2x.png"
do
    size="${entry%%:*}"
    name="${entry##*:}"
    sips -z "$size" "$size" "$TMPDIR/icon_1024.png" \
        --out "$ICONSET/$name" >/dev/null
done

echo "Building Colomin.icns..."
iconutil -c icns "$ICONSET" -o "$PROJECT_DIR/assets/Colomin.icns"

echo "Writing assets/app_icon_256.png..."
cp -f "$ICONSET/icon_256x256.png" "$PROJECT_DIR/assets/app_icon_256.png"

echo "Copying SVG to docs/images/..."
mkdir -p "$PROJECT_DIR/docs/images"
cp -f "$SRC" "$PROJECT_DIR/docs/images/icon.svg"

echo ""
echo "Done. Updated:"
echo "  $PROJECT_DIR/assets/Colomin.icns"
echo "  $PROJECT_DIR/assets/app_icon_256.png"
echo "  $PROJECT_DIR/docs/images/icon.svg"
