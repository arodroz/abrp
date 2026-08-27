# Development environment (wayfinder #24)

Filled in by running `bash scripts/bootstrap-mac.sh` on the build Mac and pasting its report.

## Build machine

| Item | Value |
|---|---|
| Machine | Mac16,10 / Apple M4 / 32 GB |
| macOS | 26.5.2 |
| Xcode | 26.6 (17F113) |
| rustc / cargo | 1.98.0 / 1.98.0 |
| Rust targets | `aarch64-apple-ios`, `aarch64-apple-ios-sim` (+ host, `x86_64-apple-ios`) |
| UniFFI | 0.32.x (ADR 0004) — no standalone CLI; runs as a bin target in the Rust crate (script's documented fallback). `cargo-swift` 0.11.1 installed. |
| MapLibre Native | `maplibre-gl-native-distribution` ios-v6.29 — _fps on blank app pending_ |
| osmium / pmtiles / aws | osmium 1.19.1 / pmtiles 1.31.2 / aws-cli 2.36.32 |

## Test device

| Item | Value |
|---|---|
| iPhone model | iPhone 15 Pro (iPhone16,1) — ProMotion ✓ |
| iOS version | 26.6 |
| Developer Mode | enabled |

## Data on disk (`$DATA_DIR`, default `~/abrp-data`)

- `luxembourg-latest.osm.pbf`, `belgium-latest.osm.pbf` from Geofabrik
- GLO-30 terrarium tiles reachable keyless via `aws s3 --no-sign-request s3://elevation-tiles-prod/terrarium/`

## Non-goals of this environment

The Windows box (`C:\Users\antonio\Downloads\abrp`) stays the pipeline/docs machine; it has Python 3.13 and the `pmtiles` CLI only.
