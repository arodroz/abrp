#!/usr/bin/env bash
# Bootstrap the macOS build environment for the vertical slice (wayfinder #24).
# Idempotent. Run:  bash scripts/bootstrap-mac.sh   then paste the printed report into issue #24.
set -euo pipefail
UNIFFI_VER="${UNIFFI_VER:-0.32}"        # pinned line per ADR 0004
DATA_DIR="${DATA_DIR:-$HOME/abrp-data}"

step() { printf '\n\033[1;34m== %s\033[0m\n' "$*"; }

step "1. Xcode + device"
xcode-select -p >/dev/null 2>&1 || { echo "Install Xcode 16+ from the App Store, then: sudo xcode-select -s /Applications/Xcode.app && sudo xcodebuild -license accept"; exit 1; }
xcodebuild -version
xcrun devicectl list devices 2>/dev/null || echo "(no paired device yet — plug in the iPhone, trust the Mac, enable Developer Mode in Settings > Privacy & Security)"

step "2. Rust toolchain + iOS targets + UniFFI"
command -v rustup >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup update stable
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo install --locked uniffi_bindgen_cli 2>/dev/null || cargo install --locked --version "^$UNIFFI_VER" uniffi-bindgen-cli 2>/dev/null || echo "NOTE: install uniffi-bindgen as a binary target in the Rust crate (library mode, ADR 0004) — the standalone CLI crate name varies by version"
cargo install --locked cargo-swift || true

step "3. Homebrew tools for the pipeline"
command -v brew >/dev/null || { echo "Install Homebrew first: https://brew.sh"; exit 1; }
brew install osmium-tool pmtiles awscli jq

step "4. Data for the first Region Pack / Map Pack"
mkdir -p "$DATA_DIR" && cd "$DATA_DIR"
for c in luxembourg belgium; do
  [ -f "$c-latest.osm.pbf" ] || curl -sSLO "https://download.geofabrik.de/europe/$c-latest.osm.pbf"
done
# Copernicus GLO-30 terrarium tiles, one probe request to prove keyless access:
aws s3 ls --no-sign-request s3://elevation-tiles-prod/terrarium/9/263/173.png && echo "GLO-30 tiles reachable"
ls -lh "$DATA_DIR"

step "5. Report (paste into issue #24 and docs/research/dev-environment.md)"
cat <<REPORT
machine:      $(sysctl -n hw.model) / $(sysctl -n machdep.cpu.brand_string) / $(sysctl -n hw.memsize | awk '{printf "%d GB", $1/1073741824}')
macOS:        $(sw_vers -productVersion)
Xcode:        $(xcodebuild -version | tr '\n' ' ')
rustc:        $(rustc --version)
cargo:        $(cargo --version)
targets:      $(rustup target list --installed | tr '\n' ' ')
uniffi:       $(ls ~/.cargo/bin | grep -i uniffi | tr '\n' ' ')
osmium:       $(osmium --version | head -1)
pmtiles:      $(pmtiles --version 2>&1 | head -1)
aws:          $(aws --version)
device:       (fill in: model, iOS version — from Xcode > Window > Devices and Simulators)
maplibre:     (fill in after step 6: ios-v6.29 blank app, fps observed with CADisableMinimumFrameDuration=YES)
REPORT
echo
echo "6. Manual: File > New iOS App, add package https://github.com/maplibre/maplibre-gl-native-distribution (6.29.x), set CADisableMinimumFrameDuration=YES in Info.plist, run on the iPhone, read fps in Xcode's FPS gauge / Instruments Core Animation."
