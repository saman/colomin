#!/bin/bash
set -euo pipefail

# Cuts a release: bumps Cargo.toml version, commits, tags, and pushes.
# GitHub Actions (.github/workflows/release.yml) picks up the tag, builds
# the universal DMG, and publishes a GitHub Release with auto-generated
# notes from PRs/commits since the previous tag.
#
# Usage: ./scripts/release.sh <version>
# Example: ./scripts/release.sh 0.2.0

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$PROJECT_DIR"

if [ $# -ne 1 ]; then
    echo "Usage: $0 <version>"
    echo "Example: $0 0.2.0"
    exit 1
fi

VERSION="$1"
TAG="v$VERSION"

if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]]; then
    echo "Error: version '$VERSION' is not a valid semver (e.g. 0.2.0 or 0.2.0-rc1)."
    exit 1
fi

CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
if [ "$CURRENT_BRANCH" != "main" ]; then
    echo "Error: must be on 'main' branch (currently on '$CURRENT_BRANCH')."
    exit 1
fi

if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "Error: working tree has uncommitted changes. Commit or stash first."
    git status --short
    exit 1
fi

if git rev-parse "$TAG" >/dev/null 2>&1; then
    echo "Error: tag $TAG already exists locally. Delete it first if you mean to recut:"
    echo "  git tag -d $TAG && git push origin :refs/tags/$TAG"
    exit 1
fi

CURRENT_VERSION=$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)"/\1/')
echo "Current Cargo.toml version: $CURRENT_VERSION"
echo "New version:                $VERSION"
echo "Tag:                        $TAG"
echo
read -r -p "Proceed? [y/N] " ANSWER
case "$ANSWER" in
    y|Y|yes|YES) ;;
    *) echo "Aborted."; exit 1 ;;
esac

# Bump both `version = "..."` lines in Cargo.toml (the [package] one and the
# [package.metadata.bundle] one). BSD sed (macOS) needs the '' after -i.
sed -i '' -E "s/^version = \"[^\"]+\"/version = \"$VERSION\"/" Cargo.toml

# Refresh Cargo.lock so the version bump is recorded.
cargo update --workspace --offline >/dev/null 2>&1 || cargo check --quiet

git add Cargo.toml Cargo.lock
git commit -m "Release $TAG"
git tag "$TAG"

echo
echo "Pushing main and $TAG to origin..."
git push origin main
git push origin "$TAG"

echo
echo "Done. Watch the build:"
echo "  gh run watch"
echo "Or open: https://github.com/saman/colomin/actions"
