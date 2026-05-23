#!/bin/bash
set -euo pipefail

# Submits an artifact to Apple's notary service and staples the ticket.
# Accepts .dmg, .pkg, .zip directly; for .app inputs, zips into a temp
# archive first (notarytool does not accept raw .app bundles).
#
# Usage:  ./scripts/notarize.sh <path>
# Env:    APPLE_API_KEY_PATH    Path to the .p8 App Store Connect API key
#         APPLE_API_KEY_ID      10-char API key ID
#         APPLE_API_ISSUER_ID   UUID issuer ID

if [ $# -ne 1 ]; then
    echo "Usage: $0 <path-to-.app-or-.dmg-or-.pkg-or-.zip>" >&2
    exit 1
fi
TARGET="$1"
: "${APPLE_API_KEY_PATH:?APPLE_API_KEY_PATH env var is required}"
: "${APPLE_API_KEY_ID:?APPLE_API_KEY_ID env var is required}"
: "${APPLE_API_ISSUER_ID:?APPLE_API_ISSUER_ID env var is required}"

if [ ! -e "$TARGET" ]; then
    echo "Error: $TARGET does not exist." >&2
    exit 1
fi

# notarytool accepts .dmg/.pkg/.zip. For .app inputs, zip first using ditto
# (preserves bundle structure and resource forks).
SUBMIT_PATH="$TARGET"
TMP_ZIP=""
case "$TARGET" in
    *.app)
        TMP_ZIP="$(mktemp -d)/$(basename "${TARGET%.app}").zip"
        echo "Zipping $TARGET → $TMP_ZIP"
        ditto -c -k --keepParent "$TARGET" "$TMP_ZIP"
        SUBMIT_PATH="$TMP_ZIP"
        ;;
esac

echo "Submitting $SUBMIT_PATH to Apple notary service..."
xcrun notarytool submit "$SUBMIT_PATH" \
    --key "$APPLE_API_KEY_PATH" \
    --key-id "$APPLE_API_KEY_ID" \
    --issuer "$APPLE_API_ISSUER_ID" \
    --wait \
    --timeout 30m

# Staple the ticket to the ORIGINAL target (not the zip), so it's attached
# to the .app/.dmg that users actually see.
echo "Stapling ticket to $TARGET..."
xcrun stapler staple "$TARGET"
xcrun stapler validate "$TARGET"

if [ -n "$TMP_ZIP" ]; then
    rm -rf "$(dirname "$TMP_ZIP")"
fi

echo "Notarization complete."
