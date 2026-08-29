# Wayfinder (ABRP-native)

A native iOS EV route planner for a Hyundai Ioniq 5, built from open data and
public facts only: on-device routing, energy modelling, and Charging Stop
optimisation over installable offline packs. No backend, no accounts.

The spec of record is `docs/adr/` (ADR 0001–0011) plus the glossary in
`CONTEXT.md`; progress is tracked as GitHub issues (build map: #28).

## Layout

| Path | What it is |
|---|---|
| `core/packs` | `.rpack` Region Pack format: zero-copy mmap reader, model types |
| `core/routing` | Contraction-hierarchy routing kernel (bidirectional p2p, bucket many-to-many) |
| `core/energy` | Hybrid physics energy model + Charging Curve integration (ADR 0003) |
| `core/optimiser` | Corridor assembly + Charging Stop label-setting search (ADR 0006/0010) |
| `core/ffi` | UniFFI boundary: one `Planner` object exposed to Swift (ADR 0004) |
| `core/fixtures` | Checked-in test data (e.g. the `tlog-1` Trip Log fixture) |
| `pipeline` | Mac-side pack builders: OSM import, CH prepare, elevation, chargers, catalog |
| `app/PlannerKit` | SwiftPM package wrapping the generated bindings + xcframework |
| `app/Wayfinder` | The iOS app (SwiftUI + MapLibre); see `app/README.md` |
| `scripts` | Mac bootstrap, xcframework build, pack publishing |
| `docs/adr`, `docs/research` | Decisions and measured gate results |

Data flows one way: raw feeds → `pipeline` → versioned pack artifacts +
catalog → object storage (ADR 0011) → in-app installer → Documents →
`Planner` → UI.

## Prerequisites

- Rust (pinned by `rust-toolchain.toml`; rustup resolves it automatically)
- Xcode with the iOS SDK, [xcodegen](https://github.com/yonaskolb/XcodeGen)
- `scripts/bootstrap-mac.sh` installs the pipeline's native deps
- Pack building needs raw data under `~/abrp-data/` (see `pipeline/`); the app
  needs either sideloaded packs or the hosted catalog (tailnet-only)

## Build and test

```sh
cargo test --workspace --locked          # Rust unit + format tests (CI)
cargo clippy --workspace --all-targets --locked -- -D warnings
scripts/build-xcframework.sh             # regenerate bindings + xcframework after core/ changes
```

App build, simulator autotests, and pack sideloading are documented in
`app/README.md`. Real-pack golden tests and the release performance gate run
locally against `~/abrp-data` (marked `#[ignore]`); on-device numbers are
recorded per milestone in `docs/research/`.

## License

MIT (see `LICENSE`). Data sources and their attributions are listed in
`NOTICE.md`.
