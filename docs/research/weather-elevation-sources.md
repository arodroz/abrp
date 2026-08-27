# Weather and elevation sources for the Energy Model

Research note for issue #7. Question: which sources can supply **temperature and wind along
each Leg at the time the vehicle will be there**, and **an elevation profile per Leg**, for a
personal, Europe-only, native iOS (Swift + Rust) ABRP replica — with licences, rate limits,
cost, resolution, data size, and the cheapest way to sample elevation on-device.

Every factual claim is followed by the URL it was checked against on 2026-08-27. Figures
marked *(estimate)* are my own arithmetic, not a source.

---

## 1. What the Energy Model actually needs

- Per Leg: an ordered set of sample points along the road geometry, each with an **ETA**
  (departure time + cumulative duration from the Routing Engine). At each sample: air
  temperature (2 m), wind speed and direction (10 m) so the head/tail/cross component can
  be projected onto the Leg heading. Hourly resolution is enough; a Plan rarely exceeds
  2–3 days, so 48 h–3 day horizons cover almost all cases.
- Per Leg: elevation at every geometry vertex (or every ~50–100 m) to derive grade.
  Vertical precision of ~1 m and horizontal resolution of ~30 m are ample; road grade
  errors from a 30 m DSM are dominated by the DSM's own noise on bridges/tunnels/tree
  cover, not by sampling.
- Weather changes slowly in space (model grids are 1–11 km), so sampling one weather point
  per 10–20 km of Leg, or one per Leg for short Legs, is sufficient. Elevation must be
  sampled densely, so it should come from **local data**, not a per-point web API.

---

## 2. Weather sources

### 2.1 Open-Meteo (recommended default)

- Endpoint `/v1/forecast`; multiple coordinates in one call:
  "Multiple coordinates can be comma separated. E.g. &latitude=52.52,48.85&longitude=13.41,2.35".
  Hourly variables include `temperature_2m`, `wind_speed_10m`, `wind_direction_10m`,
  `wind_gusts_10m`; up to 16 forecast days; `start_hour`/`end_hour` restrict the hourly
  window; `elevation=` overrides the DEM downscaling; `timeformat=unixtime` available.
  https://open-meteo.com/en/docs
- Model selection: "For each location, the highest-resolution applicable model is selected
  automatically." (`models=best_match`). https://open-meteo.com/en/docs
- European high-resolution models available through the same endpoint:
  - DWD ICON-D2 2 km / 2-day horizon / 15-minutely, ICON-EU 7 km / 5 days, ICON global
    11 km / 7.5 days. https://open-meteo.com/en/docs/dwd-api
  - Météo-France AROME 2.5 km & AROME HD 1.5 km, 2-day horizon, hourly, updated every
    3 h; ARPEGE Europe 11 km, 4 days. https://open-meteo.com/en/docs/meteofrance-api
  - KNMI HARMONIE-AROME 2 km (NL/BE) and 5.5 km (Central & Northern Europe), 2.5 days,
    updated hourly, then blended into ECMWF IFS HRES 9 km to 15 days.
    https://open-meteo.com/en/docs/knmi-api
  - DMI HARMONIE-AROME Europe 2 km, 2.5 days, updated every 3 h.
    https://open-meteo.com/en/docs/dmi-api
  - ECMWF IFS HRES 9 km hourly, 15 days, every 6 h; ECMWF open data is CC BY 4.0 since
    1 Oct 2025. https://open-meteo.com/en/docs/ecmwf-api
- Licence: data CC BY 4.0, required attribution
  `<a href="https://open-meteo.com/">Weather data by Open-Meteo.com</a>`; server code
  AGPLv3. https://open-meteo.com/en/licence , https://github.com/open-meteo/open-meteo
- Free tier (non-commercial): 600 calls/min, 5 000/h, 10 000/day, 300 000/month; requests
  with >10 variables or >2 weeks of data count fractionally (e.g. 15 variables × 2 weeks =
  1.5 calls). Commercial plans start at "Standard" 1 M calls/month on
  `customer-api.open-meteo.com`. https://open-meteo.com/en/pricing
- Non-commercial is defined as "private or non-profit websites or apps that do not have
  subscriptions or advertising" — a personal project qualifies. https://open-meteo.com/en/terms
- No API key needed on the free tier. https://open-meteo.com/en/docs
- Self-hostable via Docker (AGPLv3) if you ever need to remove the rate limit.
  https://github.com/open-meteo/open-meteo

**Access pattern for a Plan**: one request per Plan with every Leg sample point comma-joined
(e.g. 20–40 points), `hourly=temperature_2m,wind_speed_10m,wind_direction_10m`,
`forecast_days=3`, `timeformat=unixtime`; then per sample pick the hour bucket matching that
point's ETA. Cost: 1 call per Plan (variables ≤10, period ≤2 weeks). The whole daily budget
(10 000) is thousands of Plans — effectively unlimited for one user.

**Cons**: non-commercial only on the free tier; model blend seams between countries;
ICON-D2/AROME horizons are 2 days so day-3 Legs fall back to 7–11 km models.

### 2.2 MET Norway Locationforecast 2.0

- Endpoints `/complete`, `/compact` (`/classic` XML legacy); required `lat`, `lon`; optional
  `altitude` ("recommended for precise temperature values"). Returns `air_temperature`,
  `wind_speed`, `wind_from_direction`, `wind_speed_of_gust`; 9-day horizon.
  https://api.met.no/weatherapi/locationforecast/2.0/documentation
- Temporal resolution: hourly for roughly the first 60 h, then 6-hourly. Models: MEPS 2.5 km
  in the Nordic region, ECMWF HRES ~9 km elsewhere (global).
  https://docs.api.met.no/doc/locationforecast/datamodel
- Nordic temperatures come from a 1 km "Met Forecast" model; altitude parameter adjusts
  temperature only. https://docs.api.met.no/doc/locationforecast/FAQ
- Terms: CC BY 4.0; **mandatory identifying `User-Agent`** with contact info (else 403);
  hard cap 20 req/s **per application, aggregated over all installs**; must honour
  `Expires` and use `If-Modified-Since`; coordinates truncated to ≤4 decimals (5+ decimals
  → 403); mobile apps should poll no more than every 10 min.
  https://api.met.no/doc/TermsOfService , https://docs.api.met.no/doc/locationforecast/HowTO
- **One point per request only** — no batch. A Plan with 30 samples = 30 requests.

**Verdict**: excellent quality in Scandinavia (2.5 km MEPS, 1 km temperature), free, no
key; but single-point requests and strict client-etiquette rules. Good as a secondary
source for Nordic trips, not as the default for a route-sampling workload.

### 2.3 OpenWeather (what ABRP itself attributes — see `docs/abrp-tech-stack.md`)

- Free plan: 60 calls/min, 1 000 000 calls/month, includes Current Weather, 3-hourly
  5-day forecast, Air Pollution, Geocoding. Licence for self-service plans: ODbL,
  "Commercial use is allowed", attribution "Weather data © OpenWeather".
  https://openweathermap.org/full-price
- One Call API 3.0: "The first 1,000 API calls per day are free", 0.0012 GBP per extra
  call, **card subscription required even for the free quota**; returns current, minutely
  (1 h), hourly (48 h), daily (8 days), alerts, plus historical timemachine and daily
  aggregation. https://openweather.co.uk/blog/post/upgrade-your-weather-data-experience-one-call-api-30
- The pricing page now lists "One Call API 4.0 / Timeline-Based Weather" under the Startup
  plan (600 calls/min, 10 M calls/month). https://openweathermap.org/full-price
- One coordinate per request; no batch endpoint.

**Cons**: key + card on file; 3-hourly on the truly free tier (too coarse for a Leg ETA
without interpolation); ODbL share-alike obligations if you redistribute the data; per-point
calls (30 samples = 30 calls; still within 1 000/day for personal use).

### 2.4 Apple WeatherKit (native iOS option)

- Swift + REST APIs; hourly forecast for 10 days incl. temperature and wind; requires Apple
  Developer Program (paid membership); **500 000 calls/month included**, then $49.99 for
  1 M etc.; iOS 16+. Attribution: must display the Apple Weather trademark and link to the
  attribution page. https://developer.apple.com/weatherkit/get-started/
- Upstream models attributed: NOAA, ECCC, DWD, Met Office/ECMWF, JMA, Météo-France.
  https://developer.apple.com/weatherkit/data-source-attribution/
- `HourWeather` exposes temperature and a `Wind` value (speed, direction, gust).
  https://developer.apple.com/documentation/weatherkit

**Pros**: first-party Swift API, no key management, generous quota. **Cons**: needs the
$99/yr programme (which you need anyway to ship to a device for >7 days), one location per
call, trademark attribution UI, no Rust access (Swift only, or REST with JWT).

### 2.5 Others considered

- **Meteomatics** has a purpose-built *route query* (`&route=true`, a list of times paired
  with a list of locations) — exactly the Leg-sampling shape we need — but pricing is
  quote-based, no published free tier.
  https://www.meteomatics.com/en/api/request/advanced-requests/route-queries/ ,
  https://www.meteomatics.com/en/pricing/
- **Pirate Weather**: Dark-Sky-compatible, 10 000 calls/month free (20 000 with $2/mo
  donation), NOAA GFS/HRRR/NBM-based (HRRR/NBM are US-only, so Europe gets GFS ~25 km).
  https://docs.pirateweather.net/en/latest/ , https://docs.pirateweather.net/en/latest/API/
- **WeatherAPI.com**: 100 000 calls/month free but only 3-day forecast on free; Pro+ $25/mo.
  https://www.weatherapi.com/pricing.aspx

### 2.6 Weather comparison

| Source | Batch points/call | Hourly horizon | Europe resolution | Free quota | Key | Licence |
|---|---|---|---|---|---|---|
| Open-Meteo | yes (comma list) | 16 d | 1.5–2.5 km (D2/AROME/HARMONIE) then 9 km | 10 k/day non-commercial | none | CC BY 4.0 |
| MET Norway | no | ~60 h hourly, 6-hourly to 9 d | 2.5 km Nordic, 9 km elsewhere | 20 req/s app-wide | none (User-Agent) | CC BY 4.0 |
| OpenWeather One Call 3.0 | no | 48 h | undisclosed blend | 1 000/day (card required) | yes | ODbL |
| Apple WeatherKit | no | 10 d | undisclosed blend | 500 k/month | Dev Program | Apple ToS + trademark |
| Meteomatics | yes (route query) | n/a | n/a | none published | yes | commercial |

**Recommendation**: Open-Meteo as the primary source (one batched call per Plan, hourly,
no key, CC BY). Keep the weather client behind a small protocol so WeatherKit can be
swapped in later if the project ever becomes commercial (Open-Meteo free tier would then
no longer apply).

---

## 3. Elevation sources

### 3.1 Raw DEMs

**Copernicus DEM GLO-30 (recommended raw source)**
- Digital *surface* model (includes buildings/vegetation) from TanDEM-X 2011–2015; 30 m
  global (GLO-90 at 90 m); absolute vertical accuracy <4 m LE90; 1°×1° tiles; EPSG:4326,
  vertical datum EGM2008; GeoTIFF/DTED.
  https://dataspace.copernicus.eu/explore-data/data-collections/copernicus-contributing-missions/collections-description/COP-DEM
- Public S3 mirror: buckets `copernicus-dem-30m` and `copernicus-dem-90m`, Cloud-Optimized
  GeoTIFF, no AWS account needed. https://registry.opendata.aws/copernicus-dem/
- Tile naming `Copernicus_DSM_COG_10_N50_00_E006_00_DEM/…`, DEFLATE-compressed COG with
  1024×1024 internal tiling; `tileList.txt` in each bucket; GLO-30 "Public" omits tiles
  for a few countries. https://copernicus-dem-30m.s3.amazonaws.com/readme.html
- Licence (COP-DEM-GLO-30-F): rights of reproduction, distribution, communication to the
  public, adaptation/combination; worldwide, unlimited in time, free of charge. Obligations:
  attribution notice "© DLR e.V. 2010-2014 and © Airbus Defence and Space GmbH 2014-2018
  provided under COPERNICUS by the European Union and ESA; all rights reserved"; for modified
  data "produced using Copernicus WorldDEM-30 © …"; plus a no-liability sentence when
  redistributing. https://documentation.dataspace.copernicus.eu/APIs/SentinelHub/Data/DEM/resources/license/License-COPDEM-30.pdf
- *(estimate)* A 1° GLO-30 tile is 3600×3600 float32 ≈ 52 MB raw, ~20–35 MB as DEFLATE
  COG; Europe (approx. 12°W–40°E, 35°N–71°N, ~1 100 land tiles) ≈ 25–40 GB — fine for a
  build pipeline, far too big to ship in an app.

**SRTM GL1 v3 (NASA/USGS)**
- 1 arc-second (~30 m), 60°N–56°S only (**misses northern Scandinavia**), voids filled
  with ASTER/GMTED/NED, HGT format, 1°×1° tiles, ~100 GB total, requires Earthdata login.
  https://www.earthdata.nasa.gov/data/catalog/lpcloud-srtmgl1-003
- NASA data are CC0 / "no restrictions", citation requested.
  https://www.earthdata.nasa.gov/engage/open-data-services-software-policies/data-use-policy
- Flown in February 2000 — older than Copernicus and lower vertical quality in mountains.
  https://www.earthdata.nasa.gov/data/instruments/srtm

**EU-DEM 25 m** (older Copernicus product): ±7 m RMSE, covers Scandinavia north of 60°,
~1.5 GB compressed, but withdrawn from the Copernicus portal in Jan 2024 in favour of
Copernicus DEM. https://www.opentopodata.org/datasets/eudem/

### 3.2 Pre-tiled elevation (what an app can actually download on demand)

**AWS Terrain Tiles / Mapzen "Terrarium" (recommended tile source)**
- Bucket `elevation-tiles-prod` (us-east-1) with EU replica `elevation-tiles-prod-eu`
  (eu-central-1); no AWS account: `aws s3 ls --no-sign-request s3://elevation-tiles-prod/`.
  https://registry.opendata.aws/terrain-tiles/
- URL patterns: `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`,
  `…/geotiff/{z}/{x}/{y}.tif`, `…/skadi/{N|S}{y}/{N|S}{y}{E|W}{x}.hgt.gz`; terrarium is
  256 px, max zoom 15; no CDN in front (bring your own cache).
  https://github.com/tilezen/joerd/blob/master/docs/use-service.md
- Terrarium decode: `height = (R*256 + G + B/256) - 32768` (metres, Web Mercator);
  skadi = 1°×1° 16-bit HGT gzip, void −32768.
  https://github.com/tilezen/joerd/blob/master/docs/formats.md
- Composition: SRTM 30 m globally, **EU-DEM 30 m across Europe** (covers >60°N), GMTED at
  low zooms, ETOPO1 bathymetry, national LiDAR in AT/NO/UK etc. at z10+.
  https://github.com/tilezen/joerd/blob/master/docs/data-sources.md
- Attribution required: "ArcticDEM, Australia, Austria, Canada, Europe, Global ETOPO1,
  Mexico, New Zealand, Norway, United Kingdom, United States terrain data sources per
  license requirements"; EU-DEM needs "Produced using Copernicus data and information
  funded by the European Union"; SRTM "courtesy of the U.S. Geological Survey".
  https://github.com/tilezen/joerd/blob/master/docs/attribution.md
- Caveat: dataset last built ~2020 (Open Topo Data serves "Mapzen v1.1 (indexed May
  2020)"), so it predates Copernicus GLO-30.
  https://www.opentopodata.org/datasets/mapzen/

**MapTiler Terrain-RGB v2**
- Tileset id `terrain-rgb-v2`, zoom 0–14, WebP, "30 m globally, 5 m in specific areas",
  vertical resolution 6 m. https://docs.maptiler.com/schema-raster/terrain-rgb/
- Mapbox-style encoding `height = -10000 + ((R*256*256 + G*256 + B) * 0.1)`; on-prem
  planet dataset generated at zoom 0–11. https://www.maptiler.com/on-prem-datasets/dataset/terrain-rgb/
- Free plan: 100 k requests/month, MapTiler logo on map, **no commercial use**, mobile
  allowed for personal testing; Flex $30/mo for 500 k requests.
  https://www.maptiler.com/cloud/pricing/
- Terms prohibit redistribution and "manipulate or modify map content … pixels or
  underlying metadata"; attribution "© MapTiler" must stay visible.
  https://www.maptiler.com/terms/
- Note: "Vertical resolution: 6 m" is coarse for grade on gentle slopes.

**Mapbox Terrain-RGB v1 / Terrain-DEM v1**
- Terrain-RGB v1: 0.1 m increments, zoom ≤15, `https://api.mapbox.com/v4/mapbox.terrain-rgb/{z}/{x}/{y}.pngraw`,
  **frozen since 1 Dec 2021**. https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-rgb-v1/
- Terrain-DEM v1 (its replacement): zoom ≤14, "not available via the Raster Tiles API",
  only through Mapbox SDKs; mixed vertical datums.
  https://docs.mapbox.com/data/tilesets/reference/mapbox-terrain-dem-v1/
- Raster Tiles API: 750 000 requests/month free, then $0.25/1 000; device cache TTL 12 h.
  https://www.mapbox.com/pricing , https://docs.mapbox.com/api/maps/raster-tiles/
- Requires token; the useful tileset is SDK-locked. Not a fit for a Rust decoder.

### 3.3 Point-lookup web APIs (for prototyping only)

- **Open Topo Data** public API: max 100 locations/request, 1 call/s, 1 000 calls/day;
  datasets incl. `srtm30m`, `eudem25m`, `mapzen`, `aster30m`; supports `locations=` as a
  Google polyline and `samples=N` to resample a path; interpolation nearest/bilinear/cubic;
  server code MIT, self-hostable via Docker.
  https://www.opentopodata.org/ , https://www.opentopodata.org/api/ , https://github.com/ajnisbet/opentopodata
- **Open-Elevation**: `api.open-elevation.com/api/v1/lookup`, GET ≤1 024 bytes, POST
  unlimited; returns 0 m where no data; backed by CGIAR SRTM 250 m (~20 GB self-hosted);
  code GPLv2; hosted tier now "1,000 requests/month".
  https://open-elevation.com/ , https://github.com/Jorl17/open-elevation/blob/master/docs/api.md ,
  https://github.com/Jorl17/open-elevation/blob/master/docs/host-your-own.md
- **Open-Meteo Elevation API**: `/v1/elevation`, ≤100 coordinates/call, Copernicus GLO-90.
  https://open-meteo.com/en/docs/elevation-api
- **Valhalla `/height`**: samples elevation along a shape; needs local skadi tiles
  (`valhalla_build_elevation`, whole world ≈1.6 TB). https://valhalla.github.io/valhalla/concepts/elevation/

All of these are per-point HTTP calls with daily caps — a single 600 km Leg at 100 m
spacing is 6 000 points — so they are unsuitable as the production path.

### 3.4 Elevation comparison

| Source | Resolution | Europe >60°N | Format on wire | Zoom/tile | Licence | Cost |
|---|---|---|---|---|---|---|
| Copernicus GLO-30 (S3) | 30 m, <4 m LE90 | yes | COG float32, 1° | n/a | free, attribution | free |
| SRTM GL1 v3 | 30 m | **no** (≤60°N) | HGT, 1° | n/a | CC0-like | free, login |
| AWS Terrain Tiles (terrarium) | ~30 m (SRTM/EU-DEM) | yes (EU-DEM) | PNG 256 px | z≤15 | attribution list | free S3 egress |
| MapTiler terrain-rgb-v2 | 30 m, 6 m vertical | yes | WebP | z≤14 | non-commercial free | 100 k req/mo |
| Mapbox Terrain-DEM v1 | ~30 m, 0.1 m | yes | SDK-only | z≤14 | Mapbox ToS | 750 k req/mo |
| Open Topo Data | 25–30 m | eudem25m | JSON points | – | MIT server | 1 000 calls/day |

---

## 4. Sampling elevation per Leg on-device, cheaply

Three viable patterns, in increasing build effort:

**A. Terrarium tiles fetched on demand (fastest to ship)**
1. Walk the Leg geometry; compute the set of z12 (or z11) Web-Mercator tile keys it
   crosses. *(estimate)* z12 ≈ 25 m/px at 50°N, so a 256 px tile spans ~6 km; a 500 km
   Leg touches ~80–120 tiles at ~30–60 KB each → 3–6 MB, and tiles are reused across
   Legs and Plans.
2. Fetch from `elevation-tiles-prod-eu` (Frankfurt), cache on disk keyed by tile.
3. Decode PNG in Rust (`png` crate), apply `(R*256 + G + B/256) - 32768`, bilinear-sample
   at every geometry vertex, then compute grade between consecutive vertices. Decoding
   256×256 PNG is sub-millisecond; whole-Leg sampling is dominated by network.
   https://github.com/tilezen/joerd/blob/master/docs/formats.md
4. Smooth the profile (e.g. 100–200 m median) before differentiating — a raw 30 m DSM
   gives spurious ±10 % spikes at bridges and tree lines.
- Attribution block in app: the tilezen list above.

**B. Prebuilt regional elevation pack (offline-first)**
- Build pipeline (desktop): download Copernicus GLO-30 COGs for the region, resample to a
  compact int16 grid at ~1 arc-second or 90 m, tile as 1° blocks, compress. Rough sizes:
  GLO-90 int16 for Europe *(estimate)* ≈ 1 100 tiles × 1200×1200×2 B ≈ 3 GB raw, ~1–1.5 GB
  compressed; at 30 m ×9. Too large to bundle; deliver as an optional download per
  country, like map packs.
- Licence permits redistribution with the GLO-30 notice and the no-liability sentence.
  https://documentation.dataspace.copernicus.eu/APIs/SentinelHub/Data/DEM/resources/license/License-COPDEM-30.pdf
- On-device: memory-map the block, read int16 at vertex positions. Zero network.

**C. Precompute grade per edge in the routing graph (the Valhalla way — best long-term)**
- Valhalla bakes elevation into each `DirectedEdge` at tile build time: `weighted_grade()`
  (4-bit factor, "0 is a -10% grade and 15 is 15%"), `max_up_slope()` / `max_down_slope()`
  (1° precision to 16°, 4° steps to 76°). https://github.com/valhalla/valhalla/blob/master/valhalla/baldr/directededge.h
- Its build script pulls skadi HGT tiles from
  `https://elevation-tiles-prod.s3.us-east-1.amazonaws.com/skadi/{dir}/{name}.gz`
  (each HGT is exactly 25 934 402 bytes uncompressed).
  https://github.com/valhalla/valhalla/blob/master/scripts/valhalla_build_elevation
- For ABRP-native, if the Routing Engine owns a compiled road graph (issue on routing
  engine choice), the graph builder should store per-edge **cumulative climb/descent in
  metres and mean grade** (or an elevation delta per shape vertex, quantised to 1 m). Then a
  Leg's elevation profile is a sum over its edges with no runtime DEM access at all, and
  the Energy Model can also be used *inside* the router cost function. Valhalla's 4-bit
  weighted grade is too coarse for energy; keep climb/descent as int16 metres per edge.
- Cost is paid once in the graph build (needs the GLO-30 pack from B or skadi tiles), the
  app ships nothing extra beyond the graph.

**Recommendation**: start with **A** (terrarium z12 from the EU bucket, Rust decoder, disk
cache) because it needs no build pipeline; design the Leg elevation-profile API so that
**C** can replace it once the routing graph exists. Use Copernicus GLO-30 (not SRTM) for any
offline/graph build so that Norway/Sweden/Finland north of 60°N are covered.

---

## 5. Open questions for the Energy Model / Routing Engine tickets

- Wind reference height: forecasts give 10 m wind; a car sees ~1–2 m wind in a boundary
  layer with roads often sheltered. ABRP applies an empirical factor; decide ours.
- Which weather hour to use: ETA per sample point (requires the Plan's departure time and
  the Routing Engine's cumulative durations), not "now".
- Whether the Routing Engine choice (Valhalla vs. own Rust graph) lets us do pattern C
  directly — if Valhalla tiles are used on-device, its per-edge grade is already there but
  quantised.
