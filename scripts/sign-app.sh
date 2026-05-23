#!/bin/bash
set -euo pipefail

# Codesigns a macOS bundle (.app) or archive (.dmg) with the active
# Developer ID identity. Uses hardened runtime + secure timestamp,
# which are required for notarization.
#
# The signing identity must already be present in the active keychain
# (see the "Set up signing keychain" step in .github/workflows/release.yml).
#
# Usage:   ./scripts/sign-app.sh <path>
# Env:     SIGNING_IDENTITY    Full identity name as shown by
#                              `security find-identity -v -p codesigning`,
#                              e.g. "Developer ID Application: Foo Bar (TEAMID)"

if [ $# -ne 1 ]; then
    echo "Usage: $0 <path-to-.app-or-.dmg>" >&2
    exit 1
fi
TARGET="$1"
: "${SIGNING_IDENTITY:?SIGNING_IDENTITY env var is required}"

if [ ! -e "$TARGET" ]; then
    echo "Error: $TARGET does not exist." >&2
    exit 1
fi

echo "Signing $TARGET"
echo "  identity: $SIGNING_IDENTITY"

case "$TARGET" in
    *.app)
        # --deep covers nested bundles; --options runtime enables the hardened
        # runtime (required by notarytool); --timestamp uses Apple's TSA.
        codesign --force --deep \
            --options runtime \
            --timestamp \
            --sign "$SIGNING_IDENTITY" \
            "$TARGET"
        codesign --verify --strict --deep --verbose=2 "$TARGET"
        ;;
    *.dmg|*.pkg|*.zip)
        codesign --force --timestamp --sign "$SIGNING_IDENTITY" "$TARGET"
        codesign --verify --verbose=2 "$TARGET"
        ;;
    *)
        echo "Error: unsupported target type for $TARGET" >&2
        exit 1
        ;;
esac

echo "Done."
