# 4. The Rust boundary: UniFFI, one coarse Planner object, Swift owns the OS

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/ilres-antonio/abrp/issues/13

## Context

ADRs 0001–0003 place routing, energy and stop search in Rust. Research (#8, `docs/research/rust-swift-integration.md`) compared UniFFI, swift-bridge and a hand-written C ABI: all produce the same static XCFramework; UniFFI (Mozilla, Ferrostar) is mainstream and idiomatic but serialises every record through a `RustBuffer`, lacks cancellation and has an open Swift 6 strict-concurrency issue for async; swift-bridge is single-maintainer; the C ABI is maximal control and boilerplate.

## Decision

1. **Mechanism: UniFFI (≥ 0.32) in library mode**, proc-macro exports, static XCFramework as a SwiftPM binary target (local `path:` during development). The C-layer contract is kept so a hot path may drop to `extern "C"` later without changing generators.
2. **Rust owns** (pure CPU, no I/O, no async runtime): Routing Engine, Energy Model, Charging Curve maths, Charging Stop search, Region Pack parsing and mmap, Charger-feed parsing/normalisation into the Charger index.
   **Swift owns**: CoreLocation, URLSession (Charger/weather/tile fetches, pack downloads), MapLibre rendering, SwiftUI state, user-settings persistence.
3. **Call shape**: one `Planner` (`uniffi::Object`, interior mutability, `Send + Sync`) constructed once with Region Pack **file paths** (Rust mmaps them), exposing `plan(PlanRequest) throws -> Plan`, `energy(LegInput, VehicleModel) -> LegEnergy`, `loadChargers(bytes, format)`, and `cancel()`. `Plan`, `Leg`, `ChargingStop`, `Charger`, `VehicleModel` are immutable records; the SoC curve is one `[Float]`. Never per-edge or per-point calls.
4. **Concurrency**: synchronous Rust invoked from a Swift background `Task.detached`, results published on main; cancellation via an atomic flag polled in the search loop. No UniFFI async. The app adopts **Swift 6 strict concurrency**; the UniFFI module is isolated behind a `Sendable` wrapper and the prototype verifies the generated code compiles under it.
5. **Errors**: one `#[derive(uniffi::Error)]` enum (`NoRouteFound`, `InsufficientRange`, `Cancelled`, `PackMissing`, `InvalidRequest`, …) surfacing as Swift `throws`. `panic = "abort"`; a panic is a bug.
6. **Build profile**: `opt-level = "s"`, `lto = "fat"`, `codegen-units = 1`, `strip = true`; no tokio/reqwest in the core crate; UniFFI version pinned in `Cargo.toml` and the bindgen invocation.
7. **Testing**: Rust `cargo test` unit + property tests on a Luxembourg fixture graph, the ±5 % energy targets (ADR 0003), and golden-Plan snapshot tests pinned to a Region Pack checksum; Swift XCTest on the wrapper with a stub `Planner`; on-device XCUITest only for the prototype's performance measurements.

## Consequences

- The Region Pack format (#16) must be mmap-friendly and addressed by path; Charger feeds arrive in Rust as raw bytes plus a format tag.
- UI what-ifs (drag a waypoint, change max speed) re-enter through `plan`/`energy` — the boundary is chatty only at Plan granularity, so `RustBuffer` overhead is bounded per user gesture.
- Reversal: switching generator keeps the crate and the C layer; moving logic across the boundary (e.g. networking into Rust) would add a runtime and is deliberately avoided.
