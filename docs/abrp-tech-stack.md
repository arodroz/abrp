# ABRP — Tech stack analysis from the "Attributions" screen

Source: three iOS screenshots of the ABRP (A Better Routeplanner) mobile app's
*Attributions* sheet, French locale, taken 27 Aug 2026 at 14:30–14:31
(`abrp_1.png`, `abrp_2.png`, `abrp_3.png` — kept locally, git-ignored).

The screen opens with a note that ABRP is built on open-source software, data and
services "created by people, communities and companies around the world", then
lists 17 attributed components in four groups.

## 1. Inventory

### Cartes et météo (maps & weather)

| Component | Role (as stated) | Notes |
|---|---|---|
| OpenStreetMap | Map data | ODbL-licensed community map database |
| MapTiler | Map tiles & style | Commercial vector-tile host built on OSM; supplies the rendered basemap |
| OpenWeather | Weather data | Only entry shown with a logo (attribution requirement of OpenWeather's terms). Used for range estimation (temperature, wind, precipitation) |

### Recharge (charging)

| Component | Role | Notes |
|---|---|---|
| OpenChargeMap | Charging-station data | Open, crowdsourced charger registry |
| Eco-Movement | Charging-station data | Commercial aggregator of live charger status / pricing in Europe |

Two sources for the same data ⇒ ABRP merges an open dataset with a paid, higher-quality feed.

### Itinéraires et infrastructure (routing & infrastructure)

| Component | Role | Notes |
|---|---|---|
| Valhalla | Routing engine | Open-source (Mapbox origin), tiled, supports costing models — good fit for EV energy-aware routing |
| OSRM | Routing engine | Second open-source router; likely used for fast/fallback routing or specific profiles |
| Redis | In-memory database | Server-side cache / session / queue |
| Elasticsearch | Search & analytics engine | Geocoding / place search and possibly telemetry analytics |

These four are **backend** components — the app attributes its server stack, not only client libraries.

### Frameworks et bibliothèques (frameworks & libraries)

| Component | Role | Notes |
|---|---|---|
| Expo | Application framework | Managed React Native toolchain (builds, OTA updates) |
| React Native | Cross-platform UI framework | Single codebase for iOS + Android |
| React | UI library | |
| React Native Skia | 2D graphics | GPU-accelerated drawing — probably the route/elevation/SoC charts and map overlays |
| React Native Reanimated | Animations | Native-thread animations (bottom sheets, transitions) |
| Redux Toolkit | State management | Centralised app state (vehicle, plan, settings) |
| Turf.js | Geospatial analysis | Client-side geometry: distances, along-line points, bounding boxes |
| i18next | Localisation | Explains the fully translated French UI |

## 2. Architecture inferred

```
┌──────────────── Mobile client (Expo / React Native) ────────────────┐
│ React + Redux Toolkit ── state                                       │
│ Skia (charts, overlays) · Reanimated (motion) · i18next (locale)     │
│ Turf.js — local geospatial maths                                     │
│ MapTiler tiles (OSM data) rendered in the map view                   │
└──────────────────────────────┬───────────────────────────────────────┘
                               │ HTTPS API
┌──────────────────────────────▼───────────────────────────────────────┐
│ ABRP backend                                                          │
│ Valhalla / OSRM ── route computation on OSM graph                     │
│ Elasticsearch ── place search / analytics     Redis ── cache          │
│ External feeds: OpenWeather · OpenChargeMap · Eco-Movement            │
└───────────────────────────────────────────────────────────────────────┘
```

## 3. Observations

- **Open-source first, commercial where quality matters.** Map data (OSM), routing
  (Valhalla, OSRM) and charger data (OpenChargeMap) are open; MapTiler, OpenWeather
  and Eco-Movement are the paid layers that add tiles, weather and live charger status.
- **Mixed client/server attribution.** Listing Redis and Elasticsearch on a phone app's
  credits screen is unusual and suggests a single attribution list shared by web and
  mobile clients.
- **Modern RN stack.** Expo + Skia + Reanimated + Redux Toolkit is the current
  "performance-oriented" React Native combination; no native map SDK (Mapbox/Google)
  is credited, consistent with MapTiler vector tiles rendered via a MapLibre-style view.
- **Two routing engines.** Valhalla's pluggable costing is the natural home for
  energy-based EV routing; OSRM is faster for plain shortest-path queries — likely
  used for previews or as a fallback.
- **Every entry carries an external-link icon**, i.e. each attribution links to the
  project's site or licence, as most open-source licences (MIT, BSD, ODbL) require.

## 4. Screenshot ↔ content map

| File | Content |
|---|---|
| `abrp_1.png` | Intro text · Cartes et météo · Recharge · start of Itinéraires |
| `abrp_2.png` | Itinéraires et infrastructure · start of Frameworks (Expo … Skia) |
| `abrp_3.png` | Frameworks et bibliothèques, full list (Expo … i18next) |
