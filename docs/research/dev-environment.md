# Development environment (wayfinder #24)

Filled in by running `bash scripts/bootstrap-mac.sh` on the build Mac and pasting its report.

## Build machine

| Item | Value |
|---|---|
| Machine | _pending_ |
| macOS | _pending_ |
| Xcode | _pending_ (≥ 16) |
| rustc / cargo | _pending_ |
| Rust targets | `aarch64-apple-ios`, `aarch64-apple-ios-sim` |
| UniFFI | 0.32.x (ADR 0004) — _exact version pending_ |
| MapLibre Native | `maplibre-gl-native-distribution` ios-v6.29 — _fps on blank app pending_ |
| osmium / pmtiles / aws | _pending_ |

## Test device

| Item | Value |
|---|---|
| iPhone model | _pending_ (must be ProMotion: 13 Pro or newer) |
| iOS version | _pending_ |
| Developer Mode | enabled |

## Data on disk (`$DATA_DIR`, default `~/abrp-data`)

- `luxembourg-latest.osm.pbf`, `belgium-latest.osm.pbf` from Geofabrik
- GLO-30 terrarium tiles reachable keyless via `aws s3 --no-sign-request s3://elevation-tiles-prod/terrarium/`

## Non-goals of this environment

The Windows box (`C:\Users\antonio\Downloads\abrp`) stays the pipeline/docs machine; it has Python 3.13 and the `pmtiles` CLI only.
