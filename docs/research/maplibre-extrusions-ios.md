# Extruded buildings on MapLibre Native iOS 6.29 (Metal) for Drive Mode

Research for wayfinder #89 (map #86). Answers the ticket's five questions for the shipped stack: MapLibre Native iOS 6.29.0 via SPM (Metal), Protomaps basemap 4.15.2 PMTiles capped at z14, flat `buildings` fill in `pipeline/assets/styles/style-light.json`, route drawn app-side in `app/Wayfinder/Sources/RouteLayer.swift`, puck as an `MLNAnnotationImage`. Every claim carries the source it was read from; facts checked 2026-09-17. Header quotes marked "shipped header" were read from the 6.29.0 xcframework in the local SPM checkout, not from `main`.

Terms follow `CONTEXT.md`: *Drive Mode*, *Go*, *Map Pack*, *Plan*, *Leg*.

## 0. What the tiles actually give us

- The Protomaps `buildings` source-layer carries `kind` (`address` | `building` | `building_part`), `height`, `min_height` and `layer`; `height` and `min_height` "May be quantized at low zoom levels". Below z15 the layer "contains merged buildings, even disconnected ones"; from z15 it holds "individual OSM equivalent buildings". Source: [Protomaps basemap layers doc](https://docs.protomaps.com/basemaps/layers).
- Tile builder facts ([`Buildings.java`](https://github.com/protomaps/basemaps/blob/main/tiles/src/main/java/com/protomaps/basemap/layers/Buildings.java)):
  - Height comes from OSM `height` (comma-sanitised) or, failing that, `building:levels`: `height = max(levels, 1) * 3 + 2`.
  - `min_height` is written **only for `kind = building_part`**; a plain `building` has no `min_height` attribute. The base expression therefore needs a fallback (see §6).
  - Buildings have `setZoomRange(11, 15)`; `building_part` starts at z14.
  - Post-process at z < 15: `height` rounded to 20 m (z ≤ 12), 10 m (z13), **5 m (z14)**, then `FeatureMerge.mergeNearbyPolygons(items, 3.125, 3.125, 0.5, 0.5)`.
- Consequence for the shipped Map Pack (z14 ceiling): every building the extrusion layer sees is a **merged block with a 5 m-quantised height**, overzoomed at every Drive Mode zoom. Individual footprints exist only in a z15 build of the pack. The style spec confirms overzoom semantics: "Data from tiles at the maxzoom are used when displaying the map at higher zoom levels" ([style spec, sources](https://maplibre.org/maplibre-style-spec/sources/)).
- Protomaps' own published style has **no** extrusion layer: `buildings` is `type: "fill"`, filter `["in","kind","building","building_part"]`, `fill-opacity: 0.5`, ordered after roads/water and before the labels group ([`base_layers.ts`](https://github.com/protomaps/basemaps/blob/main/styles/src/base_layers.ts)). Our `style-light.json` mirrors it (`#cccccc`, 0.5).

## 1. Layer definition and light

### 1.1 Properties (style spec + shipped header)

| Style-spec property | iOS property (`MLNFillExtrusionStyleLayer`) | Default | Data-driven | Notes |
|---|---|---|---|---|
| `fill-extrusion-height` (m) | `fillExtrusionHeight` | 0 | yes | |
| `fill-extrusion-base` (m) | `fillExtrusionBase` | 0 | yes | "Must be less than or equal to `fill-extrusion-height`"; only applied if height set |
| `fill-extrusion-color` | `fillExtrusionColor` | `#000000` | yes | alpha component is ignored; shaded by the light |
| `fill-extrusion-opacity` | `fillExtrusionOpacity` | 1 | **no** | "The opacity of the entire fill extrusion layer. This is rendered on a per-layer, not per-feature, basis" |
| `fill-extrusion-vertical-gradient` | `fillExtrusionHasVerticalGradient` | true | no | darkens sides toward the base |
| `fill-extrusion-rounded-corner-distance` (m, layout) | `fillExtrusionRoundedCornerDistance` | 0 | no | new in 6.28.0; see §3 and §5 |
| `fill-extrusion-translate` / `-anchor` | `fillExtrusionTranslation` / `TranslationAnchor` | [0,0] / map | no | not needed |

Sources: [MapLibre style spec, layers](https://maplibre.org/maplibre-style-spec/layers/); shipped header `MLNFillExtrusionStyleLayer.h` (6.29.0); iOS CHANGELOG 6.28.0 "Add fill extrusion style property that enables rounded corners for extruded buildings" ([#4343](https://github.com/maplibre/maplibre-native/pull/4343)).

### 1.2 The reference configurations

- **MapLibre Native iOS example** (`BuildingLightExample`): `fillExtrusionBase = NSExpression(forKeyPath: "render_min_height")`, `fillExtrusionHeight = NSExpression(forKeyPath: "render_height")`, opacity 0.8, colour white, inserted `below` the `poi` symbol layer; `MLNLight` with `MLNSphericalPositionMake(5, 180, 80)` and `anchor = "map"`; set via `style.light = light`. Source: [BuildingLightExample.md](https://github.com/maplibre/maplibre-native/blob/main/platform/ios/MapLibre.docc/BuildingLightExample.md).
- **Mapbox GL JS "Display buildings in 3D"**: `minzoom: 15`, colour `#aaa`, opacity 0.6, height and base `interpolate linear zoom 15 → 0, 15.05 → get(height|min_height)`, inserted before the first symbol layer with a `text-field`. Source: [Mapbox example](https://docs.mapbox.com/mapbox-gl-js/example/3d-buildings/).
- **MapLibre GL JS "Display buildings in 3D"**: `minzoom: 15`, height `interpolate zoom 15 → 0, 16 → render_height`, base `case zoom ≥ 16 → render_min_height else 0`, inserted before the first text symbol layer. Source: [MapLibre GL JS example](https://maplibre.org/maplibre-gl-js/docs/examples/display-buildings-in-3d/).
- **Stadia Maps tutorial**: `minzoom: 13`, `lightgray`, height ramps 13 → 16, inserted "just under the first `symbol` layer" with a text field. Source: [Stadia tutorial](https://docs.stadiamaps.com/tutorials/adding-3d-buildings-to-your-maps-with-maplibre/).
- **MapLibre Android example**: colour `LTGRAY`, opacity 0.9, light positions `(1.5, 90, 80)` and `(1.15, 210, 30)` toggled. Source: [Building Layer](https://maplibre.org/maplibre-native/android/examples/styling/building-layer/).

Consensus: neutral light grey, opacity 0.6–0.9, height faded in over ≤ 1 zoom level from the layer's minzoom, layer just under the first text symbol layer.

### 1.3 Light

Spec ([light](https://maplibre.org/maplibre-style-spec/light/)), identical defaults in the shipped `MLNLight.h`:

- `anchor`: `map` | `viewport`, default **`viewport`**. "Whether extruded geometries are lit relative to the map or viewport." With `viewport` the lit faces stay on the same screen side while the heading-up Follow Camera rotates the world; with `map` the shading rotates with the world like a sun. For heading-up navigation `viewport` is the stable choice (Google's shading is screen-stable in nav).
- `position` `[radial, azimuthal, polar]`, default **`[1.15, 210, 30]`**: azimuth 0° = top of the viewport (or due north with `map`), clockwise; polar 0° = directly above, 180° = below.
- `intensity` 0–1, default **0.5**: "Higher numbers will present as more extreme contrast."
- `color`, default `#ffffff`.

What the Metal shader does with these ([`include/mln/shaders/mtl/fill_extrusion.hpp`](https://github.com/maplibre/maplibre-native/blob/main/include/mln/shaders/mtl/fill_extrusion.hpp)):

```
fMin   = mix(0.7, 0.98, 1.0 - light_intensity);
factor = clamp((t + base) * pow(height / 150.0, 0.5), fMin, 1.0);
directional *= (1.0 - vertical_gradient) + (vertical_gradient * factor);
minLight = mix(0.0, 0.3, 1.0 - light_color);
color = clamp(color * directional * light_color, minLight, 1.0);
```

Read: the vertical gradient darkens a wall toward its base by at most `1 - fMin`, and **`fMin` is driven by intensity** — intensity 0.5 allows walls down to 84 % of the roof shade, intensity 0.3 only to ~91 %. So "depth without a dark scene" is a low intensity (0.25–0.4), not a different colour. A pure white light gives `minLight = 0`, i.e. no floor on how dark a face can go; a faint warm/neutral tint (e.g. `#f4f2ee`) raises the floor slightly. A polar angle near 30–45° gives clear roof/wall contrast; the default 30° is fine.

## 2. Zoom, LOD and the z14 ceiling

- **minzoom**: the reference styles start extrusions at z15 (Mapbox, MapLibre GL JS) or z13 (Stadia). The ticket's "Google from roughly z15–16" is consistent. With our tiles, z15 is also where a z15 pack would switch from merged blocks to individual buildings, so a `minimumZoomLevel = 15` layer keeps the option of a later z15 Map Pack changing nothing else.
- **Layer minzoom vs source maxzoom**: layer `minzoom` hides the layer below that map zoom ([spec](https://maplibre.org/maplibre-style-spec/layers/)); the source `maxzoom: 14` means every zoom ≥ 14 reuses z14 data ([spec](https://maplibre.org/maplibre-style-spec/sources/)). Extrusion buckets are built per *overscaled* tile ID (`tile.getOverscaledTileID()` in [`render_fill_extrusion_layer.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/renderer/layers/render_fill_extrusion_layer.cpp)), so crossing an integer zoom (15 → 16 → 17) re-tessellates the same z14 geometry into 4× as many buckets each level. Issue [#4542](https://github.com/maplibre/maplibre-native/issues/4542) describes exactly this: with a `maxzoom: 14` source and zooms 15–17, "overscaling the given tile for a given zoom level ... leads to bucket/tile regeneration geometry, which contributes to dropping FPS" — reported only when rounded corners are on. Keep rounded corners at 0 and keep the Follow Camera's zoom range narrow (it already is: altitude-driven, ~z15–16).
- **`tileLodPitchThreshold` is in radians and defaults to 60°**: shipped `MLNMapView.h`: "Pitch angle in radians above which LOD calculation is performed"; `MLNMapView.mm` passes it straight to `mln::Map::setTileLodPitchThreshold`; core default `(60.0 / 180.0) * pi` ([`map_impl.hpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/map/map_impl.hpp)); the check is `transform.getPitch() > state.tileLodPitchThreshold` ([`tile_cover.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/util/tile_cover.cpp)). `maximumPitch` doc: "The pitch may not exceed 60 degrees regardless of this property." So **at the fixed 60° pitch the default threshold never fires** — variable LOD is off unless the app lowers it (e.g. `0.5236` = 30°). Companion knobs (shipped header): `tileLodMinRadius` (tiles kept at full zoom around the view point, default 3, "must be greater than 1"), `tileLodScale` (default 1, "larger than 1 ... reducing LOD"), `tileLodZoomShift` (default 0, "not recommended ... unless performance is critical"). Rationale from the PR: "when the screen is tilted (Often the case during active Navigation...), tiles away from the camera could use lower zoom levels to maintain a certain level of performance" ([#2958](https://github.com/maplibre/maplibre-native/pull/2958)).
- Note the LOD lowers the *tile* zoom for far tiles; below z14 those tiles still carry buildings (from z11) with 10–20 m height steps, so distant blocks get coarser rather than vanish. A `minimumZoomLevel` on the layer is evaluated against the map zoom, not the tile zoom, so it does not cull far tiles.

## 3. Performance

- **No published fps numbers** for fill-extrusion on iOS/Metal exist in MapLibre docs, release notes or issues; the numbers that exist are memory and one fps regression:
  - [#4107](https://github.com/maplibre/maplibre-native/issues/4107) (iOS 6.23.0, iPhone 15 Pro): ~400 MB at z17 without extrusions vs 900 MB–1.6 GB with; HudHud "had to disable 3D buildings during navigation entirely because the system kills the app in the background due to memory pressure". Also: "tile count increases significantly with pitch due to horizon visibility". Closed by instancing.
  - [#4256](https://github.com/maplibre/maplibre-native/pull/4256) "Optimize fill extrusion memory by using instancing", released in **iOS 6.26.1** (CHANGELOG): buckets memory 39 MB → 6.7 MB at z14.95 (22 buckets), 62 MB → 10.6 MB at z16 (34 buckets). We ship 6.29.0, so this is in.
  - [#4343](https://github.com/maplibre/maplibre-native/pull/4343) rounded corners: "will consume 5x more memory when enabled" (2.3 MB → 12.1 MB for 5 buckets at z15.3); [#4542](https://github.com/maplibre/maplibre-native/issues/4542) reports fps drops with them on over maxzoom-capped sources. **Leave at 0.**
- **Opacity does not force sorting, it forces a second pass.** In [`render_fill_extrusion_layer.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/renderer/layers/render_fill_extrusion_layer.cpp): the layer always renders in `RenderPass::Translucent`; drawables are back-face culled (`CullFaceMode::backCCW()`), colour drawables are `alphaBlended`; and `doDepthPass = (!opaque || hasPattern)` where `opaque = opacity >= 1` — a depth-only drawable (priority 0) is emitted before the colour drawable (priority 1) so overlapping translucent walls do not double-blend. Opacity 1 skips the depth pre-pass: one draw per tile instead of two. The comment in the file: "The non-pattern path in `render()` only uses two-pass rendering if there's translucency."
- **Feature count** is set by the tiles: z14 merged blocks (§0) are fewer polygons than z15 individual buildings, which is favourable. Pitch is the multiplier (horizon tiles), addressed by the LOD knobs in §2.
- **Antialiasing**: no MSAA/sample-count property is exposed on the shipped `MLNMapView.h` (searched `sample`, `msaa`, `antialias`); there is nothing to tune here.
- **Measuring on device**:
  - `mapView.enableRenderingStatsView(true)` shows the HUD (since 6.15.0, [RenderingStatisticsHud.md](https://github.com/maplibre/maplibre-native/blob/main/platform/ios/MapLibre.docc/RenderingStatisticsHud.md)); the same numbers arrive per frame through `mapViewDidFinishRenderingFrame:fullyRendered:renderingStats:` as `MLNRenderingStats` (`encodingTime`, `renderingTime`, `numFrames`, `numDrawCalls`, `numVertexBuffers`, `memVertexBuffers`, ... — shipped `MLNRenderingStats.h`). That is the ADR 0002 instrument: log `renderingTime`/`encodingTime` per frame during a Trip Log replay and count frames over 8.3 ms.
  - The app already sets `CADisableMinimumFrameDurationOnPhone: true` (`app/Wayfinder/project.yml`) and can pin `preferredFramesPerSecond = 120` (`MLNMapView.h`).
  - Instruments: the Metal System Trace template ("Analyzing the performance of your Metal app", [Apple](https://developer.apple.com/documentation/metal/using_metal_system_trace_in_instruments_to_profile_your_app)) for GPU frame time and vsync misses; Xcode's GPU frame capture for per-encoder cost.

## 4. Ordering

How the renderer orders things (all from [`render_fill_extrusion_layer.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/renderer/layers/render_fill_extrusion_layer.cpp), [`paint_parameters.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/renderer/paint_parameters.cpp), [`renderer_impl.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/renderer/renderer_impl.cpp)):

- Extrusion drawables are `is3D`, drawn in the translucent pass **in style-layer order** with depth read+write (`depthModeFor3D` = LessEqual, ReadWrite). Anything drawn after them in layer order paints over their walls; anything drawn before is occluded where a wall stands in front of it. This is the behaviour every reference style relies on ("insert below the first symbol layer" so labels stay on top). The one known violation, symbols hidden under extrusions ([#2259](https://github.com/maplibre/maplibre-native/issues/2259), [#2894](https://github.com/maplibre/maplibre-native/issues/2894)), was diagnosed by the maintainer as "happening only on OpenGL, not on Vulkan or Metal".
- **Route line**: `RouteLayer.addLayers` inserts the ribbon `below` the first `MLNSymbolStyleLayer`. `insertLayer(_:below:)` places a layer *immediately* below the anchor, so the last layer inserted there is the highest. If the extrusion layer is inserted below the first symbol layer **after** the route, it lands **above** the route and hides it. Either add the extrusion first, or insert it explicitly `below` `RouteLayer.routeLineId` when the route exists. Route above buildings is what Google draws in nav and what keeps the Plan legible; the cost is the route floating over walls in perspective, judged on the prototype (map §"Not yet specified").
- **Roads**: the reference styles put extrusions above all road layers (just under labels), so walls occlude streets behind them — the correct 3D read. Inserting the extrusion at the flat `buildings` slot (below roads) would paint roads across building sides at 60° pitch. Recommendation: hide the flat `buildings` fill in Drive Mode and insert the extrusion just under the first symbol layer.
- **Puck**: `MLNAnnotationImage` points render through an internal symbol layer `org.maplibre.annotations.points` that `AnnotationManager::updateStyle()` appends with `addLayer` (no `before`) on every style load, i.e. at the top of the stack at that moment ([`annotation_manager.cpp`](https://github.com/maplibre/maplibre-native/blob/main/src/mln/annotation/annotation_manager.cpp)); symbol drawables are not occluded by the 3D depth pass on Metal (see #2259 above). The puck stays visible over buildings. Layers the app adds later with `style.addLayer` (stops circles/labels) go above it — unchanged.
- **Labels/shields**: unaffected as long as the extrusion stays under the first symbol layer.

## 5. Gotchas in 6.2x

| Item | Status | Relevance |
|---|---|---|
| Memory blow-up at street zoom ([#4107](https://github.com/maplibre/maplibre-native/issues/4107)) | fixed by instancing, iOS 6.26.1 | we ship 6.29.0 |
| Rounded corners 5× memory ([#4343](https://github.com/maplibre/maplibre-native/pull/4343)) + fps drops on maxzoom-capped sources ([#4542](https://github.com/maplibre/maplibre-native/issues/4542), open) | open | our source is capped at z14: keep `fillExtrusionRoundedCornerDistance` at 0 |
| Filters on a fill-extrusion layer not applied until zoom changes ([#3039](https://github.com/maplibre/maplibre-native/issues/3039)) | fixed late 2024 | set the predicate at creation; toggle Drive Mode with `isVisible`/remove-add, not by mutating the predicate |
| Extrusion not refreshed when a GeoJSON source is replaced, z-fighting old/new ([#2746](https://github.com/maplibre/maplibre-native/issues/2746)) | open | not our case (vector-tile source); do not move buildings to an `MLNShapeSource` |
| Symbols drawn under extrusions ([#2259](https://github.com/maplibre/maplibre-native/issues/2259)) | OpenGL-only | Metal unaffected |
| Custom 3D layers cannot depth-test against extrusions ([#4301](https://github.com/maplibre/maplibre-native/issues/4301)) | `nearClippedProjectionMatrix` added to `MLNCustomStyleLayer`, iOS 6.28.0 ([#4364](https://github.com/maplibre/maplibre-native/pull/4364)) | matters only if the puck becomes a custom Metal 3D chevron |
| Depth-buffer precision when tilting ([#1863](https://github.com/maplibre/maplibre-native/issues/1863)) | closed | watch for wall shimmer at the horizon on the prototype |
| Flicker on style reload | no extrusion-specific issue found (searched maplibre-native issues) | the light/dark swap sets `mapView.styleURL`, a full reload; `PlanStore.mapView(_:didFinishLoading:)` re-adds app layers. The extrusion layer **and** `style.light` live on the style and must be re-applied there, with theme-specific colour |
| `MLNAnnotationView` (UIView) forces synchronous rendering on Metal (iOS 6.4.1 note, `docs/research/map-rendering-ios.md` §2.3) | by design | keep the puck an `MLNAnnotationImage` |
| Pan clamped to the horizon on pitched maps ([#3105](https://github.com/maplibre/maplibre-native/pull/3105), iOS 6.28.0) | shipped | relevant to the camera ticket with `contentInset` at 60° |
| `fill-extrusion-color` alpha ignored ([spec](https://maplibre.org/maplibre-style-spec/layers/)) | by design | use `fillExtrusionOpacity` |
| Plain `building` features have no `min_height` (§0) | tile schema | wrap the base in `mgl_coalesce` |

## 6. Recommended layer + light configuration for the prototype

NSExpression syntax from [Predicates and Expressions](https://github.com/maplibre/maplibre-native/blob/main/platform/ios/MapLibre.docc/Predicates_and_Expressions.md) (`mgl_interpolate:withCurveType:parameters:stops:($zoomLevel, 'linear', nil, %@)`, `mgl_coalesce({x, y})`).

```swift
// Drive Mode only; added in didFinishLoading(style:) and on Go, removed on End.
let source = style.source(withIdentifier: "protomaps")!   // the PMTiles vector source
let extrusion = MLNFillExtrusionStyleLayer(identifier: "buildings-3d", source: source)
extrusion.sourceLayerIdentifier = "buildings"
extrusion.predicate = NSPredicate(format: "kind IN {'building', 'building_part'}")
extrusion.minimumZoomLevel = 15

// Height fades in over half a zoom level so buildings "grow" as the camera drops in.
let height = NSExpression(forKeyPath: "height")
let base = NSExpression(format: "mgl_coalesce({min_height, 0})")   // plain buildings carry no min_height
extrusion.fillExtrusionHeight = NSExpression(
    format: "mgl_interpolate:withCurveType:parameters:stops:($zoomLevel, 'linear', nil, %@)",
    [15: NSExpression(forConstantValue: 0), 15.5: height])
extrusion.fillExtrusionBase = NSExpression(
    format: "mgl_interpolate:withCurveType:parameters:stops:($zoomLevel, 'linear', nil, %@)",
    [15: NSExpression(forConstantValue: 0), 15.5: base])

// Neutral, a touch lighter than the flat fill so walls read against #cccccc-class ground.
extrusion.fillExtrusionColor = NSExpression(forConstantValue: isDark
    ? UIColor(white: 0.28, alpha: 1) : UIColor(white: 0.86, alpha: 1))
extrusion.fillExtrusionOpacity = NSExpression(forConstantValue: 0.75)  // < 1 costs one extra depth pass; try 1.0 if fps demands
extrusion.fillExtrusionHasVerticalGradient = NSExpression(forConstantValue: true)
extrusion.fillExtrusionRoundedCornerDistance = NSExpression(forConstantValue: 0)   // 5x memory + fps drops on z14-capped tiles

// Ordering: above roads, below labels, below the route.
style.layer(withIdentifier: "buildings")?.isVisible = false   // flat fill off in Drive Mode
if let route = style.layer(withIdentifier: RouteLayer.routeLineId) {
    style.insertLayer(extrusion, below: route)
} else if let firstSymbol = style.layers.first(where: { $0 is MLNSymbolStyleLayer }) {
    style.insertLayer(extrusion, below: firstSymbol)
}
// RouteLayer.addLayers must keep the route above this layer: add the extrusion before the
// route, or have RouteLayer insert below the first symbol layer *after* the extrusion exists.

// Light: screen-stable, low contrast, from the upper-left, ~40° off vertical.
let light = MLNLight()
light.anchor = NSExpression(forConstantValue: "viewport")
light.position = NSExpression(forConstantValue: NSValue(mlnSphericalPosition: MLNSphericalPositionMake(1.15, 210, 40)))
light.intensity = NSExpression(forConstantValue: 0.35)
light.color = NSExpression(forConstantValue: isDark ? UIColor(white: 0.9, alpha: 1) : UIColor.white)
style.light = light   // re-apply after every style swap

// Tile LOD for the 60° camera (radians; default 60° never triggers at the 60° cap).
mapView.tileLodPitchThreshold = 30 * .pi / 180
mapView.tileLodMinRadius = 3   // default; raise to 4 if near buildings pop
mapView.tileLodScale = 1       // default; >1 trades far detail for fps
mapView.tileLodZoomShift = 0   // leave alone
```

Layer order, bottom to top, in Drive Mode: `... roads_* → (buildings flat, hidden) → buildings-3d → route-line → route-line-top → route-connector → first symbol layer (address/road labels, shields, pois, places) → org.maplibre.annotations.points (puck) → stops-circles → stops-labels`.

Acceptance instrumentation: `enableRenderingStatsView(true)` in the debug build plus a per-frame log of `MLNRenderingStats.renderingTime`/`encodingTime` from `mapViewDidFinishRenderingFrame:fullyRendered:renderingStats:` during the Trip Log replay through a dense town at pitch 60; the ADR 0002 bar is no frame over 8.3 ms on the iPhone 15 Pro. First knobs if it misses: opacity 1.0 (drops the depth pre-pass), `tileLodScale` 1.5, then a lower `tileLodMinRadius` — before touching `tileLodZoomShift`.

Open question for the prototype, not answerable from sources: how blocky the z14 merged footprints look at z16 in Luxembourg City. If they read as slabs, the fix is a z15 Map Pack build (`Buildings.java` already emits individual buildings at z15), sized per `docs/research/map-pack-sizes.md`.
