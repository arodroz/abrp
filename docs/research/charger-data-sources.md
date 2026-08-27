# Charger data sources — survey (issue #6)

Research note, 27 Aug 2026. Question: which datasets can feed the app's **Charger** objects
(location, connectors, max power, operator) for Luxembourg + BE/NL/FR/DE, and whether
**live status** (real-time availability per connector) is achievable for a personal, non-commercial
project.

Vocabulary follows `CONTEXT.md`: *Charger* = physical charging location with connectors, max power
and operator; *Charging Stop* = a Charger chosen by the planner. "EVSE" below is the OCPI/AFIR term
for one charge point inside a Charger.

Method: primary sources only (portals, API specs, licence pages). Where a portal blocked automated
fetches (EUR-Lex, data.public.lu, chargemap.com) this is flagged and the fallback source is named.

---

## 1. Summary table

| Source | Coverage LU / BE / NL / FR / DE | Connector + power quality | Live status | Licence | Cost | API shape |
|---|---|---|---|---|---|---|
| OpenChargeMap | Global, crowdsourced + imports; LU gaps reported | Connections with `ConnectionTypeID`, `PowerKW`, `Amps`, `Voltage`, `CurrentTypeID`, `Quantity` | **No** (StatusType = "generally operational", editorial) | CC BY 4.0 for user data; imported data keeps provider licence (`opendata=true` filter) | Free, API key | REST `GET /v3/poi` (bbox, radius, polyline, `modifiedsince`), fair-use, mirror encouraged |
| Eco-Movement | 80+ countries, 1.8 M+ connectors, "all (semi)public networks" | OCPI 2.2 Location/EVSE/Connector | **Yes** (push within ms, 90 %+ dynamic rate) | Commercial contract | Paid, quote-based; no free tier | OCPI 2.2 pull/push + `prices` endpoint |
| Chargeprice | Europe-focused tariff DB | `plug`, `power`, `energy_type`, `count` per charge point | **Partial** (`available_count`, nullable) | Contract; demo non-commercial only, no server caching | Free demo, commercial on quote | REST JSON:API, 1000 req/min |
| NOBIL | Norway + Sweden only (DK/FI/IS dropped Oct 2025) | Documented in v3 PDF | Yes (realtime added) | CC BY 4.0 (attribution Enova) | Free, key on request | REST v3 |
| Chargemap | FR-centric, EU | n/a | n/a | Proprietary | B2B (Chargemap Business) | **No public third-party API found** |
| **LU — Chargy open data** | Chargy public network (760+ stations) | Type 2 22 kW + SuperChargy; connector type & max speed per charge point | **Yes** (available / occupied, ~5 min KML refresh) | CC0 1.0 | Free | KML on data.public.lu; direct my.chargy.lu KML needs key (401 without) |
| **FR — transport.data.gouv.fr IRVE** | France, 1 500+ producers, daily consolidation | `puissance_nominale`, `prise_type_2/combo_ccs/chademo/ef/autre`, `nbre_pdc` | **Yes** (dynamic CSV, `etat_pdc`, `occupation_pdc`, ~seconds cache) | Licence Ouverte 2.0 (Etalab) | Free, no key | CSV/GeoJSON bulk downloads |
| **DE — BNetzA Ladesäulenregister** | Germany, registered operators (LSV), not exhaustive | `Nennleistung Ladeeinrichtung`, `Steckertypen`, `Betreiber`, coordinates | **No** (monthly-ish snapshot) | CC BY 4.0 (attribution Bundesnetzagentur.de) | Free | CSV/XLSX (51 MB / 28 MB) |
| **DE — Mobilithek (NAP)** | Germany; per-CPO or aggregated AFIR offers | DATEX II AFIR Recharging profile | **Yes** (≤1 min, delta + snapshot) | Offer-specific, CC0 recommended | Free; open offers need no account, restricted ones need org registration | DATEX II (mandatory since 14 Apr 2026) |
| **DE — MobiData BW (OCPDB)** | Baden-Württemberg + BNetzA register + operator feeds | OCPI 3.0 Locations/EVSE/Connector | Yes (dynamic status) | Datenlizenz Deutschland Namensnennung 2.0 | Free, no auth | OCPI 3.0 + DATEX II endpoints, CSV, WFS |
| **NL — NDW DOT-NL (NAP)** | Netherlands, connected CPOs | OCPI 2.2.1 Locations | **Yes** (dynamic status + tariffs, ≤1 min from CPOs) | "Open data", licence not stated on portal | Free, no registration to consume | GeoJSON bbox API (10 req/s, 1°² bbox, 1 000 features) + gzip bulk OCPI/GeoJSON |
| **BE — transportdata.be (NAP)** | Belgium, "selected CPOs" (Allego, bp pulse, Blink, Chargepoint, Circle K…) via Eco-Movement + per-CPO offers | DATEX II v3 JSON static | **Yes** (DATEX II v3 status per EVSE, ≤1 min) | Not stated per dataset | Free per portal; Eco-Movement "Overzicht" fee-based | REST `/datex2/v1/status/{evse_id}` + static JSON |
| **BE — Flanders MOW laadpunten** | Flanders only | ID, operator, speed & power, current/connector type | No (monthly) | Open Data License Vlaanderen | Free | WFS/WMS/KML/GML |
| OpenStreetMap | Global; LU has recurring Chargy import | `socket:<type>`, `socket:<type>:output`, `capacity`, `operator` — quality uneven | **No** | ODbL | Free | Overpass API (<100 queries / 10 MB per day for regular apps) or planet extracts |
| OCPI (protocol) | n/a — protocol, not a dataset | Connector `standard`, `power_type`, `max_electric_power`, `max_voltage`, `max_amperage` | EVSE `status` enum (AVAILABLE, CHARGING, …) | Spec published on GitHub | n/a | Peer-to-peer or hub; requires credentials handshake with each CPO/aggregator |

---

## 2. Per-source findings

### 2.1 OpenChargeMap (OCM)

- API base `https://api.openchargemap.io/v3/`; `/poi` for sites, `/referencedata` for lookup tables. API key mandatory (`X-API-Key` header or `key=` param), obtained free by registering an application; custom `User-Agent` requested. Fair-use policy: no duplicate queries, throttle, automated bans possible; "If you need to make a high volume of queries … host your own API mirror or import the data". Data "has mixed licensing"; "filter by `opendata=true`" to get only open-licensed records. — OpenAPI spec: <https://raw.githubusercontent.com/openchargemap/ocm-docs/master/Model/schema/ocm-openapi-spec.yaml>
- `/poi` parameters include `boundingbox`, `polyline` (route corridor), `distance`, `countrycode`, `connectiontypeid`, `levelid`, `operatorid`, `statustypeid`, `modifiedsince`, `compact`, `verbose`, `maxresults` (default 10). — same spec.
- Connection object: `ConnectionTypeID`, `StatusTypeID`, `LevelID`, `Amps`, `Voltage`, `PowerKW`, `CurrentTypeID`, `Quantity`. `StatusType` "indicates whether it is generally operational" (`IsOperational`, `IsUserSelectable`) — an editorial state, **not real-time occupancy**. — same spec.
- Licence: "Data contributed to us by our users which we then redistribute is licensed under a Creative Commons Attribution 4.0 International (CC BY 4.0)"; third-party imported data keeps its provider's licence; apps must show "the appropriate Data Provider attribution (including license terms) … visible to the end user". — <https://openchargemap.org/develop> and <https://openchargemap.io/about/terms>
- Non-commercial, non-profit service; 4 000+ registered API developers. — <https://openchargemap.org/develop>
- Luxembourg coverage gaps have been raised on the OCM forum ("Missing chargers in Luxembourg"). — <https://community.openchargemap.org/t/missing-chargers-in-luxembourg/489> (page returned 403 to automated fetch; title only)
- ABRP itself lists OCM and Eco-Movement as its two Charger sources — see `docs/abrp-tech-stack.md`.

### 2.2 Eco-Movement (commercial)

- "80+ countries", "1,8M+ connectors", "real-time EVSE status updates within milliseconds", "90%+ dynamic data rate", "preferred connector for Apple Maps". — <https://www.eco-movement.com/>
- Data API is OCPI 2.2 (legacy 2.1.1): `locations`, `evse`, `tariffs`, `prices` (MSP + CPO ad-hoc prices at connector level), `credentials`. Real-time via PATCH push (preferred; requires you to host a receiving endpoint and do the credentials handshake) or GET pull with `date_from`/`date_to`; max 5 simultaneous GETs recommended. Token auth; OAuth 2.0 on request. — <https://developers.eco-movement.com/docs/data-api-user-guide>
- Pricing: no published price list; "Request a demo"; marketplace listing describes one-off / monthly / yearly / usage-based licences, custom-priced by geography and usage, free samples on request. — <https://datarade.ai/data-providers/eco-movement/profile>, <https://marketplace.eiturbanmobility.eu/products/eco-movement-ev-charging-station-location-tariffs-data-real-time-api> (secondary, marketplace pages)
- Note: Eco-Movement is also the publisher of the Belgian NAP AFIR datasets (§2.9), which are the free, regulated slice of the same data.

### 2.3 Chargeprice

- Docs: <https://github.com/chargeprice/chargeprice-api-docs>. Endpoints v1 `charging_stations` (index/show), `charge_prices`, `tariffs`, `companies`, v2 `vehicles`. "Free demo API access" via form, demo data "limited", commercial use prohibited; commercial licences via sales@chargeprice.net. OCPI export on special request.
- `charging_stations` supports bbox filters `filter[latitude.gte|lte]`, `filter[longitude.gte|lte]`; each charge-point group returns `plug`, `power` (kW), `energy_type` (AC/DC), `count`, `available_count` ("ready to use and not occupied", nullable). — <https://chargeprice.github.io/chargeprice-api-docs/api/v1/charging_stations/index.html>
- Terms: 1000 req/min (5-min window), 400 req/min for tariff-details and charge-prices; client cache ≤24 h; "explicitly prohibited to cache any price or charging station related data" server-side; no redistribution without written consent; €10 per record penalty for misuse. — <https://chargeprice.github.io/chargeprice-api-docs/terms.html>
- Verdict: usable as a **tariff** source, not as the primary Charger store (no caching allowed).

### 2.4 NOBIL

- Real-time supported; "Starting in October 2025, the NOBIL API will no longer include charging stations from Denmark, Finland, and Iceland" → Norway + Sweden only. — <https://info.nobil.no/>
- API key on request; test client `nobil.no/api/client/search_apiVer3.php`; docs PDF `API_NOBIL_Documentation_v3_20260816.pdf`; licence CC BY 4.0, attribution Enova. — <https://info.nobil.no/api>
- **Out of scope** for LU/BE/NL/FR/DE; listed for completeness.

### 2.5 Chargemap

- chargemap.com pages answer HTTP 402 to automated fetches; no developer portal or public API documentation was found by search. Only a B2B "Chargemap Business" data export API for fleet customers is advertised. — <https://www.chargemap-business.com/en/charging-management-software>
- Chargemap publishes an open-source OCPI PHP library but that is a client library, not data access. — <https://github.com/ChargeMap/ocpi-protocol>
- Verdict: **not a candidate** for a third-party personal app.

### 2.6 Luxembourg — Chargy open data (data.public.lu)

- Dataset "Bornes de chargement publiques pour voitures électriques" on data.public.lu, licence **CC0 1.0 Universal**, KML; the OSM import page documents the fields: station name, address, coordinates, device count, per-charge-point connector type and max speed (22 kW standard), and **occupancy status**. — <https://wiki.openstreetmap.org/wiki/Import/Catalogue/Chargy_Import_Luxembourg> (data.public.lu itself timed out on every automated fetch; dataset URL `https://data.public.lu/fr/datasets/bornes-de-chargement-publiques-pour-voitures-electriques/`)
- Converter used by the OSM import confirms "published under the Creative Commons Zero License" and availability (occupied/available) in the KML. — <https://github.com/DavidMoraisFerreira/ChargingStations2GeoJson>
- Geoportail metadata (Creos Luxembourg, Electro-mobility): "Information about availability is shown in real-time. Blue dot = available, Green dot = occupied"; WMS layer 1381 at `https://wms.geoportail.lu/public_map_layers/service`. — <https://geocatalogue.geoportail.lu/geonetwork/geoportail-lu/api/records/801d732a-c285-4fcd-a043-c5220d6d7eef?language=eng>
- The direct feed `https://my.chargy.lu/b2bev-external-services/resources/kml` returns `401 Unauthorized` without a key (verified 27 Aug 2026); the OSM community reports it refreshes every 5 minutes. — <https://community.openchargemap.org/t/missing-chargers-in-luxembourg/489> (forum, secondary)
- Network size: "plus de 760 bornes publiques dont 86 SuperChargy". — <https://transports.public.lu/fr/conduire/bornes-charge-electriques/publiques-chargy.html>
- Luxembourg NAP for AFIR: the transport ministry states operators must share static and dynamic data "via le point d'accès national", modalities by grand-ducal regulation; no separate AFIR portal URL found beyond data.public.lu. — <https://transports.public.lu/fr/plus/services/bornes-recharge-vehicules-electriques.html> (403 to automated fetch; content from search snippet)

### 2.7 France — transport.data.gouv.fr IRVE

- Legacy consolidated file: national IRVE registry, daily, schema v2.3.1, Licence Ouverte (Etalab), CSV + GeoJSON; ~1 000 validation errors, 97 % availability; migrates to the new base by 31 Dec 2026. — <https://transport.data.gouv.fr/datasets/fichier-consolide-des-bornes-de-recharge-pour-vehicules-electriques>
- New "[BETA] Base Nationale des IRVE": static (deduplicated, nightly) `https://transport.data.gouv.fr/resources/84013/download`, **dynamic CSV** `https://transport.data.gouv.fr/resources/84098/download` (real-time, few-seconds cache), 1 500+ producers, Licence Ouverte 2.0. — <https://transport.data.gouv.fr/datasets/beta-bases-nationales-des-points-de-recharge-pour-vehicules-electriques-en-france-irve>
- Static schema fields: `id_pdc_itinerance`, `puissance_nominale`, `prise_type_ef/2`, `prise_combo_ccs`, `prise_chademo`, `prise_autre`, `nbre_pdc`, `gratuit`, `paiement_acte`, `tarification`, `accessibilite_pmr`, `horaires`, `coordonneesXY`. — <https://schema.data.gouv.fr/etalab/schema-irve-statique/>
- Dynamic schema: `id_pdc_itinerance`, `etat_pdc`, `occupation_pdc`, `horodatage`, `etat_prise_*`; every state change must be pushed. — <https://schema.data.gouv.fr/etalab/schema-irve-dynamique/2.3.1/>, <https://doc.transport.data.gouv.fr/type-donnees/infrastructures-de-recharge-de-vehicules-electriques-irve/donnees-dynamiques>

### 2.8 Germany — Bundesnetzagentur register, Mobilithek, MobiData BW

- BNetzA Ladesäulenregister: CSV (51 MB) / XLSX (28 MB) dated 2026-07-28 at `https://data.bundesnetzagentur.de/Bundesnetzagentur/DE/Fachthemen/ElektrizitaetundGas/E-Mobilitaet/Ladesaeulenregister_BNetzA_2026-07-28.csv`; fields include rated power, plug types, operator, coordinates; **CC BY 4.0**, attribution "Bundesnetzagentur.de"; "Die LSV ermöglicht keine lückenlose Erfassung" (not exhaustive); no live status. — <https://www.bundesnetzagentur.de/DE/Fachthemen/ElektrizitaetundGas/E-Mobilitaet/Ladesaeulenkarte/start.html>
- Known geocoding errors in the register (stations placed in the wrong city). — <https://medium.com/comsystoreply/the-most-complete-map-of-charging-stations-1ebbf91e4ef3> (secondary)
- Mobilithek is the German NAP; use is free; open-data offers download without registration; restricted offers need user + organisation registration and a subscription approved by BASt. — <https://www.bast.de/DE/Publikationen/Daten/VerhaltenundSicherheit/MDC/Datenbezug/Datenbezug_node.html>
- AFIR feeds on Mobilithek: per-CPO or aggregated offers; dynamic data "no later than one minute after the triggering event", delta mechanism with full snapshot every 3 500 deltas or 6 h; DATEX II "AFIR-DATEX-II-Recharging Profil" mandatory from 14 Apr 2026; CC0 "strongly recommended". — <https://github.com/MobilithekDE/AFIR-DATEX-II-Recharging-Profil/wiki/FAQ--zur-Datenbereitstellung-durch-Betreiber-%C3%B6ffentlich-zug%C3%A4nglicher-Ladeinfrastruktur-im-DATEX%E2%80%90II%E2%80%90Standard>
- mobilithek.info is a JS single-page app; its catalogue could not be enumerated by automated fetch — the number of charging offers and their individual licences must be checked by hand in a browser.
- MobiData BW "Gebündelte Daten E-Ladesäulen": BNetzA register + operator feeds (EnBW, Tesla, Eco-Movement, Stadtwerke…) + Swiss operators; OCPI 3.0 "OCPDB-API" since Mar 2026, DATEX II endpoints since Apr 2026, dynamic status included; Datenlizenz Deutschland Namensnennung 2.0; no auth mentioned. — <https://mobidata-bw.de/dataset/e-ladesaulen>
- Nationale Leitstelle: MOBIDROM (NRW) and MobiData BW act as regional aggregators feeding Mobilithek. — <https://nationale-leitstelle.de/en/bestand-ausbau/afir/>

### 2.9 Belgium — transportdata.be (NAP ITS), Flanders MOW

- transportdata.be is "Belgium's national access point for ITS"; AFIR data categories "should be registered on the NAP ITS". — <https://www.transportdata.be/>, <https://transportdata.be/pages/information-for-dataproviders>
- Eco-Movement publishes: "Public charging infrastructure dynamic dataset selected CPOs (DATEX II)" — "Real-time EVSE availability/status and ad-hoc price, updated within 1 minute", DATEX II v3 JSON, resource `https://nap-be.eco-movement.com/datex2/v1/status/{evse_id}`, no registration/fee mentioned; a matching static dataset; and "Overzicht laadpunten" (JSON, every minute, availability per charge point, fee depends on scope, contact required). — <https://transportdata.be/dataset/public-charging-infrastructure-dynamic-dataset-selected-cpos-datex-ii>, <https://transportdata.be/organization/eco-movement>, <https://transportdata.be/nl/dataset/overzicht-laadpunten>
- Other CPO offers: EnergyVision (AFIR/DATEX II), Road (`https://roaming.road.io/files/…/locations.json`, JSON, open download), Group INDIGO, Monta. — <https://www.transportdata.be/en/dataset>, <https://transportdata.be/nl/dataset/road-public-charging-network>
- Licences are "not specified" on the charging datasets; portal-wide, CC0 (48) and CC BY 4.0 (27) dominate. — <https://www.transportdata.be/en/dataset>
- Flanders regional: "Laadpunten voor elektrische voertuigen" (Dept. MOW) — all public/semi-public connectors with ID, operator, access, speed/power, current & connector type, coordinates; monthly; WFS/WMS/KML/GML; Open Data License Vlaanderen. — <https://data.gov.be/nl/datasets/051113b5-2055-4abc-a2a8-8422afadea02>
- Note: the per-EVSE `status/{evse_id}` shape means live status for Belgium requires knowing EVSE IDs from the static file first, then polling per EVSE — costly for corridor-wide refresh; check whether a bulk endpoint exists (not documented on the dataset page).

### 2.10 Netherlands — NDW DOT-NL (NAP)

- DOT-NL = Dutch NAP for charging under AFIR; static (≤24 h), dynamic availability + tariffs (≤1 min); OCPI 2.2.1 today, OCPI 2.3 planned, DATEX II translation provided; "free of charge and openly accessible"; consuming needs no registration. — <https://docs.ndw.nu/en/faq/DOT-NL/>, <https://www.ndw.nu/producten-en-diensten/dataportalen/dot-nl>
- Consumer endpoints: GeoJSON bbox `https://dotnl.ndw.nu/api/rest/geojson/dynamic-road-status/charge-point-data/v1/features?bbox=…` (max 10 req/s → 429, bbox ≤1°², ≤1 000 features); bulk `https://opendata.ndw.nu/charging_point_locations_ocpi.json.gz`, `charging_point_tariffs_ocpi.json.gz`, `charging_point_locations.geojson.gz`; APIs "still under development". — <https://docs.ndw.nu/en/data-uitwisseling/interface-beschrijvingen/dafne-api/dafne_api_consumer_pull/>
- Licence text is not stated on the docs pages (described only as "open data") — confirm before shipping.
- Municipal datasets (e.g. Eindhoven) are static only; they point to oplaadpalen.nl for occupancy. — <https://data.overheid.nl/en/dataset/22449-oplaadpalen>

### 2.11 OCPI (protocol)

- Latest 2.3.0; modules Locations, Sessions, Tariffs, CDRs, Tokens, Commands…; peer-to-peer or hub, credentials/registration handshake. — <https://github.com/ocpi/ocpi>
- EVSE `status` enum: AVAILABLE, BLOCKED, CHARGING, INOPERATIVE, OUTOFORDER, PLANNED, REMOVED, RESERVED, UNKNOWN. Connector: `standard` (IEC_62196_T2, CHADEMO, …), `format` SOCKET/CABLE, `power_type`, `max_voltage`, `max_amperage`, `max_electric_power` (W). Receivers get Locations by push (PUT/PATCH, preferred) or pull (GET, "not too often"). — <https://raw.githubusercontent.com/ocpi/ocpi/master/mod_locations.asciidoc>
- Practical: raw OCPI feeds are bilateral (a CPO or hub must onboard you). For a personal project the only OCPI access without a contract is via NAPs that re-publish in OCPI (NL DOT-NL, MobiData BW).

### 2.12 OpenStreetMap `amenity=charging_station`

- Tags: `socket:<type>` (count), `socket:<type>:output` (kW), `operator`/`brand`, `capacity`, `access`, `opening_hours`; `man_made=charge_point` for individual EVSEs; wiki warns `capacity` is ambiguous and `network` was historically confused (~17 % of entries). ODbL. — <https://wiki.openstreetmap.org/wiki/Tag:amenity%3Dcharging_station>
- Overpass main instance: for regular apps "fewer than 100 queries and 10 MB per day", set `User-Agent`, back off 30 s on 429/406; server "currently overloaded"; self-host or use extracts for production. — <https://wiki.openstreetmap.org/wiki/Overpass_API>
- Luxembourg: Chargy data is imported into OSM on a recurring, manually validated basis (CC0 → ODbL compatible). — <https://wiki.openstreetmap.org/wiki/Import/Catalogue/Chargy_Import_Luxembourg>
- No live status by design.

### 2.13 AFIR (Regulation (EU) 2023/1804) Article 20

- EUR-Lex refused every automated fetch (empty body). Official text: <https://eur-lex.europa.eu/legal-content/EN/TXT/?uri=CELEX%3A32023R1804> (consolidated: <https://eur-lex.europa.eu/legal-content/EN/TXT/HTML/?uri=CELEX:02023R1804-20250414>). The following is from implementing bodies, to be verified against the text:
  - Since 14 Apr 2025 CPOs must provide static and dynamic data "free of charge and without discrimination" via an API to the NAP; DATEX II mandatory from 14 Apr 2026. — <https://nationale-leitstelle.de/en/bestand-ausbau/afir/>
  - Static ≤24 h: location, connectors, power, hours, access; dynamic ≤1 min: availability/occupancy, operating status, ad-hoc price (free chargers exempt from price). — <https://www.innobu.com/en/articles/datex-ii-charging-data-obligation-2026.html>, <https://www.greenflux.com/expertise/blogs/afir-national-access-point-nap-reporting-complete-guide-charge-point-operators/> (industry guides, secondary)
- Consequence for this project: every country in scope now has a legally mandated, free, ≤1-minute live-status feed at its NAP. The NAPs differ in maturity and shape (FR: bulk CSV; NL: OCPI/GeoJSON; BE: DATEX II per-EVSE REST; DE: DATEX II offers on Mobilithek, plus OCPI via MobiData BW; LU: Chargy KML on data.public.lu).

---

## 3. Resolution

**Static Charger store (recommended stack)**

1. Build the Charger table from the **national open datasets**, not from OCM alone: LU Chargy (CC0), FR IRVE static (Licence Ouverte 2.0), DE BNetzA register (CC BY 4.0) merged with MobiData BW OCPI 3.0, NL DOT-NL bulk OCPI (open), BE transportdata.be static DATEX II + Flanders MOW WFS. All are free, attribution-only or public-domain, and bulk-downloadable — no rate-limit exposure.
2. Use **OpenChargeMap** (`opendata=true`, CC BY 4.0) as the cross-border gap filler and for photos/comments, mirroring via GitHub clone rather than live API calls, as OCM itself asks for high-volume use.
3. Use **OSM** for map rendering context only; its connector/power tags are too uneven to be the source of truth for `max power`.
4. Skip Chargemap (no API), NOBIL (Nordics only), and Chargeprice as a Charger store (caching prohibited) — Chargeprice remains an option for tariffs later.

**Live status — achievable for a personal project: yes, country by country, without Eco-Movement**

- NL: GeoJSON bbox API at 10 req/s, no key. FR: dynamic CSV, refresh every few seconds, no key. BE: Eco-Movement's NAP `status/{evse_id}` endpoint, no key mentioned (per-EVSE polling, verify bulk). DE: DATEX II via Mobilithek open offers or OCPI 3.0 via MobiData BW (BW + BNetzA base; national completeness unverified). LU: Chargy KML with occupancy, ~5 min, CC0 — direct my.chargy.lu feed needs a key (ask Creos/Chargy; the data.public.lu mirror is keyless).
- Cost of doing it "properly" (single pan-European feed, push, SLAs) is Eco-Movement's commercial contract; that is what ABRP pays for. For a learning project, a small backend that polls the five NAP feeds and normalises them to the OCPI `status` enum is the realistic path; polling straight from the iOS app is feasible for NL/FR but would hit OCM/Overpass fair-use limits, so those two should never be queried live.

**Open verifications before committing**

- Read AFIR Art. 20 text on EUR-Lex in a browser (automated fetch blocked).
- Open mobilithek.info in a browser, count AFIR charging offers and note per-offer licences.
- Confirm NDW DOT-NL licence wording and BE dataset licences (both "not specified" on portals).
- Request Chargy `my.chargy.lu` API key or confirm data.public.lu KML refresh cadence.

---

## 4. Licence & access verification (issue #21, 27 Aug 2026)

Live probes from the pipeline machine (`curl -I` / `GET`, no credentials unless noted).

| Feed | URL | Access | Licence (verified) | Observed |
|---|---|---|---|---|
| **NL — NDW DOT-NL bulk** | `https://opendata.ndw.nu/charging_point_locations_ocpi.json.gz` (18.3 MB), `…/charging_point_locations.geojson.gz` (4.9 MB), `…/charging_point_tariffs_ocpi.json.gz` | **Keyless**, HTTP 200, `Accept-Ranges: bytes` | Portal lists no per-file licence. `https://www.ndw.nu/copyright`: "Creative Commons Zero (CC0)" applies to NDW content unless stated otherwise (exception: photos/videos/infographics). **Treat as CC0; attribute NDW anyway.** | `Last-Modified` within the same minute as the request → regenerated ~every minute |
| **LU — Chargy KML** | `https://my.chargy.lu/b2bev-external-services/resources/kml?API-KEY=486ac6e4-…` (key published on data.public.lu) | **Works with the published key**, HTTP 200, 693 KB | data.public.lu dataset *Bornes de chargement publiques pour voitures électriques* (org Chargy): **CC-Zero**, frequency "continuous" | 527 `<Placemark>` (482 `#AVAILABLE`, 45 `#UNAVAILABLE`) → the KML **carries live status**; no separate key request to Creos needed |
| **LU — Eco-Movement multi-operator DATEX II** | `https://api.eco-movement.com/api/nap/datexii/locations?token=S76E…` (token published on data.public.lu) | Works, HTTP 200, 881 KB | data.public.lu: licence **not specified**, frequency quarterly, resource updated 18 Feb 2026 | static only; use as LU gap filler after Chargy |
| **BE — Road public charging network** | `https://roaming.road.io/files/9ef09c78-2666-418a-aa45-4f2261e2e305/locations.json?force=true` | **Keyless** (GET; HEAD returns 405), 5.2 MB OCPI Locations JSON | **Not specified**: portal licence field empty *and* CKAN API `license_id: null` (browser + API, 27 Aug) | 3 387 BE locations with EVSE status fields |
| **BE — Eco-Movement static/dynamic DATEX II** | `https://nap-be.eco-movement.com/datex2/v1/locations` | **HTTP 401** — key required, contrary to the "free per portal" note in §2.9 | **Not specified**: CKAN API `license_id: null` for both static and dynamic datasets (27 Aug) | contact support@eco-movement.com for NAP credentials (draft on ticket #21) |
| **BE — EnergyVision AFIR** | dataset `energyvision-public-charging-network-locations-afir-ocpi-2-2-1` (the earlier 404 was a wrong slug — it's **OCPI 2.2.1**, not DATEX II) | **Not open**: resource URL on the portal literally reads "Communicated via email" | **Not specified**: CKAN API `license_id: null` (27 Aug) | skip; Road + OCM cover BE static (ADR 0005) |
| **DE — Mobilithek, BNetzA aggregate** | offer 951517095896416256 "Bundesnetzagentur Liste der Ladesäulen aus Webserviceschnittstelle" (NOW GmbH / Nationale Leitstelle) → direct CSVs `https://d1269bxe5ubfat.cloudfront.net/bnetza-api/data/bnetza_api_{ladestation,ladepunkt,stecker}000.csv` | **Keyless**, HTTP 200 (46.8 / 27.8 / 21.2 MB); "Datenzugriff nur mit Abonnement: Nein"; update interval "kontinuierlich" | **CC BY 4.0** with prescribed Quellenvermerk: „Bundesnetzagentur Liste der Ladesäulen aus Webserviceschnittstelle" / NOW GmbH (Nationale Leitstelle Ladeinfrastruktur) (year) under CC BY 4.0 | verified in a browser session, 27 Aug; HVD-tagged; national coverage, all 16 Länder |
| **DE — Mobilithek AFIR offers** | search "AFIR": **70 offers** (66 open-data, 4 licence-free; 58 brokered / 12 not); search "Ladepunkte": 22 offers | **No national AFIR aggregate exists** — per-CPO static+dynamic DATEX II v3 pairs (VW Group Charging, Audi, GP JOULE, Monta, Eco-Movement, EnBW, ELU, Road, …). Brokered offers: "Einzeldatenzugriff für anonyme Nutzer: Nein" → free Mobilithek account + subscription per feed | offer-level "Lizenz, freie Nutzung/Open Data" class; per-offer standard licence varies | Road B.V. also publishes its DE AFIR static+dynamic here (offers 1021366453630017536 / 1021364001002217472) |

Consequences for ADR 0005: NL and LU live status are confirmed keyless (LU via the published key, refresh continuous);
BE live status through Eco-Movement needs credentials → BE live stays **off** for v1, BE static comes from Road (keyless) + OCM.
Belgian licence texts are now confirmed **absent** (portal fields empty and CKAN API `license_id: null` on all four datasets,
27 Aug 2026 browser + API session) — no login required to establish this; the portal simply doesn't declare licences.
Road usage rests on the feed being the mandated AFIR Art. 20 NAP publication; a licence clarification can ride along in the
Eco-Movement credentials e-mail if ever sent. DE static is settled: the BNetzA/NOW aggregate is keyless CC BY 4.0 with a
prescribed attribution string; DE *live* via Mobilithek means per-CPO brokered subscriptions (free account, no anonymous
access, no national aggregate) — consistent with live status staying display-only/off beyond NL+LU for v1.
