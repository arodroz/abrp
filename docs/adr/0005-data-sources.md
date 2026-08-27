# 5. Data sources: national open Charger datasets as Charger Packs, Open-Meteo, Copernicus GLO-30 in the pipeline

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/14

## Context

Research (#6 `docs/research/charger-data-sources.md`, #7 `docs/research/weather-elevation-sources.md`) found: AFIR Art. 20 makes static and live Charger data free via national access points (LU Chargy CC0, FR IRVE Licence Ouverte, DE BNetzA CC BY + MobiData BW, NL DOT-NL, BE transportdata.be); OpenChargeMap is a CC BY gap filler that asks high-volume users to mirror rather than call; Eco-Movement (ABRP's supplier) is a commercial contract; Open-Meteo batches many coordinates per call, CC BY, no key; Copernicus GLO-30 is a free 30 m DEM; Mapzen terrarium tiles are the runtime alternative. ADR 0001 rules out a backend; ADR 0004 has Swift fetch and Rust parse.

## Decision

1. **Static Charger store**: the batch pipeline builds a per-country **Charger Pack** from the national open datasets, with OpenChargeMap (`opendata=true`) as gap filler, normalised to an OCPI-like Charger record (connectors, `max_electric_power`, operator, access). Downloaded next to the Region Pack; parsed in Rust. OSM `amenity=charging_station` is never the source of truth. Rebuilt monthly.
2. **Live status: best-effort, display-only.** The phone polls keyless feeds along the Plan corridor — NL DOT-NL GeoJSON bbox, FR IRVE dynamic CSV, LU data.public.lu KML — at most every 5 min while a Plan is on screen, and shows availability on Charging Stops. The optimiser never consumes it. BE (per-EVSE polling) and DE (Mobilithek offers unverified) deferred. No backend poller.
3. **Weather**: Open-Meteo `/v1/forecast`, one batched call per Plan (20–40 samples along the route; hourly `temperature_2m`, `wind_speed_10m`, `wind_direction_10m`, 3 days). Swift fetches behind a `WeatherProvider` protocol (WeatherKit is the documented swap-in); Rust receives per-sample temperature and wind vector. Offline: user-set default temperature, zero wind.
4. **Elevation**: the build pipeline samples **Copernicus GLO-30** (public COGs on AWS) at every graph node, stores i16 metres per node in the Region Pack, smooths (100–200 m median), and derives per-edge grade. The app never fetches elevation; the SoC/elevation chart reads the same per-node values.
5. **Attribution**: an in-app "Data sources" screen and a repo `NOTICE.md` listing every source and licence: OpenStreetMap ODbL, Protomaps, MapLibre BSD-2, Chargy CC0, IRVE Licence Ouverte 2.0, Bundesnetzagentur CC BY 4.0, MobiData BW DL-DE-BY 2.0, OpenChargeMap CC BY 4.0 with per-provider credits, Open-Meteo CC BY 4.0, Copernicus DEM (ESA/Airbus notice), NDW and transportdata.be once their wording is confirmed.

## Consequences

- Four verifications remain before shipping: NDW DOT-NL licence text, BE dataset licences, Mobilithek AFIR charging offers and licences, Chargy `my.chargy.lu` key or data.public.lu cadence — tracked as a wayfinder task.
- The pipeline grows three jobs: Region Pack (graph + elevation), Charger Pack, Map Pack extract.
- Live status as an optimiser input would require a backend poller and normalisation to the OCPI status enum — deliberately left in the fog.
- Reversal: swapping a weather or Charger source is cheap behind the protocol / pack format; moving elevation to runtime would be a pack-format change.
