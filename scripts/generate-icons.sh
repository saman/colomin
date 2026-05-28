#!/bin/bash
#
# Regenerate all icon assets from assets/icon.svg (the source of truth).
#
# Outputs:
#   assets/Colomin.icns       — multi-resolution macOS bundle icon (16→1024)
#   assets/Colomin.ico        — multi-resolution Windows .exe icon (16→256)
#   assets/app_icon_256.png   — 256x256 PNG embedded in the binary (src/main.rs)
#   docs/images/icon.svg      — copy for the website
#
# Requires: sips, iconutil (both ship with macOS) and python3 with Pillow
# (for the Windows .ico — `pip3 install Pillow` if not present).

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
    "$PROJECT_DIR/assets/Colomin.ico" \
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

echo "Cleaning tiny macOS icon edge matte..."
python3 - <<PY
from pathlib import Path
from PIL import Image

ICONSET = Path("$ICONSET")

# The SVG renderer can leave very light RGB values in semi-transparent edge
# pixels. At Finder/sidebar sizes those pixels read as a silver rim, so clean
# the matte on the low-resolution macOS layers before iconutil packs them.
EDGE_FILES = {
    "icon_16x16.png": True,
    "icon_16x16@2x.png": True,
    "icon_32x32.png": True,
}
ALPHA_THRESHOLD = 96


def nearest_opaque_rgb(source, x, y):
    width, height = source.size
    for radius in range(1, max(width, height) + 1):
        best = None
        best_dist = None
        x0 = max(0, x - radius)
        x1 = min(width - 1, x + radius)
        y0 = max(0, y - radius)
        y1 = min(height - 1, y + radius)
        for yy in range(y0, y1 + 1):
            for xx in range(x0, x1 + 1):
                r, g, b, a = source.getpixel((xx, yy))
                if a != 255:
                    continue
                dist = (xx - x) * (xx - x) + (yy - y) * (yy - y)
                if best is None or dist < best_dist:
                    best = (r, g, b)
                    best_dist = dist
        if best is not None:
            return best
    return None


for filename, harden_alpha in EDGE_FILES.items():
    path = ICONSET / filename
    image = Image.open(path).convert("RGBA")
    source = image.copy()
    pixels = image.load()

    for y in range(image.height):
        for x in range(image.width):
            r, g, b, a = pixels[x, y]
            if not 0 < a < 255:
                continue
            replacement = nearest_opaque_rgb(source, x, y)
            if replacement is None:
                continue
            r, g, b = replacement
            if harden_alpha:
                a = 255 if a >= ALPHA_THRESHOLD else 0
            pixels[x, y] = (r, g, b, a)

    image.save(path)
PY

echo "Building Colomin.icns..."
iconutil -c icns "$ICONSET" -o "$PROJECT_DIR/assets/Colomin.icns"

echo "Writing assets/app_icon_256.png..."
cp -f "$ICONSET/icon_256x256.png" "$PROJECT_DIR/assets/app_icon_256.png"

# Build a multi-resolution Windows .ico for the .exe icon resource (embedded
# at link time by build.rs via the `winresource` crate). PIL packs all sizes
# into a single .ico — Explorer/taskbar picks whichever resolution fits.
echo "Building Colomin.ico (16, 24, 32, 48, 64, 128, 256)..."
python3 - <<PY
from PIL import Image
img = Image.open("$TMPDIR/icon_1024.png").convert("RGBA")
img.save(
    "$PROJECT_DIR/assets/Colomin.ico",
    format="ICO",
    sizes=[(16, 16), (24, 24), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
)
PY

echo "Copying SVG to docs/images/..."
mkdir -p "$PROJECT_DIR/docs/images"
cp -f "$SRC" "$PROJECT_DIR/docs/images/icon.svg"

echo ""
echo "Done. Updated:"
echo "  $PROJECT_DIR/assets/Colomin.icns"
echo "  $PROJECT_DIR/assets/Colomin.ico"
echo "  $PROJECT_DIR/assets/app_icon_256.png"
echo "  $PROJECT_DIR/docs/images/icon.svg"
