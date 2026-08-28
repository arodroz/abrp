#!/usr/bin/env bash
# Builds the `planner_ffi` static xcframework and its generated Swift
# bindings from the `ffi` crate (wayfinder #34, ADR 0004). Mac-only.
#
# Produces:
#   app/PlannerKit/artifacts/planner_ffi.xcframework  (gitignored)
#   app/PlannerKit/Sources/PlannerKit/Planner.swift    (committed)
#
# Three slices: iOS device, iOS simulator, and host macOS -- the macOS slice
# isn't shipped in the app, but it's what lets `swift test` run the golden
# test (Part 4) on this Mac without a simulator/device round-trip.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

LIB_NAME="planner_ffi"
DEVICE_TARGET="aarch64-apple-ios"
SIM_TARGET="aarch64-apple-ios-sim"
HOST_TARGET="aarch64-apple-darwin"
PROFILE="ffi-release"

PLANNER_KIT_DIR="$ROOT_DIR/app/PlannerKit"
ARTIFACTS_DIR="$PLANNER_KIT_DIR/artifacts"
SOURCES_DIR="$PLANNER_KIT_DIR/Sources/PlannerKit"
GEN_DIR="$ROOT_DIR/target/ffi-xcframework-gen"
XCFRAMEWORK_OUT="$ARTIFACTS_DIR/$LIB_NAME.xcframework"

step() { printf '\n\033[1;34m== %s\033[0m\n' "$*"; }

step "1. Ensure Rust targets"
for target in "$DEVICE_TARGET" "$SIM_TARGET" "$HOST_TARGET"; do
  rustup target list --installed | grep -qx "$target" || rustup target add "$target"
done

step "2. Build the static libs (device, simulator, host) at profile $PROFILE"
cargo build -p ffi --profile "$PROFILE" --target "$DEVICE_TARGET"
cargo build -p ffi --profile "$PROFILE" --target "$SIM_TARGET"
cargo build -p ffi --profile "$PROFILE" --target "$HOST_TARGET"

DEVICE_LIB="$ROOT_DIR/target/$DEVICE_TARGET/$PROFILE/lib$LIB_NAME.a"
SIM_LIB="$ROOT_DIR/target/$SIM_TARGET/$PROFILE/lib$LIB_NAME.a"
HOST_LIB="$ROOT_DIR/target/$HOST_TARGET/$PROFILE/lib$LIB_NAME.a"
HOST_DYLIB="$ROOT_DIR/target/$HOST_TARGET/$PROFILE/lib$LIB_NAME.dylib"

step "3. Generate Swift bindings (library mode, ADR 0004 point 1) from the host dylib"
rm -rf "$GEN_DIR"
mkdir -p "$GEN_DIR"
cargo run -p ffi --bin uniffi-bindgen --features cli -- \
  generate --library "$HOST_DYLIB" --language swift --out-dir "$GEN_DIR"

GEN_SWIFT="$GEN_DIR/$LIB_NAME.swift"
GEN_HEADER="$GEN_DIR/${LIB_NAME}FFI.h"
GEN_MODULEMAP="$GEN_DIR/${LIB_NAME}FFI.modulemap"
for f in "$GEN_SWIFT" "$GEN_HEADER" "$GEN_MODULEMAP"; do
  [ -f "$f" ] || { echo "expected generated file missing: $f" >&2; exit 1; }
done

step "4. Assemble per-slice header dirs (module.modulemap is a fixed name xcodebuild requires)"
for slice in device sim host; do
  headers_dir="$GEN_DIR/headers-$slice"
  rm -rf "$headers_dir"
  mkdir -p "$headers_dir"
  cp "$GEN_HEADER" "$headers_dir/"
  cp "$GEN_MODULEMAP" "$headers_dir/module.modulemap"
done

step "5. Assemble the xcframework"
mkdir -p "$ARTIFACTS_DIR"
rm -rf "$XCFRAMEWORK_OUT"
xcodebuild -create-xcframework \
  -library "$DEVICE_LIB" -headers "$GEN_DIR/headers-device" \
  -library "$SIM_LIB" -headers "$GEN_DIR/headers-sim" \
  -library "$HOST_LIB" -headers "$GEN_DIR/headers-host" \
  -output "$XCFRAMEWORK_OUT"

step "6. Copy the generated Swift bindings into the SwiftPM target"
mkdir -p "$SOURCES_DIR"
cp "$GEN_SWIFT" "$SOURCES_DIR/Planner.swift"

step "Done"
echo "xcframework: $XCFRAMEWORK_OUT"
echo "bindings:    $SOURCES_DIR/Planner.swift"
