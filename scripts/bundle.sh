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

# Copy the prebuilt .icns directly (same approach as the Tauri build).
cp "$PROJECT_DIR/assets/Colomin.icns" "$BUNDLE_DIR/Contents/Resources/icon.icns"

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
