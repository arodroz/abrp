# Research: Hyundai Ioniq 5 (2022, EU) Vehicle Model data and Energy Model approaches

Issue: #5 (wayfinder research ticket). Date: 2026-08-27.

Scope: public/open data only for the first Vehicle Model (Hyundai Ioniq 5, model year 2022, European market), plus a survey of Energy Model forms suitable for predicting a Leg's energy use. No ABRP/Iternio proprietary vehicle data was consulted or copied; the only ABRP material cited is Iternio's own public help article describing what "reference consumption" means.

Terms follow `CONTEXT.md`: Vehicle Model, Energy Model, Charging Curve, SoC, Leg, Charging Stop.

Confidence legend used in the tables: **A** = manufacturer/regulatory or peer-reviewed; **B** = independent instrumented test (ADAC, InsideEVs, electrive, Bjørn Nyland); **C** = aggregator estimate (EV Database, EVKX) or derived/assumed here.

---

## 1. Trims and headline manufacturer figures (MY2021/22, EU)

Source of the whole table unless noted: Hyundai Motor Europe press kit, "Hyundai IONIQ 5 redefines electric mobility lifestyle" — https://www.hyundai.news/eu/models/electrified/ioniq-5/press-kit/the-new-hyundai-ioniq-5.html (A).

| Trim | Battery (nominal) | Drive | Max power / torque | 0–100 km/h | WLTP combined consumption (19" / 20") | WLTP range |
|---|---|---|---|---|---|---|
| Standard Range 2WD | 58 kWh | RWD | 125 kW / 350 Nm | 8.5 s | 16.7 kWh/100 km (19") | 384 km (Wikipedia, citing Hyundai) https://en.wikipedia.org/wiki/Hyundai_Ioniq_5 |
| Standard Range AWD | 58 kWh | AWD | 173 kW / 605 Nm | 6.1 s | 18.1 kWh/100 km (19") | — |
| Long Range 2WD | 72.6 kWh | RWD | 160 kW / 350 Nm | 7.4 s | 16.8 / 17.9 kWh/100 km | 481 km (19") |
| Long Range AWD | 72.6 kWh | AWD | 225 kW / 605 Nm | 5.2 s | 17.7 / 19.0 kWh/100 km | 462 km (19") / 431 km (20") per InsideEVs quoting Hyundai specs https://insideevs.com/news/523712/hyundai-ioniq5-range-test-bjorn/ |

Common to all: top speed 185 km/h; 800 V battery system with 400/800 V multi-charging; "10 to 80 per cent charged in just 18 minutes" on a 350 kW charger; "less than five minutes of charging" for 100 km; V2L 3.6 kW; length/width/height 4,635 / 1,890 / 1,605 mm, wheelbase 3,000 mm; heat pump climate system (same press kit).

EV Database's WLTP breakdown for the LR 2WD MY21: rated 166 Wh/km (TEL, 485 km) to 179 Wh/km (TEH, 451 km); the site's "vehicle consumption" net of charging losses is 144–155 Wh/km. LR AWD MY21: 176–190 Wh/km rated, 462–430 km. https://ev-database.org/car/1478/Hyundai-IONIQ-5-Long-Range-2WD and https://ev-database.org/uk/car/1479/Hyundai-IONIQ-5-Long-Range-AWD (C, "rated" figures are Hyundai's).

Model-year caveat: from MY2023 the long-range pack became 77.4 kWh and battery preconditioning became standard ("the new battery heater and conditioning feature is standard across the range"), per Hyundai UK — https://www.hyundai.news/uk/articles/press-releases/hyundai-ioniq-5-to-receive-product-enhancements-for-2023-model-year.html (A). Many third-party pages (EVKX, evspecifications) now show 77.4 kWh figures under a "2022" label; treat any 77.4 kWh / 233 kW number as MY2023+, not our Vehicle Model.

---

## 2. Vehicle Model parameter table

### 2.1 Battery: gross vs usable

| Parameter | Value | Source | Conf. |
|---|---|---|---|
| Nominal capacity, LR | 72.6 kWh | Hyundai press kit (above) | A |
| Usable capacity, LR (EV Database estimate) | 70.0 kWh | https://ev-database.org/car/1478/Hyundai-IONIQ-5-Long-Range-2WD ("Useable Capacity* 70.0 kWh", * = estimated) | C |
| Usable capacity, LR (Bjørn Nyland test) | 70.6 kWh "used battery capacity (estimated)" | https://insideevs.com/news/523712/hyundai-ioniq5-range-test-bjorn/ | B |
| Gross vs net, LR (ADAC) | "Batteriekapazität (netto/brutto) 72,6/77 kWh"; a full 0→100 % AC charge drew 82.1 kWh from the grid | ADAC Autotest PDF https://assets.adac.de/image/upload/Autodatenbank/Autotest/at6147-hyundai-ioniq-5-726-kwh-techniq-paket-2wd/hyundai-ioniq-5-726-kwh-techniq-paket-2wd.pdf | B |
| Pack architecture, LR | 180s2p, 360 NMC pouch cells, 653 V nominal, "800 V" | EV Database (above) | C |
| Standard Range | 58 kWh nominal; InsideEVs assumed 58.2 kWh net / 62 kWh gross ("our guess") | https://insideevs.com/news/513818/hyundai-ioniq5-58kwh-charging-analysis/ | C |
| US-market equivalents (for cross-checks only) | 58.0 kWh @ 522.7 V; 77.4 kWh @ 697 V, 2P192S, SK Innovation | Hyundai Motor America 2022 spec sheet https://www.hyundainews.com/assets/documents/original/49532-2022Ioniq5ProductGuidespecsV4030722.pdf | A |

Recommendation: Vehicle Model `usableCapacity_kWh = 70.0` (LR) with SoC defined against that figure (CONTEXT.md: "percentage of usable capacity"). Note the discrepancy in vocabulary: Hyundai and ADAC call 72.6 kWh "net", while EV Database/Nyland see ~70–70.6 kWh actually dischargeable; ADAC's 82.1 kWh grid draw for a full charge implies ~88 % AC charging efficiency relative to 72.6 kWh, which matters for cost/time at AC Charging Stops but not for Leg energy.

### 2.2 Mass

| Variant | Kerb / unladen (EU) | GVWR | Source | Conf. |
|---|---|---|---|---|
| LR 2WD (19", TECHNIQ) | 1,985 kg ("Leergewicht"), payload 445 kg | — | ADAC Autotest PDF (above) | B |
| LR 2WD | 1,985 kg "Weight Unladen (EU)" | 2,430 kg | EV Database 1478 | C |
| LR AWD | 2,095 kg "Weight Unladen (EU)" | 2,540 kg | EV Database 1479 | C |
| LR 2WD (arenaev, likely 20") | 2,065 kg unladen | — | https://www.arenaev.com/hyundai_ioniq_5_72_6kwh_rwd_2021-specs-40.php | C |
| Ranges by pack | 58/63 kWh: 1,800–2,020 kg; 72.6/77.4 kWh: 1,905–2,125 kg | — | https://en.wikipedia.org/wiki/Hyundai_Ioniq_5 | C |
| US 2022 | RWD 4,200–4,414 lb (1,905–2,002 kg); AWD 4,464–4,662 lb (2,025–2,115 kg) | — | Hyundai Motor America spec sheet (above) | A |

Recommendation: `mass_kg = 1985` (LR 2WD) / `2095` (LR AWD), EU "unladen" convention (includes a 75 kg driver), plus a user-settable payload. Rotational-mass allowance: SUMO uses a `rotatingMass` of 40 kg by default (https://sumo.dlr.de/docs/Models/Electric.html); Koch et al. use a powertrain inertia of 12.5 kg·m² for a BMW i3 (https://www.tkn.tu-berlin.de/bib/koch2021accurate/koch2021accurate.pdf). For a planner operating on speed profiles at link resolution, a 3–5 % mass uplift is an adequate stand-in.

### 2.3 Aerodynamics

| Parameter | Value | Source | Conf. |
|---|---|---|---|
| Cd | 0.288 | Hyundai Motor Group, "The aerodynamic design of IONIQ 5": "the drag coefficient of the IONIQ 5 was 0.288" https://www.hyundaimotorgroup.com/en/story/CONT0000000000002760 ; also Hyundai Motor America spec sheet "Coefficient of Drag (Cd) (lowest) 0.288" | A |
| Active air flap effect | ΔCd ≈ 0.013 open vs closed, "increases the range by about 7.3 km for each charging" | same HMG article | A |
| Frontal area A | Not published. Width × height = 1.890 × 1.605 = 3.03 m²; with the usual 0.80–0.85 fill factor this gives A ≈ 2.4–2.6 m². Koch et al. use 2.38 m² for the smaller BMW i3, SUMO's default is 2.6 m² (sources above). | derived | C |
| CdA (working value) | 0.288 × 2.5 = 0.72 m² — to be **calibrated** against the 120 km/h and 110 km/h tests in §3 | derived | C |
| Air density | 1.225 kg/m³ at 15 °C sea level; scale with temperature and altitude in the Energy Model | ISA standard atmosphere | A |

Note ADAC's own spec table lists "Stirnfläche/cW-Wert n.b." (not available), confirming that A is not disclosed.

### 2.4 Rolling resistance

| Parameter | Value | Source | Conf. |
|---|---|---|---|
| OE tyres | 235/55 R19 (ADAC test car, "Reifengröße (Serie) 235/55 R19"); 255/45 R20 on 20" trims (Hyundai Motor America spec: tread widths given for 19"/20"; evspecifications lists 235/55R19 and 255/45R20 https://www.evspecifications.com/en/model/558213f) | ADAC PDF; HMA | A/B |
| Tyre make on tested cars | Nyland's AWD test car: 19" Michelin Primacy 4 https://insideevs.com/news/523712/hyundai-ioniq5-range-test-bjorn/ ; UK 20" AWD cars: Michelin Pilot Sport EV (owner report) https://www.ioniqforum.com/threads/michelin-pilot-sport-ev-tyres-as-fitted-to-uk-awd-20-inch-cars.39553/ | B / C |
| EU label classes (C1), RRC in N/kN = kg/t | A ≤ 6.5; B 6.6–7.7; C 7.8–9.0; D 9.1–10.5; E ≥ 10.6 | Regulation (EU) 2020/740 Annex I https://www.legislation.gov.uk/eur/2020/740/annex/I/part/1/adopted/data.xht?view=snippet&wrap=true | A |
| Primacy 4 label | typically B or C for fuel efficiency; e.g. 7.74 kg/t new in 205/55 R16 | https://www.tyrereviews.com/Article/EU-Tyre-Label-Current-status-and-challenges.htm and Michelin comparison via https://www.michelin.com.au/auto/advice/choose-tyres/eco-designed-tyre | C |
| Pilot Sport EV label | "most commonly carries EU label ratings of A for fuel efficiency" | https://www.tyrereviews.com/Tyre/Michelin/Pilot-Sport-EV.htm | C |
| Whole-vehicle Crr (working value) | 0.008–0.010 (tyre RRC 0.0065–0.009 plus bearings/road texture). Koch et al. use 0.007 for the i3; SUMO default 0.01. | derived | C |

Recommendation: `crr = 0.009` for 19" Primacy 4, `0.008` for 20" Pilot Sport EV, then calibrate jointly with CdA (the two are separable because rolling scales with v and aero with v³ in power terms — the 90 vs 120 km/h Nyland pair in §3 gives two equations).

### 2.5 Drivetrain, regeneration and auxiliaries

| Parameter | Value | Source | Conf. |
|---|---|---|---|
| Motor/inverter hardware | Rear IPMSM 160 kW (2WD) / 155 kW rear + 70 kW front (AWD); rear inverter SiC MOSFET, front IGBT; oil-cooled hairpin stators | MarkLines/Munro teardown summary https://www.marklines.com/en/report/Munro014_202309 ; Hyundai press kit | B/A |
| Component efficiencies used in a validated physics model (BMW i3, PMSM) | gearbox 96 %, motor ≈ 90 % (map average), inverter 97 %, battery 98 % → combined 82 % propulsion and 82 % recuperation; 360 W auxiliaries | Koch et al., ITSC 2021, Table II and Eq. (6) https://www.tkn.tu-berlin.de/bib/koch2021accurate/koch2021accurate.pdf | A |
| SUMO defaults (upper bound, criticised by Koch) | propulsionEfficiency 0.98, recuperationEfficiency 0.96, constantPowerIntake 100 W | https://sumo.dlr.de/docs/Models/Electric.html | A |
| Regen behaviour | Adjustable regen "from 0 to 3 levels" via paddles; i-Pedal one-pedal mode | Hyundai Motor Group https://www.hyundaimotorgroup.com/en/story/CONT0000000000047551 | A |
| Regen field evidence | Pre-facelift dual-motor Ioniq 5: ~75 mi descent from Loveland Pass consumed "around 10 %" of the pack; whole 145 mi mountain loop 99 %→46 % | https://www.notebookcheck.net/How-much-charge-would-an-Ioniq-5-EV-recover-when-going-downhill-YouTubers-attempt-to-find-out.874176.0.html | C |
| Regen efficiency vs deceleration | VT-CPEM models "instantaneous regenerative braking energy efficiency as a function of the deceleration level"; average error 5.9 % vs measured data | Fiori, Ahn, Rakha, Applied Energy 168 (2016) 257–268 https://www.sciencedirect.com/science/article/abs/pii/S030626191630085X | A |
| Working values | η_drive = 0.85 (battery→wheel, SiC rear inverter slightly better than i3's IGBT), η_regen = 0.65 effective (includes the fraction of braking not captured), P_aux_base = 300 W | derived | C |

### 2.6 HVAC / heat pump

| Item | Value | Source | Conf. |
|---|---|---|---|
| Fitment | Heat pump is optional on MY21/22 EU cars ("HP Standard Equipment: No, optional") | EV Database 1478/1479 | C |
| How it works | "utilizing the waste heat generated from the battery and PE system"; "the heater consumes the most energy except for the motor" | Hyundai Motor Group https://www.hyundaimotorgroup.com/en/story/CONT0000000000047551 | A |
| Effect on range (generic EV data) | heat pumps "increase range by 8 % to 10 % in cold conditions" near 30 °F; at 20 °F a heat pump gave a "31 % reduction in energy consumption" vs PTC (Zhao et al. 2022 as cited) | Recurrent https://www.recurrentauto.com/research/heat-pumps | B |
| Steady-state cabin load (generic) | heat pump 1.8–2.6 kW vs PTC 4.2–5.8 kW at 5 °C ambient, 20 °C cabin; COP ≥ 1.8 down to −15 °C | search summary of https://carinterior.alibaba.com/tips/ptc-vs-heat-pump-efficiency (vendor content, low trust) and MDPI WEVJ 17(4):168 https://doi.org/10.3390/wevj17040168 (chassis-dyno HP vs PTC at −10/−7/0/25 °C) | C / A |
| Ioniq 5 in bad weather | AWD 72.6 kWh at highway speed, heavy rain, 12–13 °C, heating on (heat pump car): 309 Wh/km | https://insideevs.com/news/512271/hyundai-ioniq5-range-worst-conditions/ | B |
| Planner convention | EVKX applies a flat "2 kW AC" heating load to WLTP (−21 %) and to 120 km/h (24.0 → 25.7 kWh/100 km) | https://evkx.net/models/hyundai/ioniq_5/ioniq_5_long_range_awd_gen1/rangeandconsumption/ | C |

Working model: `P_hvac(T_amb)` piecewise-linear — 0.3 kW at 18–24 °C, rising to ~2 kW at 0 °C and ~3.5 kW at −10 °C with heat pump (PTC-only cars ×1.8), plus ~1 kW for A/C above 30 °C. Calibrate against EV Database's −10 °C scenarios in §3.

---

## 3. Published consumption vs speed and temperature (calibration targets)

| Condition | Variant | Consumption | Source | Conf. |
|---|---|---|---|---|
| 90 km/h constant, 25 °C, 19" Primacy 4 | LR AWD | 153 Wh/km, 461 km | Bjørn Nyland via InsideEVs https://insideevs.com/news/523712/hyundai-ioniq5-range-test-bjorn/ | B |
| 120 km/h constant, 21 °C, some rain, 19" | LR AWD | 244 Wh/km, 289 km (+59 % vs 90 km/h) | same | B |
| 110 km/h, 23 °C, no A/C ("Highway – Mild") | LR 2WD / AWD | 209 / 212 Wh/km | EV Database 1478 / 1479 | C |
| 110 km/h, −10 °C, heating ("Highway – Cold") | LR 2WD / AWD | 264 / 269 Wh/km | EV Database | C |
| City, 23 °C / −10 °C | LR 2WD | 128 / 189 Wh/km | EV Database | C |
| Combined, 23 °C / −10 °C | LR 2WD | 165 / 222 Wh/km | EV Database | C |
| ADAC Ecotest (incl. AC charging losses) | LR 2WD 19" | 20.9 kWh/100 km overall; city 14.8, rural 22.7, motorway 24.9 kWh/100 km; ~390 km | ADAC Autotest PDF | B |
| 70 mph (113 km/h), −1 °C, 19", AWD SEL | LR AWD (US 77.4) | 227 mi | InsideEVs https://insideevs.com/reviews/566406/hyundai-ioniq5-70mph-range-test/ | B |
| 70 mph, 3 °C, 20", AWD Limited | LR AWD (US 77.4) | 195 mi; quarter-by-quarter display 2.6–2.7 mi/kWh (≈230–240 Wh/km) | same | B |
| Highway, heavy rain, 12–13 °C, heating | LR AWD | 309 Wh/km | InsideEVs worst-conditions | B |
| 120 km/h "perfect", EVKX model | LR AWD (77.4) | 24.0 kWh/100 km; 25.7 with 2 kW heating | EVKX (above) | C |
| Hyundai demo assumption | LR | 151 Wh/km (WLTP 480 km / 72.6 kWh) | https://insideevs.com/news/503522/hyundai-ioniq5-fast-charging-analysis/ | C |

Sanity check of the physics parameters against Nyland's pair (AWD, 2,095 kg + driver, Crr 0.009, CdA 0.72 m², ρ 1.20, η_drive 0.85, P_aux 0.3 kW): at 90 km/h the model gives ≈150 Wh/km and at 120 km/h ≈235 Wh/km, versus 153 and 244 measured — within 4 %, so the working values are a credible starting point before calibration.

---

## 4. Charging Curve (800 V, DC)

### 4.1 Warm-battery curve, 72.6 kWh pack

Hyundai's own demo session (South Korea, 20 Apr 2021, 19 °C ambient, ~15 °C initial battery temperature), as digitised by InsideEVs https://insideevs.com/news/503522/hyundai-ioniq5-fast-charging-analysis/ (B):

| SoC | Power |
|---|---|
| 10 % | ~115 kW |
| 14–15 % | ~187 kW |
| 29–30 % | 220 kW |
| ~51 % | peak > 225 kW |
| 52–54 % | dip to ~120 kW (thermal), then recovers |
| 79–80 % | ~130 kW |
| 20–80 % average | 180 kW ("80 % of the peak value") |

InsideEVs' summary table for the same pack: peak 225 kW, avg 20–80 % 180 kW (Hyundai demo) and 224 / 170 kW (IONITY session by "Battery Life") https://insideevs.com/news/513818/hyundai-ioniq5-58kwh-charging-analysis/.

Independent EU measurements of the 72.6 kWh pack: ADAC (LR 2WD) measured 10–80 % in exactly 18:00 min at an average of 188 kW, 53 kWh added; 10 min → +33.3 kWh (to 55 %), 20 min → +55.8 kWh (83 %), 30 min → +63.7 kWh (92 %); "Ladeleistungen bis 220 kW" (ADAC Autotest PDF, B). EV Database lists max 221 kW, 10–80 % average 179 kW, 17 min (C).

Full 0–100 % point table (for curve *shape* above 80 %): EVKX publishes a per-percent table, but for the 77.4 kWh MY23+ pack — 205 kW at 1 %, 221 kW at 10 %, 233–234 kW at 43–46 %, step down to 186 kW at 48 %, 158 kW at 80 %, 122 kW at 83–85 %, 63 kW at 90 %, 40 kW at 95 %, 12 kW at 100 %; 10–80 % 16 m 34 s, avg 187.6 kW https://evkx.net/models/hyundai/ioniq_5/ioniq_5_long_range_awd_gen1/chargingcurve/ (C; scale the plateau to ~220 kW for 72.6 kWh, keep the >80 % taper shape).

### 4.2 Standard Range 58 kWh pack

Peak ~177 kW from ~7 % to 45 % SoC, then <145 kW to 57 %, ~140 kW, tapering to 50 kW at 86 %; 20–80 % in a little over 15 min, average 144 kW (81 % of peak); 16 °C ambient. https://insideevs.com/news/513818/hyundai-ioniq5-58kwh-charging-analysis/ (B).

### 4.3 Temperature effects and preconditioning

- Cold battery, MY22 (no preconditioning): at −5 °C to 0 °C ambient, three sessions all took "exactly 30 minutes to go from 10 % to 80 %", max 147 kW; author's rule of thumb is that the battery must be "north of 65 °F (18 °C)" for the rated curve. https://insideevs.com/news/578869/hyundai-ioniq-5-charging-analysis/ (B).
- MY23 with preconditioning at 4 °C ambient (electrive, 77.4 kWh): without preconditioning the session started at ~68 kW, peaked 134 kW, averaged 102 kW over 28 min; with preconditioning it started at 113 kW (19 %), plateaued ~195 kW, held 224 kW from 40–55 %, still 125 kW at 80 %, 19→80 % in 16 min at 182 kW average. Target pack temperature stated as about 25 °C. https://www.electrive.com/2023/03/24/hyundai-ioniq-5-battery-preconditioning-is-it-worth-the-upgrade/ (B).
- Preconditioning availability: Hyundai UK states the "battery heater and conditioning feature" arrives with MY2023 and "activates automatically when a high-power charging point is entered into the vehicle's navigation system" (Hyundai UK press release, A). The MY21/22 EU Vehicle Model therefore has **no** automatic preconditioning; the planner should assume a cold-start curve whenever ambient is below ~10 °C unless the battery has been warmed by driving.
- Charger side: the full curve needs an 800 V-capable ≥250 kW charger; on 400 V 150 kW hardware Hyundai quotes 25 min (est.) for 10–80 % (Hyundai Motor America spec sheet, A). Fastned's brand page notes generically that the published curves assume an optimally tempered battery and that "a colder (or warmer) battery can result in a significantly lower charge speed" https://www.fastnedcharging.com/en/brands-overview/hyundai (C).

Recommended Charging Curve representation: a piecewise-linear table `P(SoC)` for the warm state (from §4.1, scaled to the 72.6 kWh plateau), a cold-state table (30 min 10–80 % ⇒ ~100–147 kW cap), and a battery-temperature scalar interpolating between them; plus a per-Charger cap `min(P_vehicle, P_charger)` and a 400 V-charger branch (~150 kW cap → 25 min).

---

## 5. Energy Model approaches

### 5.1 Physics-based longitudinal (backward-facing) model

Formulation (Wu et al. 2015, Eq. 1–5): tractive force `F = m·a + ½ρ·Cd·A·v² + Crr·m·g + m·g·sin θ`, wheel power `p = F·v`, battery power `P = p/η` (motor efficiency η), auxiliaries added separately. https://wakengineering.com/wp-content/uploads/2015/07/Wu_et_al_2015.pdf (Transportation Research Part D 34 (2015) 52–67; A). The same structure underlies:

- SUMO's battery device (Kurczveil, López, Schnieder 2014, LNCS 8594 pp. 33–43 https://link.springer.com/chapter/10.1007/978-3-662-45079-6_3): parameters `vehicleMass`, `frontSurfaceArea`, `airDragCoefficient`, `rollDragCoefficient`, `rotatingMass`, `radialDragCoefficient`, `constantPowerIntake`, `propulsionEfficiency`, `recuperationEfficiency` https://sumo.dlr.de/docs/Models/Electric.html (A, open source, EPL-2.0).
- VT-CPEM (Fiori, Ahn, Rakha 2016): adds regen efficiency as a function of deceleration; validated to 5.9 % average error on a Nissan Leaf; intended "for use in ... in-vehicle applications including eco-routing systems" https://www.sciencedirect.com/science/article/abs/pii/S030626191630085X (A).
- MMPEVEM (Koch et al. 2021) in SUMO: component-level model with motor loss map, transmission efficiency, battery internal resistance; RMSE 4.99 kW instantaneous but cumulative energy within ~1 % on a WLTC chassis-dyno run; shows that the two-constant-efficiency SUMO model can under-predict by 4–7 % and mis-handle recuperation https://sumo.dlr.de/docs/Models/MMPEVEM.html and https://www.tkn.tu-berlin.de/bib/koch2021accurate/koch2021accurate.pdf (A).
- Genikomsakis & Mitrentsis 2017 ("computationally efficient simulation model ... route planning applications"): tabular motor efficiency, generic power-electronics map, auxiliaries, electro-thermal battery; designed for exactly our use case https://www.sciencedirect.com/science/article/abs/pii/S1361920915302881 (A).

Pros: needs only the §2 parameters; extrapolates to any speed, grade, payload, wind, air density; grade and wind enter naturally; per-Leg integration over the Routing Engine's speed profile is trivial and fast (closed form per link at constant speed).
Cons: unpublished parameters (A, Crr, η maps) must be calibrated; temperature effects (battery internal resistance, cold-oil drivetrain drag, HVAC) are not captured by the mechanics and need add-on terms; regen fraction depends on driver/traffic; sensitivity to speed-profile realism (a speed-limit profile over-predicts constant-speed driving; ABRP's public help notes they assume drivers go "~15 % above the speed limit" https://abrp.featurebase.app/articles/3305478-reference-consumption).

### 5.2 Empirical lookup (reference-consumption / scenario tables)

Examples: Iternio's public "reference consumption" concept — a single Wh/km "base value measured at a constant speed of 110 km/h (or 65 mph) in near-perfect conditions" that scales a fixed speed curve, with elevation, temperature and speed adjustments layered on (only the concept is public; the curves are proprietary) https://abrp.featurebase.app/articles/3305478-reference-consumption. EV Database's six-scenario table (city/highway/combined × mild/cold) https://ev-database.org/car/1478/Hyundai-IONIQ-5-Long-Range-2WD. evnav (open-source, MIT): single constant Wh/km efficiency, "does not take into account the elevation, the weather" https://github.com/giraldeau/evnav.

Pros: directly anchored to measured data; one user-calibratable scalar; trivial to implement.
Cons: no grade, wind or payload response unless bolted on; table sparsity at low/high speeds; every new tyre/wheel/trim needs new data; cannot be reasoned about physically when it disagrees with reality.

### 5.3 Hybrid and data-driven

- De Cauwer, Van Mierlo, Coosemans 2015 (Energies 8(8) 8573–8593): physics terms as regressors with coefficients fitted to real-world Leaf data — a physics skeleton with statistically fitted gains https://doi.org/10.3390/en8088573 (A).
- NREL RouteE Powertrain (BSD-3-Clause): link-level ML models (random forest etc.) trained on FASTSim outputs and ~1 M miles of drive data; inputs are link speed, grade, turns; open catalogue includes BEVs https://github.com/NREL/routee-powertrain and https://www2.nrel.gov/transportation/route-energy-prediction-model (A). RouteE Compass adds energy-aware routing.
- NREL review of EV energy-consumption estimation models (taxonomy physics / data-driven / hybrid) https://www.osti.gov/servlets/purl/1824218 (A).
- Routing-algorithm literature that consumes any of these models: Baum, Dibbelt, Pajor, Wagner, "Energy-Optimal Routes for Battery Electric Vehicles", Algorithmica 82 (2020) 1490–1546 — negative edge energies (regen), battery capacity constraints, and the speed-consumption trade-off https://link.springer.com/article/10.1007/s00453-019-00655-9 ; Baum et al. "Shortest Feasible Paths with Charging Stops" (SIGSPATIAL 2015) https://dl.acm.org/doi/10.1145/2820783.2820826 (A).

Pros: hybrid keeps physical extrapolation while correcting bias from data; ML gives best in-distribution accuracy for free-flow/congestion effects.
Cons: hybrid needs a fleet or telemetry to fit; ML needs training data we do not have for the Ioniq 5 and is opaque on device.

### 5.4 Recommendation for ABRP-native

Adopt **hybrid = physics core + calibrated scalars**:

1. Physics core per link (constant-speed closed form, plus kinetic term from the speed profile's Δv, grade from the DEM): `E_link = [ (Crr·m·g·cos θ + m·g·sin θ + ½ρ·CdA·(v+v_headwind)²)·d + ½·m_eff·Δ(v²) ] / η_drive` with negative results multiplied by `η_regen` instead, plus `(P_aux + P_hvac(T))·t`.
2. Vehicle Model = §2 parameters; three calibration scalars exposed to the user/telemetry: `k_aero` (multiplies CdA), `k_roll` (multiplies Crr), and `k_hvac`.
3. Temperature: ρ from T and altitude; `P_hvac(T)`; a mild cold penalty on η_drive below 5 °C (1–3 %) to reflect the −10 °C EV Database scenarios once HVAC is accounted for.
4. Validate against §3 targets (90/110/120 km/h mild, 110 km/h cold, 70 mph cold) before release; then let the user's own trips refit the three scalars (this is the same idea as a reference-consumption scalar, but with physically separable knobs).

Open items for a follow-up ticket: (a) a direct measurement or CFD-derived frontal area; (b) a MY22 cold-battery Charging Curve table (only the 30-min/147 kW aggregate exists publicly); (c) 58 kWh pack usable capacity from an EU test rather than InsideEVs' guess.

---

## 6. Source index

Manufacturer / regulatory (A)
- Hyundai Motor Europe press kit — https://www.hyundai.news/eu/models/electrified/ioniq-5/press-kit/the-new-hyundai-ioniq-5.html
- Hyundai UK MY2023 enhancements — https://www.hyundai.news/uk/articles/press-releases/hyundai-ioniq-5-to-receive-product-enhancements-for-2023-model-year.html
- Hyundai Motor Group, aerodynamic design of IONIQ 5 — https://www.hyundaimotorgroup.com/en/story/CONT0000000000002760
- Hyundai Motor Group, heat pump / battery conditioning / regen levels — https://www.hyundaimotorgroup.com/en/story/CONT0000000000047551
- Hyundai Motor America 2022 IONIQ 5 specifications (PDF) — https://www.hyundainews.com/assets/documents/original/49532-2022Ioniq5ProductGuidespecsV4030722.pdf
- Regulation (EU) 2020/740 Annex I (tyre label classes) — https://www.legislation.gov.uk/eur/2020/740/annex/I/part/1/adopted/data.xht?view=snippet&wrap=true

Independent tests (B)
- ADAC Autotest, IONIQ 5 72.6 kWh TECHNIQ 2WD (PDF) — https://assets.adac.de/image/upload/Autodatenbank/Autotest/at6147-hyundai-ioniq-5-726-kwh-techniq-paket-2wd/hyundai-ioniq-5-726-kwh-techniq-paket-2wd.pdf
- Bjørn Nyland range test via InsideEVs — https://insideevs.com/news/523712/hyundai-ioniq5-range-test-bjorn/
- InsideEVs 70 mph cold-weather range test — https://insideevs.com/reviews/566406/hyundai-ioniq5-70mph-range-test/
- InsideEVs worst-conditions consumption — https://insideevs.com/news/512271/hyundai-ioniq5-range-worst-conditions/
- InsideEVs 72.6 kWh charging analysis (Hyundai demo data) — https://insideevs.com/news/503522/hyundai-ioniq5-fast-charging-analysis/
- InsideEVs 58 kWh charging analysis — https://insideevs.com/news/513818/hyundai-ioniq5-58kwh-charging-analysis/
- InsideEVs cold-weather DC charging — https://insideevs.com/news/578869/hyundai-ioniq-5-charging-analysis/
- electrive, preconditioning test (MY23) — https://www.electrive.com/2023/03/24/hyundai-ioniq-5-battery-preconditioning-is-it-worth-the-upgrade/
- Recurrent, heat pump range study — https://www.recurrentauto.com/research/heat-pumps
- MarkLines / Munro teardown summary — https://www.marklines.com/en/report/Munro014_202309

Aggregators (C)
- EV Database LR 2WD MY21 — https://ev-database.org/car/1478/Hyundai-IONIQ-5-Long-Range-2WD
- EV Database LR AWD MY21 — https://ev-database.org/uk/car/1479/Hyundai-IONIQ-5-Long-Range-AWD
- EVKX charging curve (77.4 kWh) — https://evkx.net/models/hyundai/ioniq_5/ioniq_5_long_range_awd_gen1/chargingcurve/
- EVKX range & consumption — https://evkx.net/models/hyundai/ioniq_5/ioniq_5_long_range_awd_gen1/rangeandconsumption/
- arenaev specs — https://www.arenaev.com/hyundai_ioniq_5_72_6kwh_rwd_2021-specs-40.php
- evspecifications — https://www.evspecifications.com/en/model/558213f
- Wikipedia, Hyundai Ioniq 5 — https://en.wikipedia.org/wiki/Hyundai_Ioniq_5
- Fastned Hyundai brand page — https://www.fastnedcharging.com/en/brands-overview/hyundai
- Tyre Reviews, Pilot Sport EV — https://www.tyrereviews.com/Tyre/Michelin/Pilot-Sport-EV.htm
- Iternio, "Reference consumption" help article (concept only) — https://abrp.featurebase.app/articles/3305478-reference-consumption
- Notebookcheck, Loveland Pass regen test — https://www.notebookcheck.net/How-much-charge-would-an-Ioniq-5-EV-recover-when-going-downhill-YouTubers-attempt-to-find-out.874176.0.html

Energy-model literature and open source (A)
- Wu, Freese, Cabrera, Kitch 2015, TR Part D 34 — https://wakengineering.com/wp-content/uploads/2015/07/Wu_et_al_2015.pdf
- Fiori, Ahn, Rakha 2016, Applied Energy 168 (VT-CPEM) — https://www.sciencedirect.com/science/article/abs/pii/S030626191630085X
- Genikomsakis & Mitrentsis 2017, TR Part D 50 — https://www.sciencedirect.com/science/article/abs/pii/S1361920915302881
- De Cauwer, Van Mierlo, Coosemans 2015, Energies 8(8) — https://doi.org/10.3390/en8088573
- Kurczveil, López, Schnieder 2014 (SUMO energy model) — https://link.springer.com/chapter/10.1007/978-3-662-45079-6_3
- SUMO Electric model docs — https://sumo.dlr.de/docs/Models/Electric.html ; MMPEVEM — https://sumo.dlr.de/docs/Models/MMPEVEM.html
- Koch et al. 2021, ITSC (MMPEVEM paper) — https://www.tkn.tu-berlin.de/bib/koch2021accurate/koch2021accurate.pdf
- NREL review of EV energy consumption models — https://www.osti.gov/servlets/purl/1824218
- NREL RouteE Powertrain (BSD-3) — https://github.com/NREL/routee-powertrain ; overview https://www2.nrel.gov/transportation/route-energy-prediction-model
- Baum et al. 2020, Algorithmica — https://link.springer.com/article/10.1007/s00453-019-00655-9 ; SIGSPATIAL 2015 — https://dl.acm.org/doi/10.1145/2820783.2820826
- evnav (MIT) — https://github.com/giraldeau/evnav
- MDPI WEVJ 17(4):168, heat pump vs PTC dyno study — https://doi.org/10.3390/wevj17040168
