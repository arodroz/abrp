# ABRP iPhone app — planner inputs, outputs, UX, premium split, data sources, feature inventory

Research note for GitHub issue #2 (wayfinder RESEARCH ticket). Terms follow `CONTEXT.md`
(Plan, Leg, Charging Stop, Charger, SoC, Vehicle Model, Energy Model, Charging Curve,
Routing Engine). Attributed components (OSM, MapTiler, OpenWeather, OpenChargeMap,
Eco-Movement, Valhalla, OSRM, …) are already listed in `docs/abrp-tech-stack.md` and are
not repeated here.

Date of research: 2026-08-27. Every claim carries the URL it was read from. The Postman
collections were read through Postman's public documenter JSON endpoint
(`https://documenter.gw.postman.com/api/collections/7396339/<id>`), the v2 spec from
`https://api.iternio.com/swagger-ui/spec/prod/IternioPlanning.out.yaml` (linked from the
Swagger UI's `swagger-initializer.js`). No vehicle model data was copied — only the *shape*
of the vehicle-library record is described.

Source tiers used:

- **Primary (Iternio)**: abetterrouteplanner.com marketing pages, help centre
  (`abrp.featurebase.app`), changelog, App Store listing, Planning API v1 (Postman) and
  v2 (OpenAPI), Telemetry API, deep-links doc, `iternio/abrp-translations` on GitHub.
- **Secondary (reviews / user voice)**: App Store reviews, ev-tips.com, upperinc.com.
  Used only for the "distinctive value" section and flagged as such.

---

## 1. App identity and pricing (App Store)

- Developer **Iternio Planning AB**; iOS version **7.1.5** (11 Aug 2026), 6.5K ratings,
  4.7/5. In-app purchases: **$4.99/month, $49.99/year**. Store copy: "the world's most
  respected service to plan, learn and dream about Electric Vehicles"; supports "every
  major EV in the market"; works as real-time navigation with replanning.
  <https://apps.apple.com/us/app/a-better-routeplanner-abrp/id1490860521>
- Web pricing: **€5/month**, 14-day free trial (web-only), monthly or annual; annual is
  cheaper per month.
  <https://abetterrouteplanner.com/home>,
  <https://abrp.featurebase.app/en/help/articles/9883175-upgrade-to-abrp-premium>
- Client stack is Expo / React Native (see `docs/abrp-tech-stack.md`); the v7.0 changelog
  confirms "CarPlay and Android Auto now share a unified codebase".
  <https://abrp.featurebase.app/changelog>

---

## 2. Planner inputs

The help centre's *Plan Settings* collection (29 articles) is the authoritative list of
user-facing inputs; the Planning API v1 `plan` endpoint exposes the same parameters with
units and defaults. Both are cited per row.

Help collection index: <https://abrp.featurebase.app/en/help/collections/7246826-plan-settings>
API v1 `plan`: <https://documenter.getpostman.com/view/7396339/SWTK3YsN>

### 2.1 Vehicle

| Input | App wording (help centre) | API v1 parameter | Notes / source |
|---|---|---|---|
| Vehicle Model | "Select and save a vehicle" → Settings → Vehicle; saved to a "garage"; multiple saved vehicles = Premium | `car_model` (typecode, **mandatory**) | Typecode is hierarchical (`manufacturer:model:year:battery:trim`) with options appended after `:` (e.g. heat pump, wheels). Each library record carries `ref_cons` (Wh/km at 110 km/h), `rec_max_speed`, `fast_chargers`, `rec_fast_chargers`, `level2_chargers`, `usable_battery_wh`, `maturity` (`mature`/`alpha`/`beta`) and per-option `ref_cons_delta`. <https://abrp.featurebase.app/en/help/articles/5577371-select-and-save-a-vehicle>, <https://documenter.getpostman.com/view/7396339/SWTK3YsN> (`get_vehicle_library`) |
| Model maturity label | "Estimate" = modelled manually, close to EPA/WLTP; no label ("Release") = "modeled based entirely on real-world data" | `maturity` | <https://abrp.featurebase.app/en/help/articles/2379287-what-do-the-estimate-labels-on-vehicle-models-mean> |
| Reference consumption | "base value measured at a constant speed of 110 km/h (or 65 mph) in near-perfect conditions"; shown as Wh/km@110 (100–350) or km/kWh@110 (2–5); default model value, or **calibrated** from a live-data link | `ref_consumption` [Wh/km @ 110 km/h] overrides model default | <https://abrp.featurebase.app/en/help/articles/3305478-reference-consumption> |
| Battery degradation | Settings → Vehicle → Battery degradation: assumed loss vs. brand-new | v2 `vehicle.degradation` | <https://abrp.featurebase.app/en/help/articles/8972133-battery-degradation>; v2 spec example `degradation: 3` |
| Extra weight | "Weight for additional persons or luggage inside the vehicle (driver already accounted for)" | `extra_weight` [kg] | <https://abrp.featurebase.app/en/help/articles/9514655-extra-weight> |
| Initial vehicle temperature | "Vehicle temperature at the beginning of the plan - accounts, e.g., initial heating." | — (app only) | <https://abrp.featurebase.app/en/help/articles/2913669-initial-vehicle-temperature> |
| Accessories / trailer | "Pre-trained models for vehicle trailers and accessories" via vehicle configurations (6.0) | vehicle options in typecode | <https://abrp.featurebase.app/changelog/a-better-routeplanner-60> |

### 2.2 Battery / SoC

| Input | App wording | API v1 | Source |
|---|---|---|---|
| Departure SoC | must be set manually, or read from live data | `initial_soc_perc` | <https://abrp.featurebase.app/en/help/articles/6744660-departure-soc> |
| Destination arrival SoC | "minimum battery level … allowed when arriving at your final destination" | `arrival_soc_perc` | <https://abrp.featurebase.app/en/help/articles/7718276-destination-arrival-soc> |
| Charger arrival SoC | minimum SoC at any Charging Stop or waypoint; must be lower than Departure SoC or the plan fails | `charger_soc_perc` | <https://abrp.featurebase.app/en/help/articles/3809687-charger-arrival-soc> |
| Charger max SoC | default 80 %; planner "will automatically avoid charging for 'too long'" and typically stops at 60–80 % even if set to 100 % | `charger_max_soc_perc` | <https://abrp.featurebase.app/en/help/articles/9778614-charger-max-soc> |
| Charging overhead | "Added time for locating the charging station, connecting, and starting charging"; higher → fewer, longer stops | `charge_overhead` [s] (300 in doc example) | <https://abrp.featurebase.app/en/help/articles/0122642-charging-overhead> |
| Charging stops bias | slider: "Short but many" / "Quickest arrival" (middle) / "Few but long" | `charge_stops` ∈ {most, more, optimal, fewer, least} | <https://abrp.featurebase.app/en/help/articles/4023913-charging-stops> |

### 2.3 Speed

| Input | App wording | API v1 | Source |
|---|---|---|---|
| Maximum speed | "The maximum speed allowed, even if speed limits allow more." Default = model's `rec_max_speed` | `max_speed` [km/h] | <https://abrp.featurebase.app/en/help/articles/5005606-maximum-speed> |
| Reference speed | "Speed factor (in percentage) relative to the speed limits or estimated speed of the road" (110 = 10 % faster) | `speed_factor_perc` | <https://abrp.featurebase.app/en/help/articles/6845568-reference-speed> |
| Adjust speed | "Allow the planner to lower the maximum speed for individual legs if necessary to reach the next charger" — also reduces stop count | `adjust_speed` (bool) | <https://abrp.featurebase.app/en/help/articles/9155904-adjust-speed> |
| Real-time traffic (**Premium**) | "not only gives you better time estimates, but it is also calculated into your consumption"; applies to the first legs | `realtime_traffic` (beta, "first hour of driving") | <https://abrp.featurebase.app/en/help/articles/5543299-real-time-traffic> |

### 2.4 Weather / road conditions

| Input | App wording | API | Source |
|---|---|---|---|
| Real-time weather (**Premium**) | "live temperature and weather information to improve accuracy in predicted consumption" | v1 `realtime_weather`; v2 `weather.type=REAL_TIME` ("current conditions for wind, temperature and road-conditions") | <https://abrp.featurebase.app/en/help/articles/6430985-real-time-weather> |
| Seasonal weather | "Use typical seasonal temperature information." | v2 `SEASONAL` (fallback when real-time not licensed) | <https://abrp.featurebase.app/en/help/articles/9948707-seasonal-weather> |
| Temperature | "Outside temperature for the plan." | `outside_temp` [°C] | <https://abrp.featurebase.app/en/help/articles/5895984-temperature> |
| Wind | "Speed and direction." | `wind_speed` [m/s], `wind_dir` ∈ {head, tail} | <https://abrp.featurebase.app/en/help/articles/4862722-wind> |
| Road conditions | Dry / Rain or snow / Heavy rain or snow | `road_condition` ∈ {normal, rain, heavy_rain} | <https://abrp.featurebase.app/en/help/articles/8237579-road-conditions> |
| Avoid on route | "highways, ferries, tolls, or country border crossings (alpha)" | `allow_motorway`, `allow_ferry`, `allow_toll`, `allow_border` | <https://abrp.featurebase.app/en/help/articles/5684703-avoid-on-route> |
| Add a ferry manually | long-press on a ferry line → "add ferry line" | — | <https://abrp.featurebase.app/en/help/articles/8170717-add-a-ferry-manually> |

### 2.5 Chargers & networks (charger filtering)

| Input | App wording | API | Source |
|---|---|---|---|
| Type of chargers | after selecting a vehicle only compatible types are shown: Tesla SC, Tesla CCS, CCS, NACS, CHAdeMO, Level 2 | `fast_chargers` (comma list; default `rec_fast_chargers`) | <https://abrp.featurebase.app/en/help/articles/9996995-type-of-chargers> |
| Avoid and prefer (networks) | rank networks; warning that avoiding a dominant network "will not get a working plan" | `network_preferences` {networkId: −2 never, 0, +1 prefer, +2 exclusive, +3 exclusive+preferred}; tuned by `preferred_charge_cost_multiplier` (0.7 = only 70 % of charge time counted as cost) and `nonpreferred_charge_cost_addition` [s] | <https://abrp.featurebase.app/en/help/articles/4671857-avoid-and-prefer>, v1 doc |
| Charge cards | add cards → prices shown in plan; optional "prefer chargers supported by my cards" (may lengthen or fail plan) | v2 `charging.cardPreferences` | <https://abrp.featurebase.app/en/help/articles/5884894-charge-cards> |
| Charger availability (**Premium**) | "Use real-time charger availability and forecast in planning, if available." | `realtime_chargers` | <https://abrp.featurebase.app/en/help/articles/2269735-charger-availability> |
| Minimum charger stalls | soft preference, "not a 'hard' requirement" | v2 `preferredMinimumStallCount` | <https://abrp.featurebase.app/en/help/articles/3836603-minimum-charger-stalls> |
| Prefer trailer-friendly / dogs-friendly / play area / restroom (**Premium**) | "soft preference for charger locations that are rated by other ABRP users …" | v2 `featurePreferences` (PREFER only) | <https://abrp.featurebase.app/en/help/articles/7505540-prefer-trailer-friendly>, <https://abrp.featurebase.app/en/help/articles/3874005-prefer-dogs-friendly> |
| Exclude a specific charger | "Avoid this charger" UI string | `exclude_ids`, `exclude_locationids` | <https://raw.githubusercontent.com/iternio/abrp-translations/master/en.json> |
| Charger databases | — | `allowed_dbs` (e.g. `ocm,sc`) | v1 doc |

### 2.6 Waypoints / destinations

- ≥ 2 destinations; each may be an Iternio charger `id`, `lat/lon`, an `address`, a
  `locationid`, plus `bearing` and departure time. Ferries and railway stations can be
  inserted by the planner (`is_station`, `is_ferry`).
  <https://documenter.getpostman.com/view/7396339/SWTK3YsN>
- Per-stop overrides: on a Charging Stop's "waypoint options" the user can edit
  Arrival SoC, Charging power, Charging duration, Departure SoC, Departure date/time,
  then re-plan.
  <https://abrp.featurebase.app/help/articles/0210787-edit-the-details-of-a-charging-stop-in-your-plan>

---

## 3. Planner outputs

### 3.1 Plan-level

- Status `ok` / `invalid` (a plan is still returned with the failing step flagged
  `is_valid_step=false` — "Showing this to the user usually helps the user to understand
  what makes the trip impossible") / `notfound` / `address_not_found` /
  `address_different_regions`.
- `routes[]`: "the first is the fastest found option. The others are alternative routes
  which are defined as being reasonably close in time to the best, and significantly
  different when it comes to route." v1 `find_alts` → up to 3; `find_next_charger_alts`
  → up to 5 next-charger options. App 7.0: "up to 9 unique alternatives per plan",
  labelled Fastest / Saves Energy / Less Traffic.
- Per route: `total_drive_duration`, `total_charge_duration`, `total_dist`,
  `average_consumption` [Wh/km]. v2 `RouteSummary` adds `ferryDurationSec`,
  `consumedSoc`, `consumedWh`, `chargedSoc`, `primaryChargeDurationSec` vs
  `secondaryChargeDurationSec` ("over-the-night charge-stops").
- `plan_uuid` for reproducing/sharing and for `refresh_plan`.

Sources: <https://documenter.getpostman.com/view/7396339/SWTK3YsN>,
<https://api.iternio.com/swagger-ui/spec/prod/IternioPlanning.out.yaml>,
<https://abrp.featurebase.app/changelog>

### 3.2 Step (= waypoint or Charging Stop) + Leg

Each step: `name`, `id`, `lat/lon`, `is_charger`, `is_waypoint`, `is_station`,
`is_ferry`, `arrival_perc`, `departure_perc`, `arrival_duration`/`departure_duration`
(time left), `departure_dist`, `max_speed` [m/s] + `is_mod_speed` (adjusted-speed leg),
`charger` object, `drive_duration`, `drive_dist`, `charge_duration`, `charge_energy`
[kWh], `wait_duration` (ferries), `drive_weather` {temp, wind_dir, wind_speed,
condition}. v2 adds `ChargeProfilePoint` {durationSec, socFrac, powerW} (the predicted
Charging Curve at that stop) and `LegTag` (ARRIVAL_SOC_BELOW_WANTED / …_ACCEPTABLE /
…_ZERO / …_CRITICAL, SECONDARY_CHARGE_SESSION).
<https://documenter.getpostman.com/view/7396339/SWTK3YsN>,
<https://api.iternio.com/swagger-ui/spec/prod/IternioPlanning.out.yaml>

### 3.3 Path (the SoC / elevation chart data)

Path points "in resolution about 200m steps or less": `lat`, `lon`, `soc_perc`,
`cons_per_km` [Wh/km], `speed`, `remaining_time`, `remaining_dist`, `speed_limit` (or
"∞"), `elevation` [m]. This is the series behind the app's SoC-vs-distance and elevation
chart; UI strings `elevation`, `arrival_soc`, `charge_duration` confirm they are
displayed.
<https://documenter.getpostman.com/view/7396339/SWTK3YsN>,
<https://raw.githubusercontent.com/iternio/abrp-translations/master/en.json>

### 3.4 Charger object

`name`, `address`, `lat/lon`, `status` (OPEN / CONSTRUCTION / CLOSED / LIMITED),
`network_id/name`, `locationid` (e.g. `ocm_130563` → OpenChargeMap origin), `outlets[]`
{type, stalls, power kW, live status per OCPI 2.1}.
<https://documenter.getpostman.com/view/7396339/SWTK3YsN>

---

## 4. UX flows and screens (iOS)

1. **Plan screen**: type departure + arrival (autocomplete), blue **Plan** button; tap
   route lines on the map to switch alternatives; blue **Drive** to start navigation;
   **Restart → Clear** to start over.
   <https://abrp.featurebase.app/en/help/articles/2516346-create-a-plan>
2. **Itinerary**: list of Legs and Charging Stops with per-stop "waypoint options" editor
   (see 2.6). Map callouts show network logo, charger speed, estimated charging duration
   in pill-shaped labels with "improved collision detection" (7.1).
   <https://abetterrouteplanner.com/resources/article/2026-05-12_abrp-7-1>
3. **Save / share plan**: heart icon saves "a set of plan settings" (results may differ when
   reloaded); "Share plan → Share ABRP link". Saved plans listed after Restart.
   <https://abrp.featurebase.app/en/help/articles/0538360-save-a-plan>,
   <https://abrp.featurebase.app/en/help/articles/6991246-share-a-plan>
4. **Charger Details** (7.1 redesign): "Plugs and live availability are at the top",
   photos, amenities, nearby POIs, opening hours, "latest successful charges" (6.0).
   <https://abetterrouteplanner.com/resources/article/2026-05-12_abrp-7-1>,
   <https://abrp.featurebase.app/changelog/a-better-routeplanner-60>
5. **Find Chargers / charger map** (7.1): filters by speed, network, plug type, charge
   card, amenities; result cards with photos, live availability, ratings.
   <https://abetterrouteplanner.com/resources/article/2026-05-12_abrp-7-1>
6. **Navigation amenity search** (6.0): "Search for suitable chargers on your route with,
   e.g., a few restaurants for a lunch break or simply a quick bathroom break." (v2 API
   `/plan/amenity-candidates`, alpha.)
   <https://abrp.featurebase.app/changelog/a-better-routeplanner-60>
7. **Driving mode**: background re-planning ("If you consume more energy than expected,
   ABRP can propose earlier or different charging stops"); Premium adds traffic, weather
   and charger-availability forecasts to refreshes; API `refresh_plan` "a reasonable
   calling period is 1-10 minutes".
   <https://abetterrouteplanner.com/resources/article/2024-10-10_obd-connection>,
   <https://documenter.getpostman.com/view/7396339/SWTK3YsN>
8. **Live Activities** (7.1): charging widget (SoC, target, time remaining) and driving
   widget (destination, ETA, distance, progress); iOS home-screen widget (6.0); Apple Watch
   (Premium page).
   <https://abetterrouteplanner.com/resources/article/2026-05-12_abrp-7-1>,
   <https://abetterrouteplanner.com/premium/>
9. **Settings**: Vehicle (garage, live data, "Edit settings"), Plan settings (sections
   above), App settings (units, consumption unit, theme, map type, voice guidance, speed
   camera alerts).
   <https://abrp.featurebase.app/en/help/collections/9196812-app-settings>
10. **CarPlay** (Premium), unified codebase with Android Auto (7.0).
    <https://abrp.featurebase.app/changelog>

---

## 5. Free vs Premium

| Free ("Standard") | Premium (€5/mo; $4.99/mo or $49.99/yr on iOS) |
|---|---|
| EV route planning with chargers | Planning using **live data** (traffic, weather, charger availability) |
| In-app navigation | **Apple CarPlay** / Android Auto |
| Global charger map, "Charger Search and Filters" | **Real-time traffic** (also fed into consumption) |
| 1,000+ EV models | **Weather forecasts** in planning |
| Calibrated driving profiles | **Live charger availability** + forecast |
| Vehicle live data (OBD) | Speed cameras |
| Save plans | **Drive & Charge history** ("My Drives", Excel export) |
| | **Multiple vehicles**, share a vehicle, vehicle configurations |
| | Soft charger preferences (trailer / dog / play area / restroom) |
| | Enode (OEM cloud) live data |
| | Apple Watch, priority support |

Sources: <https://abetterrouteplanner.com/home>, <https://abetterrouteplanner.com/premium/>,
<https://abrp.featurebase.app/en/help/articles/7182124-my-drives>,
<https://abrp.featurebase.app/en/help/articles/3872028-live-data-via-enode>,
<https://abrp.featurebase.app/en/help/articles/5577371-select-and-save-a-vehicle>,
<https://www.upperinc.com/reviews/a-better-route-planner-review/>

The API mirrors the gating: v2 says real-time traffic and real-time weather are
"considered premium and has to be explicitly enabled for your API key", falling back to
the default traffic model / seasonal weather with a warning.
<https://api.iternio.com/swagger-ui/spec/prod/IternioPlanning.out.yaml>

---

## 6. Known data sources (beyond the attribution screen)

| Data | Source / mechanism | Evidence |
|---|---|---|
| Charger registry | Iternio internal DB "consists mostly of data from various chargers database around the world, which we have received permission to use"; `locationid` prefixes such as `ocm_` (OpenChargeMap); `allowed_dbs` e.g. `ocm,sc` (Tesla Superchargers) | <https://documenter.getpostman.com/view/7396339/SWTK3YsN> |
| Live charger status | OCPI 2.1 status vocabulary per outlet (AVAILABLE, CHARGING, OUTOFORDER, …) — consistent with the Eco-Movement feed | same |
| Vehicle Models | manual ("Estimate") or "modeled based entirely on real-world data" from user telemetry; maturity `alpha/beta/mature` | <https://abrp.featurebase.app/en/help/articles/2379287-what-do-the-estimate-labels-on-vehicle-models-mean> |
| Telemetry (crowd + per-user calibration) | Telemetry API `tlm`: `utc, soc, power, speed, lat, lon, is_charging, is_dcfc, is_parked` (high priority) + `capacity, soe, soh, heading, elevation, ext_temp, batt_temp, voltage, current, odometer, est_battery_range, hvac_power, …`; 1 point / 5 s desired; "To enable vehicle individual consumption calibration, we need at least speed, power and is_charging at a rate of at least once per 10 seconds" | <https://documenter.getpostman.com/view/7396339/SWTK5a8w> |
| Live data channels | OBD BLE dongle (in-car), Tesla, Enode (OEM cloud, Premium), Android Auto, OVMS; comparison table at `/compare/livedata/` | <https://abrp.featurebase.app/en/help/collections/8696302-live-data>, <https://abetterrouteplanner.com/resources/article/2024-10-10_obd-connection> |
| Weather | real-time forecast or seasonal averages (OpenWeather per attributions) | v2 spec `Weather` |
| Community charger ratings | trailer / dog / play-area / restroom "rated by other ABRP users" | <https://abrp.featurebase.app/en/help/articles/7505540-prefer-trailer-friendly> |
| Charge cards / prices | user-added cards → prices in plan; `currency` in v2 `resultOptions` | <https://abrp.featurebase.app/en/help/articles/5884894-charge-cards> |
| Geocoding | v2 autosuggest modes: HERE, Google, Pelias, charger, ferry | v2 spec `AutosuggestMode*` schemas |

---

## 7. What users consider ABRP's distinctive value (secondary sources)

- Iternio's own framing: "Pick your destination and we'll add the best chargers to get
  there"; range prediction that "learns from your driving".
  <https://abetterrouteplanner.com/home>
- App Store reviewers: "virtually eliminates any vestige of 'range anxiety'"; keeping SoC
  "between 30% and 70%"; "plan my trip even better than Tesla's already good in car route
  planner"; granular speed/weight controls; "avoid highways" that the car's native planner
  lacks. Complaints: crashes/lag on long trips and CarPlay, stale charger data, charging
  time errors on 800 V cars.
  <https://apps.apple.com/us/app/a-better-routeplanner-abrp/id1490860521?see-all=reviews>
- ev-tips: "very accurate" battery predictions; "It will optimise charging times and
  percentages to meet your goals"; both a planning app and a navigation app.
  <https://ev-tips.com/a-better-route-planner-review-abrp/>
- upperinc: praised for multi-day / long trips and network-only planning; criticised for
  stability, Android Auto lag, charge-time optimism, filters not always honoured.
  <https://www.upperinc.com/reviews/a-better-route-planner-review/>

Net: the value is the **energy-aware Plan** (Charging Stops with arrival/departure SoC
and durations that respect the Charging Curve, speed, elevation, weather and payload) and
its **re-planning against live SoC**. Map, POI, navigation and photos are table stakes.

---

## 8. Feature inventory ranked ABRP-specific → generic

Ranking criterion: does a mainstream nav app (Apple/Google Maps) or a car's built-in
planner already offer it? Tier A is what "the core planner" must replicate.

### Tier A — ABRP-specific, core planner (in scope)

| # | Feature | Why ABRP-specific | Where documented |
|---|---|---|---|
| A1 | Vehicle Model with Energy Model (consumption vs speed, elevation, temperature, wind, road condition, extra weight) and Charging Curve per connector | The physics is the product; 1,000+ models with maturity labels | §2.1, §6 |
| A2 | Plan optimiser: choose Charging Stops minimising total time, honouring Departure SoC, Charger arrival SoC, Destination arrival SoC, Charger max SoC, overhead; returns `invalid` plans with the failing Leg flagged | Built-in planners fix SoC targets; ABRP exposes them and explains failures | §2.2, §3.1 |
| A3 | Charging-stops bias (many-short ↔ quickest ↔ few-long) | v1 `charge_stops`, app slider | §2.2 |
| A4 | Reference consumption override + per-user calibration from telemetry | Unique to ABRP | §2.1, §6 |
| A5 | Speed model: max speed, reference speed factor, **adjust speed** to reach next Charger | No nav app plans slower to make a charger | §2.3 |
| A6 | Weather model: manual temp/wind/road condition, seasonal fallback, real-time (Premium) | Consumption input, not just ETA | §2.4 |
| A7 | SoC-vs-distance + elevation chart from path points (`soc_perc`, `elevation`, `cons_per_km` every ~200 m) | The signature output | §3.3 |
| A8 | Charger filters that feed the optimiser: connector types, network avoid/prefer/exclusive with cost multipliers, charge-card preference, min stalls (soft), exclude charger, Premium amenity preferences | Google/Apple filter the map, not the plan | §2.5 |
| A9 | Per-stop overrides (arrival/departure SoC, power, duration, departure time) then re-plan | | §2.6 |
| A10 | Route alternatives labelled Fastest / Saves Energy / Less Traffic; next-charger alternatives | Energy-labelled alternatives are ABRP's | §3.1 |
| A11 | Live re-planning from live SoC (OBD / OEM cloud), `refresh_plan` cadence 1–10 min | | §4.7, §6 |
| A12 | Battery degradation, initial vehicle temperature, trailer/accessory configurations | Energy-model modifiers | §2.1 |

### Tier B — EV-flavoured but increasingly commodity (nice-to-have)

| # | Feature | Who else has it | Where documented |
|---|---|---|---|
| B1 | Live charger availability + forecast (Premium) | Google Maps, PlugShare, OEM apps | §2.5 |
| B2 | Charger details: photos, opening hours, last successful charge, ratings, amenities, nearby POIs | PlugShare, Chargemap | §4.4 |
| B3 | Charge-card prices in plan | Chargemap, OEM apps | §2.5 |
| B4 | Amenity search along route (lunch / restroom) | Google Maps "along route" | §4.6 |
| B5 | Drive & charge history with export | OEM apps | §5 |
| B6 | Multi-vehicle garage, vehicle sharing | OEM apps | §5 |
| B7 | Ferries / stations as steps, manual ferry add | Google Maps handles ferries | §2.4 |

### Tier C — any nav app has it (out of core scope)

| # | Feature | Where documented |
|---|---|---|
| C1 | Address autocomplete, waypoints, avoid highways/tolls/ferries/borders | §2.4, §2.6 |
| C2 | Turn-by-turn navigation, voice, CarPlay, speed cameras, real-time traffic ETA | §4, §5 |
| C3 | Save / share plan links, deep links (`car_model`, `plan_uuid`, `charger_id`) | §4.3, <https://documenter.getpostman.com/view/7396339/TWDTNeds> |
| C4 | Map tiles, map type, theme, units, language | §4.9 |
| C5 | Live Activities / widgets / Watch | §4.8 |

---

## 9. Implications for ABRP-native scope

- The **Plan request** to replicate is the v1 `plan` parameter set (§2) minus
  Premium/live items; the **Plan result** is `routes[] → steps[] → path[]` (§3) with
  `RouteSummary`-style totals. These map 1:1 onto the CONTEXT.md terms Plan / Leg /
  Charging Stop / Charger / SoC.
- The first Vehicle Model (Ioniq 5 2022) needs at least: usable capacity, a
  reference consumption at 110 km/h, a recommended max speed, connector list, and a
  Charging Curve (power vs SoC) — the fields the vehicle-library record exposes. Values
  must be sourced independently (manufacturer data, published tests), not from Iternio.
- The failure semantics (`invalid` plan with flagged Leg; "Charger arrival SoC must be
  lower than Departure SoC") and the "planner stops at 60–80 % even if max is 100 %"
  behaviour are explicit product rules worth carrying into ADRs.

## 10. Gaps / not verified

- The app's SoC/elevation chart was not screenshotted; its existence is inferred from the
  path-point fields and UI strings, not from a primary screenshot.
- `abetterrouteplanner.com/compare/livedata/` and the Postman deep-links page render
  client-side; only the collection JSON was readable.
- The forum blog (`forum.abetterrouteplanner.com`) returned HTTP 525 during research; the
  founder's older algorithm write-ups were not consulted.
- The GitHub org `A-Better-Routeplanner` could not be confirmed as Iternio-owned and was
  not used as a source.
