#!/bin/bash
set -euo pipefail

# Ensure cargo is on PATH
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="Colomin"
BUNDLE_DIR="$PROJECT_DIR/target/release/$APP_NAME.app"
BINARY_NAME="Colomin"

echo "Building release binary..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml"

echo "Creating app bundle..."
rm -rf "$BUNDLE_DIR"
mkdir -p "$BUNDLE_DIR/Contents/MacOS"
mkdir -p "$BUNDLE_DIR/Contents/Resources"

# Copy binary and normalize the packaged executable name to Colomin.
if [ -f "$PROJECT_DIR/target/release/colomin" ]; then
    SRC_BINARY="$PROJECT_DIR/target/release/colomin"
elif [ -f "$PROJECT_DIR/target/release/Colomin" ]; then
    SRC_BINARY="$PROJECT_DIR/target/release/Colomin"
else
    echo "Error: no built executable found in target/release"
    exit 1
fi
cp "$SRC_BINARY" "$BUNDLE_DIR/Contents/MacOS/$BINARY_NAME"

# Copy Info.plist
cp "$PROJECT_DIR/Info.plist" "$BUNDLE_DIR/Contents/"

# Convert icon.png to .icns
ICONSET_DIR=$(mktemp -d)/AppIcon.iconset
mkdir -p "$ICONSET_DIR"

for size in 16 32 64 128 256 512 1024; do
    cp "$PROJECT_DIR/icon.png" "$ICONSET_DIR/_tmp.png"
    sips --resampleHeightWidth "$size" "$size" "$ICONSET_DIR/_tmp.png" > /dev/null 2>&1
    case $size in
        16)   cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_16x16.png" ;;
        32)   cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_16x16@2x.png"
              cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_32x32.png" ;;
        64)   cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_32x32@2x.png" ;;
        128)  cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_128x128.png" ;;
        256)  cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_128x128@2x.png"
              cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_256x256.png" ;;
        512)  cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_256x256@2x.png"
              cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_512x512.png" ;;
        1024) cp "$ICONSET_DIR/_tmp.png" "$ICONSET_DIR/icon_512x512@2x.png" ;;
    esac
done
rm -f "$ICONSET_DIR/_tmp.png"

iconutil -c icns "$ICONSET_DIR" -o "$BUNDLE_DIR/Contents/Resources/AppIcon.icns"
rm -rf "$(dirname "$ICONSET_DIR")"

# Copy themes if present
if [ -d "$PROJECT_DIR/themes" ]; then
    cp -r "$PROJECT_DIR/themes" "$BUNDLE_DIR/Contents/Resources/"
fi

# Copy assets if present
if [ -d "$PROJECT_DIR/assets" ]; then
    cp -r "$PROJECT_DIR/assets" "$BUNDLE_DIR/Contents/Resources/"
fi

echo ""
echo "Done: $BUNDLE_DIR"

# Refresh Launch Services so Finder picks up the updated bundle
# (document types, icon, executable). Without this, macOS can get
# stuck on an older cached version of the app.
LSREG="/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister"
if [ -x "$LSREG" ]; then
    "$LSREG" -f "$BUNDLE_DIR" >/dev/null 2>&1 && echo "Registered with Launch Services."
fi

echo "Run with: open \"$BUNDLE_DIR\""
