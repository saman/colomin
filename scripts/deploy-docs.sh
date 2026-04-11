#!/usr/bin/env bash

set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TEMP_DIR="$(mktemp -d /tmp/colomin-docs.XXXXXX)"
BRANCH_NAME="${1:-docs}"
REMOTE_NAME="${2:-origin}"
REMOTE_URL="$(git -C "$ROOT_DIR" remote get-url "$REMOTE_NAME")"

cleanup() {
  rm -rf "$TEMP_DIR"
}

trap cleanup EXIT

if [[ ! -d "$ROOT_DIR/docs" ]]; then
  echo "docs/ directory not found"
  exit 1
fi

echo "Preparing temporary publish worktree in $TEMP_DIR"
git -C "$ROOT_DIR" init -q "$TEMP_DIR"
git -C "$TEMP_DIR" checkout --orphan "$BRANCH_NAME" >/dev/null 2>&1 || git -C "$TEMP_DIR" checkout "$BRANCH_NAME"

find "$TEMP_DIR" -mindepth 1 -maxdepth 1 \
  ! -name '.git' \
  -exec rm -rf {} +

cp -R "$ROOT_DIR"/docs/. "$TEMP_DIR"/

git -C "$TEMP_DIR" add .

if git -C "$TEMP_DIR" diff --cached --quiet; then
  echo "No docs changes to publish"
  exit 0
fi

git -C "$TEMP_DIR" -c user.name="Codex" -c user.email="codex@openai.com" \
  commit -m "Deploy docs"

echo "Pushing site contents to $REMOTE_NAME/$BRANCH_NAME"
git -C "$TEMP_DIR" remote add "$REMOTE_NAME" "$REMOTE_URL"
git -C "$TEMP_DIR" push --force "$REMOTE_NAME" "HEAD:refs/heads/$BRANCH_NAME"

echo "Docs published to $REMOTE_NAME/$BRANCH_NAME"
