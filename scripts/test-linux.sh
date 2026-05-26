#!/bin/bash
set -euo pipefail

# Reproduce the GitHub Actions Linux build locally inside an Ubuntu 22.04
# container. Produces .deb + .AppImage in ./dist, then installs the .deb
# inside the container to verify dependencies resolve and the binary runs.
#
# Requires: Docker (Desktop / OrbStack / colima).
#
# Usage: ./scripts/test-linux.sh
#
# Persistence: cargo and apt caches live in named volumes (colomin-linux-cargo,
# colomin-linux-apt) so reruns finish in ~30s once everything is warmed up.

PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
IMAGE="ubuntu:22.04"
CARGO_VOL="colomin-linux-cargo"
TARGET_VOL="colomin-linux-target"

echo "── Building Linux artifacts in $IMAGE ─────────────────────────────────"

# Linux container script. Heredoc so quoting stays simple.
docker run --rm -i \
    -v "$PROJECT_DIR":/work \
    -v "$CARGO_VOL":/root/.cargo \
    -v "$TARGET_VOL":/work/target \
    -w /work \
    -e CARGO_TERM_COLOR=always \
    -e APPIMAGE_EXTRACT_AND_RUN=1 \
    "$IMAGE" bash <<'EOF'
set -euo pipefail

# ── System deps (mirrors .github/workflows/release.yml build-linux job) ─────
apt-get update -qq
DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    ca-certificates curl build-essential pkg-config \
    libgtk-3-dev libxkbcommon-dev libssl-dev libfontconfig1-dev \
    libwayland-dev libxcb1-dev libx11-dev \
    file fuse libfuse2 desktop-file-utils \
    >/dev/null

# ── Rust (idempotent: rustup-init bails fast if already present) ───────────
if ! command -v cargo >/dev/null 2>&1; then
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --default-toolchain stable --profile minimal --no-modify-path
fi
export PATH="/root/.cargo/bin:$PATH"

# ── cargo-packager (cached after first run) ────────────────────────────────
if ! command -v cargo-packager >/dev/null 2>&1; then
    cargo install cargo-packager --locked
fi

echo
echo "── Building release binary ──"
cargo build --release

echo
echo "── Packaging .deb + .AppImage ──"
cargo packager --release --formats deb,appimage

echo
echo "── Artifacts ──"
ls -la dist/

echo
echo "── Installing .deb to verify deps resolve ──"
apt-get install -y ./dist/*.deb >/dev/null
echo "Installed colomin to: $(which colomin)"
# Expected: binary launches and panics on missing DISPLAY/WAYLAND.
# That proves all the GTK/X11/Wayland .so deps linked — anything else is a real failure.
# Capture-then-grep avoids pipefail propagating the SIGABRT from the panic.
DEB_OUT="$(colomin 2>&1 || true)"
if echo "$DEB_OUT" | grep -qE "(WAYLAND_DISPLAY|DISPLAY is set)"; then
    echo ".deb binary: linked + launches as expected (no display in container)"
else
    echo "::error::.deb binary: unexpected failure mode"
    echo "$DEB_OUT" | head -10
    exit 1
fi

echo
echo "── AppImage smoke test (extract + run) ──"
chmod +x dist/*.AppImage
APPIMG_OUT="$(APPIMAGE_EXTRACT_AND_RUN=1 dist/*.AppImage 2>&1 || true)"
if echo "$APPIMG_OUT" | grep -qE "(WAYLAND_DISPLAY|DISPLAY is set)"; then
    echo "AppImage: extracts + binary launches as expected"
else
    echo "::error::AppImage: unexpected failure mode"
    echo "$APPIMG_OUT" | head -20
    exit 1
fi

echo
echo "── All Linux artifacts verified ──"
EOF

echo
echo "Local build complete. Artifacts on host:"
ls -la "$PROJECT_DIR/dist/"
