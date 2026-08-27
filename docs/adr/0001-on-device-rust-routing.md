# 1. Routing Engine runs on-device in Rust over precomputed Region Packs

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/ilres-antonio/abrp/issues/10

## Context

The planner must feel Google-Maps-class (first Plan < 1 s, re-route < 300 ms) and this is a learning project with no revenue to fund per-request APIs. Research (#3, `docs/research/on-device-routing.md`) showed CH queries are sub-ms at continental scale, a turn-aware CH for Benelux+FR+DE (~1.2 GiB) fits an iPhone via mmap, and no Rust crate goes OSM → CH end-to-end. Alternatives were a self-hosted Valhalla (16–32 GB-RAM server, network-bound Plans) and third-party EV APIs (HERE, Mapbox preview, Iternio: $0.75–2 per 1k requests, server-side Plan, Iternio contradicts the "open sources only" stance).

## Decision

1. The Routing Engine runs **on the phone, in Rust**, as a contraction hierarchy over a road graph precomputed off-device.
2. Graphs are built by a **batch pipeline** (developer machine / CI) and published as static **Region Packs**, one per country, mirroring Geofabrik extracts. **No always-on backend** in the MVP; Charger and weather data are fetched by the phone directly from their providers.
3. First slice: node-based CH (**turn restrictions ignored**), Luxembourg + Belgium packs. Turn-aware routing is a known defect to fix before any navigation feature.
4. The Routing Engine optimises **travel time**; energy is evaluated afterwards per Leg by the Energy Model. Energy-as-weight (CCH re-customisation) stays possible later at the kernel level.
5. Per-edge speed for v1 is **static**: OSM `maxspeed`, else road-class/country defaults. No traffic feed.
6. **Offline is a hard requirement**: a Plan must compute with no connectivity using the installed Region Packs and the last cached Charger data; weather degrades to a default.
7. Acceptance bar for the vertical-slice prototype (#15): first Plan < 1 s warm, Leg re-route < 300 ms, cold start to first Plan < 3 s, resident graph < 1 GB (with the turn-aware ×2–3 penalty estimated).

## Consequences

- Highest engineering effort of the three options: OSM importer, speed model, elevation join, serialisation, mmap loading, later a turn-expanded graph — all ours.
- App storage grows by 1–2 GB for the first slice; whole Europe (5–8 GB) needs per-region downloads.
- Fixes the Rust boundary's core: routing, energy and stop search live in Rust; Swift never sees the graph.
- Reversal cost: switching to a server engine later would discard the pipeline and pack format but keep the Plan optimiser and Energy Model.
