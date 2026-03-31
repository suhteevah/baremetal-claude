#!/usr/bin/env bash
# ClaudioOS build script — compiles kernel + creates bootable disk images
set -euo pipefail

PROFILE="${1:-debug}"
BUILD_FLAGS=""
if [ "$PROFILE" = "release" ]; then
    BUILD_FLAGS="--release"
fi

TARGET="x86_64-unknown-none"
KERNEL_BIN="target/$TARGET/$PROFILE/claudio-os"

echo "=== ClaudioOS Build ==="
echo "[1/3] Compiling kernel ($PROFILE)..."
RUSTC_WRAPPER="" cargo build $BUILD_FLAGS 2>&1

echo "[2/3] Building disk image tool..."
# Build the host-side image builder from its own directory.
# Must override build-std since the parent .cargo/config.toml sets it for bare-metal.
pushd tools/image-builder > /dev/null
RUSTC_WRAPPER="" cargo +nightly build \
    -Z "build-std=" \
    --target x86_64-pc-windows-msvc \
    2>&1
popd > /dev/null

IMAGE_BUILDER="tools/image-builder/target/x86_64-pc-windows-msvc/debug/claudio-image-builder.exe"

echo "[3/3] Creating bootable disk images..."
"$IMAGE_BUILDER" "$KERNEL_BIN"

echo ""
echo "=== Done! ==="
echo "Boot with: qemu-system-x86_64 -drive format=raw,file=target/$TARGET/$PROFILE/claudio-os-bios.img -serial stdio -m 512M"
