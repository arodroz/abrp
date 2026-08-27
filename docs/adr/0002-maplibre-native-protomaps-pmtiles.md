# 2. MapLibre Native (Metal) with self-hosted Protomaps PMTiles

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/11

## Context

The planner needs Google-Maps-class map feel: custom vector styling, thousands of Chargers, a multi-Leg polyline coloured by SoC, 120 Hz on ProMotion, and an offline basemap consistent with ADR 0001's offline Plan. Research (#4, `docs/research/map-rendering-ios.md`) found Apple MapKit cannot take custom vector tiles, has no offline basemap and no frame-rate API; Google's SDK caps at 60 fps; MapLibre Native iOS (Metal since 6.0, now 6.29, BSD-2) satisfies every requirement but has only a community SwiftUI layer. Hosted tile providers either forbid bulk download (MapTiler, ABRP's own supplier) or cap offline caching (Stadia ≤ 100 MB); Protomaps PMTiles is a single static file that serves online via range requests and offline via download.

## Decision

1. **Map engine: MapLibre Native iOS** (Metal), wrapped in our own `UIViewRepresentable`; `maplibre/swiftui-dsl` optional.
2. **Tiles: self-hosted Protomaps PMTiles**, extracted for Benelux+FR+DE and served as static files from the same bucket as the Region Packs (no backend). Protomaps API is the zero-setup fallback for the prototype.
3. **Offline basemap is a target requirement**: a per-region **Map Pack** (PMTiles extract) downloaded alongside the Region Pack. The prototype uses online tiles + MapLibre's ambient cache only.
4. **Style**: a Protomaps basemap flavor as-is (light/dark); custom styling is later polish.
5. **Rendering rules** (protect the frame budget): Legs are data-driven line layers; Chargers are a clustered symbol/circle layer from an `MLNShapeSource`; `MLNAnnotationView` (UIView) is used only for the selected Charging Stop's callout, never per Charger.
6. **Acceptance bar for the prototype (#15)**: sustained 120 fps on a ProMotion iPhone during pan/zoom with ~10 k clustered Chargers and one multi-Leg polyline, `CADisableMinimumFrameDuration` set, no frame > 16 ms during a re-plan; measured with MapLibre's rendering-statistics HUD.

## Consequences

- Attribution: "© OpenStreetMap" (ODbL) on-map; Protomaps styles are BSD. No OpenMapTiles credit needed.
- We own a UIKit wrapper and its SwiftUI state bridge; MapKit's free first-party SwiftUI `Map` is given up.
- Map Pack sizes (maxzoom 14 vs 15) are unknown and must be measured before the pack design is final.
- Reversal cost: high — style, layers, offline packs and the Charger rendering path are engine-specific.
