# OBD EV dataset corpus — OBDb coverage, test vectors, licensing (issue #73)

Research note, 1 Sep 2026, wayfinder map [#72](https://github.com/arodroz/abrp/issues/72). Question:
which community OBD EV datasets can supply Telemetry Profiles and vector-validation for EVs we
don't physically own? Audits **OBDb** (github.com/OBDb) front and center, plus the secondary corpus
named in the ticket (JejuSoul, Esprit1st, OVMS, evDash, SoulEVSpy). This is fact-finding only, for a
later separate Telemetry Profile format-decision ticket (this ticket blocks
[#74](https://github.com/arodroz/abrp/issues/74)) — it does not choose a format or a licensing lane.

Vocabulary follows `CONTEXT.md`: a **Telemetry Profile** is the data file describing how to read one
vehicle's live telemetry over OBD — which ECUs answer, which identifiers to poll, how response bytes
decode into signals, and the pack-variant constants; data, not code, since the polling/decoding
engine is generic. Validation tiers: **car-validated** (checked against a real car), **vector-validated**
(passes recorded test vectors in a replay harness), or **paper** (defined, untested). Context:
`docs/research/ioniq5-obd-telemetry.md` §4–5, which named the candidates audited here.

Method: primary sources only, two research passes dated 2026-09-01 — live `gh api` calls against
public GitHub repos (org/repo metadata, file contents, license fields), real signalset/test-case/
generations files fetched and quoted verbatim, no secondary write-ups or reliance on model memory.
Where a claim is inference rather than something explicitly documented, it is flagged as such.

---

## Summary table

| Dataset | Format | EV / Ioniq 5 coverage | Battery-domain signals | Test vectors | License |
|---|---|---|---|---|---|
| **OBDb** (github.com/OBDb) | JSON `signalsets/v3` + YAML test cases, one repo per vehicle | ~83 BEV-named repos of ~653 model repos (~13%); Ioniq 5, EV6, IONIQ 6, EV9, Kona Electric, Niro EV, Taycan, ID.4, Bolt well-populated | Yes, in populated repos (23/24 sampled) — but 60% of sampled EV repos are empty stubs, including **all four Tesla repos** | Yes — recorded multi-frame response bytes + expected decoded values, per command per model-year; 97% of Ioniq 5 commands have ≥1 test case | **CC-BY-SA-4.0** on vehicle-data + schema repos; not org-wide (some tooling repos are Apache-2.0 or unlicensed) |
| **JejuSoul/OBD-PIDs-for-HKMC-EVs** | Torque Pro CSV | Kona/Niro EV, Ioniq EV (28 kWh)/HEV/PHEV, Niro HEV/PHEV, Optima PHEV, Ray EV, Soul EV — **no Ioniq 5** | Yes (Kona/Niro BMS CSVs) | No | None — no LICENSE file (all-rights-reserved by default) |
| **Esprit1st/Hyundai-Ioniq-5-Torque-Pro-PIDs** | Torque Pro CSV (2 files) | Ioniq 5 AWD only, 74 kWh & 77 kWh variants | Yes, extensive (per-cell voltages, 16 module temps, SoH) | No | None — no LICENSE file (all-rights-reserved by default) |
| **OVMS** (openvehicles/Open-Vehicle-Monitoring-System-3) | C++ firmware module | Ioniq 5 + Kia EV6 jointly | Yes, broad (BMS, cell max/min, temps, aux, LDC, TPMS) | No dedicated test-vector files found | **MIT** (root LICENSE; GitHub's auto-detected repo-level field is inconclusive) |
| **evDash** (nickn17/evDash) | C++/ESP32 firmware | Ioniq 5/6 + EV6 share one "eGMP" parser; EV9 separate | Yes, broad, plus crowd-sourced field captures | Partial — hardcoded demo hex-response snippets, not a portable vector file | **MIT** (confirmed via API + LICENSE file) |
| **SoulEVSpy** (langemand/SoulEVSpy) | Java/Android app | Kia Soul EV 2014–2018 + related older Hyundai/Kia EVs — **does not cover Ioniq 5/6/EV6/EV9** (last commit 2020-10-23, predates all of them) | Yes, for the cars it covers | Yes — real recorded multi-message session logs + a replay harness, but only for pre-Ioniq5 cars | **Apache-2.0** (confirmed via API + LICENSE file) |

---

## 1. OBDb — EV coverage census

**Total repos.** `gh api orgs/OBDb/repos --paginate -q '.[].name' | wc -l` → **740**, cross-checked
against `gh api orgs/OBDb -q '.public_repos'` → **740**.

Not all 740 are per-vehicle-model repos. Four non-model categories were identified and subtracted:

- **73 bare brand-name repos** (`Tesla`, `Ford`, `Volkswagen`, `Hyundai`, `Kia`, …) generated from a
  `.make-template` scaffold, holding brand-wide shared signals rather than one model. Population
  varies wildly: `Tesla` = 0 commands (its `README.md` literally reads "`# Make template`"), `Volvo`
  = 2, `Mercedes-Benz` = 11, `Honda` = 36, `BMW` = 46, `Hyundai` = 79, `Kia` = 79, `Toyota` = 168,
  `Ford` = 204, `Volkswagen` = 945 commands.
- **7 dot-prefixed org/tooling repos**: `.github`, `.meta`, `.vehicle-template`, `.schemas`,
  `.make-template`, `.claude-skills`, `.devcontainer`.
- **5 community web/tooling repos**: `obdb.community`, `editor.obdb.community`,
  `logformatter.obdb.community`, `pidhunter.obdb.community`, `vscode-obdb`.
- **2 cross-model standard repos**: `SAEJ1979` (the generic OBD-II standard-PID reference) and
  `NissanInfiniti` ("Standard OBD signals for Nissan/Infiniti vehicles", 51 commands).

87 non-model repos subtracted from 740 → **≈653 genuine per-vehicle-model repos**.

**EV (BEV) count.** Name-matching the 740 repo list against known BEV nameplates (Tesla, Ioniq
5/6/Electric, Kona Electric, EV3/6/9, Niro EV, Leaf, Ariya, ID.3/4/5/7/Buzz/e-Golf, ZOE, Megane
E-Tech, Bolt EV/EUV, all Model 3/S/X/Y + Cybertruck, e-tron/Q4/Q8, i3/i4/i5/iX/iX3, Taycan, I-PACE,
EQA/EQB/EQE/EQS, LYRIQ, Enyaq, Solterra, bZ4X, Mach-E, F-150 Lightning, Corsa-e, EX30, MG4/ZS EV,
Born, R1S/R1T, Polestar 2/3/4, Atto 3/Dolphin Mini, and more), manually excluding both false-positive
substring hits (`Hyundai-i30`, `Toyota-Matrix`, `Lincoln-Corsair`) and PHEV/HEV nameplates
(`BMW-i8`, `Jeep-Wrangler-4xe`, `Toyota-Prius-Prime`, the large non-plug-in-hybrid Toyota/Lexus/Honda
lineup) gives **≈83 BEV-named model repos**, roughly 13% of the ~653 model repos.

**Battery-domain sampling.** 60 of the ~83 BEV repos (72%) were sampled by fetching each repo's real
`signalsets/v3/default.json` and counting commands, `path`-starts-with-`"Battery"` signals, and
battery `suggestedMetric` signals (`stateOfCharge`/`Health`, `tractionBatteryCurrent`/`Voltage`/
`Capacity`/`Efficiency`):

- **36/60 (60%) are empty stubs** — literally `{ "commands": [] }`. Confirmed directly on several,
  e.g. [OBDb/Tesla-Model-3](https://github.com/OBDb/Tesla-Model-3/blob/main/signalsets/v3/default.json)
  returns exactly `{ "commands": [\n\n]\n}` (`pushed_at: 2026-01-30`). **This includes all four Tesla
  model repos**, both Rivian repos, Lucid-Air, Fisker-Ocean, Cadillac-LYRIQ, Mercedes-Benz-EQS/EQE,
  Volvo-EX30, Skoda-Enyaq, Subaru-Solterra, Cupra-Born, Honda-e, BYD-Atto-3/Dolphin-Mini,
  Renault-ZOE/Megane-E-Tech, BMW-iX, Audi-e-tron, Kia-EV3, Polestar-3, Volkswagen-ID3/ID-Buzz,
  Nissan-ARIYA, Chevrolet-Blazer-EV/Silverado-EV, and more. **Repo existence in OBDb does not imply
  any decoded content exists.**
- **24/60 (40%) have real signal content**, and **23/24 (96%) of those** carry battery-domain
  signals. The one exception, `BMW-i4`, is itself only a 2-command/3-signal near-stub with no
  battery signals.
- Best-populated EV repos found: **Porsche-Taycan** (593 Battery-path signals of 653 total, across
  168 commands), **Kia-EV6** (203/210), **Hyundai-IONIQ-6** (203/209), **Kia-EV9** (191/204),
  **Kia-Niro-EV**/**Hyundai-Kona-Electric** (133/146 each), **Volkswagen-ID.4** (211/290 across 251
  commands), **Chevrolet-Bolt-EV/EUV** (106/107 each).

Takeaway: among EV-named repos that have *any* data at all, battery telemetry is almost universal
(contributors clearly targeted battery PIDs specifically) — but the dominant limiting factor on
"coverage" is that a majority of nameplate repos are unpopulated placeholders, not that battery
signals are missing from populated ones.

---

## 2. OBDb — the `signalsets/v3` schema

The authoritative JSON Schema lives at
[OBDb/.schemas/signals.json](https://github.com/OBDb/.schemas/blob/main/signals.json) (336 lines,
JSON Schema draft-07). It is a **closed schema**: every object (`commands[]` items, `signals[]`
items, `fmt`, `filterObject`) declares `"additionalProperties": false`, so the field lists below are
exhaustive, not just currently-observed:

- **Command-level**: `hdr, rax, eax, pri, tst, tmo, fcm1, dbg, din, dout, cmd, freq, proto, filter, dbgfilter, signals`
- **Signal-level**: `id, name, hidden, description, fmt, path, suggestedMetric`
- **`fmt`-level**: `bix, len, blsb, sign, min, max, add, mul, div, unit, nullmin, nullmax, omin, omax, oval, map`
- **`filterObject`**: only `from, to, years` (all integers, calendar years)

Real signal definitions, quoted verbatim from
[OBDb/Hyundai-IONIQ-5/signalsets/v3/default.json](https://github.com/OBDb/Hyundai-IONIQ-5/blob/main/signalsets/v3/default.json)
(34 commands, 389 unique signal IDs), command `hdr:"7E4", rax:"7EC", cmd:{"22":"0101"}` — the same
DID our own context doc calls the workhorse frame:

```json
{ "id": "IONIQ5_HVBAT_SOC", "path": "Battery",
  "fmt": { "bix": 32, "len": 8, "max": 100, "div": 2, "unit": "percent" },
  "name": "HV battery charge, dashboard", "suggestedMetric": "stateOfCharge" }
{ "id": "IONIQ5_BATTERY_CURRENT", "path": "Battery",
  "fmt": { "bix": 80, "len": 16, "max": 230, "min": -230, "div": 10, "sign": true, "unit": "amps" },
  "name": "Battery current" }
```

And from [OBDb/Porsche-Taycan](https://github.com/OBDb/Porsche-Taycan/blob/main/signalsets/v3/default.json)
(168 commands, 653 signals — sampled as the non-Hyundai EV), `hdr:"7E5", rax:"7ED", cmd:{"22":"028C"}`:

```json
{ "id": "TAYCAN_BMS_SOC", "path": "Battery", "din": "03",
  "fmt": { "len": 8, "max": 100, "unit": "percent" },
  "name": "HV battery state of charge", "suggestedMetric": "stateOfCharge" }
```

### Expressiveness verdicts

**(a) UDS extended-session prerequisites — Yes.** The schema has per-command `din`/`dout`
("diagnostic level the ECU must enter to run this command" / "...should return to after") plus a
vehicle-wide `diagnosticLevel`. Not dead schema: Porsche-Taycan sets `"din": "03"` on ~80+ commands,
including header `7E5`/DID `028C` above — while a sibling command on the *same* header (`7E5`/`1801`,
voltage) carries no `din` — structurally identical to the Ioniq 5's "ICCU 7E5 sometimes needs `10 03`
first" behavior. [Nissan-Leaf](https://github.com/OBDb/Nissan-Leaf/blob/main/signalsets/v3/default.json)
sets a vehicle-wide `"diagnosticLevel": "C0"`. The reference tooling
([`.schemas/python/can/signals.py`](https://github.com/OBDb/.schemas/blob/main/python/can/signals.py),
lines 337–338, 425) parses these into `diagnostic_level_in`/`diagnostic_level_out` — confirmed real,
not vestigial. **Caveat**: no documentation anywhere in the org (code-search for `"UDS"` or
`"DiagnosticSessionControl"` across the whole org returns 0 hits) states that `din`/`dout` are
literally UDS service `0x10` sub-function bytes — the tooling only re-serializes them into a
descriptor string (`din=03`), not actual session-control bytes. The mapping to a literal `10 03` wire
sequence is a strong structural inference (field name + the `03` value matching ISO 14229-1's
extendedDiagnosticSession), not something explicitly documented. **Open question**: neither research
pass confirmed whether OBDb's *own* Hyundai-IONIQ-5 signalset flags its ICCU/aux-SoC command (DID
`22 E0 11`, header `7E5`, per our context doc) with `din`, or leaves it unflagged — worth checking
directly before relying on it.

**(b) ISO-TP flow-control parameters — Partial.** Only `fcm1` ("whether to enable flow control;
disabled by default", boolean) and a generic `tmo` (hex timeout) exist. **No block-size or
STmin/separation-time field anywhere** — confirmed by reading the closed schema and by grepping both
downloaded signalsets for `stmin|blocksize|flowcontrol|bs` (zero matches). Because
`additionalProperties: false` applies to the command object, a block-size/STmin pair cannot be added
without changing the schema itself.

**(c) Pack-variant constants — No.** `filterObject` is closed to `{from, to, years}` — calendar years
only; confirmed empirically (only `from`/`to`/`years` keys ever appear in any `filter`/`dbgfilter` in
either downloaded file). No trim/variant/capacity key exists anywhere. The closest concept,
`tractionBatteryCapacity` as a `suggestedMetric`, decorates a **live decoded signal** (e.g. Taycan's
`TAYCAN_HVBAT_KWH`, a UDS-read value), not a static declared constant. Where pack-capacity
differences are recorded at all, it is only as **free-text prose** inside `generations.yaml`'s
`description` field (§4) — not a machine-readable field. The Ioniq 5's own 58/72.6/77.4 kWh trims
(same model years) have **no schema mechanism to be distinguished** short of contributors
hand-branching signal IDs.

**(d) Multi-frame DID layouts — Yes.** `bix`/`len` are unconstrained integers with no single-frame
ceiling. Evidence: `IONIQ5_ISOLATION_RESISTANCE` sits at `bix: 456, len: 16` (byte 59 of the
reassembled payload), and the real captured test case for that command (§3) is a literal multi-frame
ISO-TP capture — one First Frame plus 8 Consecutive Frames, 62 bytes total, far beyond a single
frame's 7–8 usable bytes.

**(e) Polling cadence / rate hints — Yes.** `freq` is a *required* numeric command property,
"frequency at which to request this command, seconds." Observed real values: `0.25` (wheel speed, 4
Hz), `1` (most battery signals), `5`, `15` (tire pressure/temperature). It is set per-command (all
signals sharing a command share one `freq`), not per-signal, and is a contributor-declared target
rather than an independently bench-validated safe maximum (see §7).

---

## 3. OBDb — test cases

Structure at
[Hyundai-IONIQ-5/tests/test_cases/](https://github.com/OBDb/Hyundai-IONIQ-5/tree/main/tests/test_cases):
one directory per model year (`2018, 2022, 2023, 2024, 2025, 2026`), each with a `command_support.yaml`
(which commands apply to that year) and a `commands/` folder of one YAML file per command.

Real example,
[`2023/commands/7E4.7EC.220101|fc=1.yaml`](https://github.com/OBDb/Hyundai-IONIQ-5/blob/main/tests/test_cases/2023/commands/7E4.7EC.220101%7Cfc=1.yaml):

```yaml
command_id: 7E4.7EC.220101|fc=1
test_cases:
- expected_values:
    IONIQ5_HVBAT_SOC: 71
    IONIQ5_BATTERY_CURRENT: 1.5
    IONIQ5_BATTERY_DC_VOLTAGE: 403.2
    IONIQ5_AUXILLARY_BATTERY_VOLTAGE: 13.5
    # ...
  response: "220101\n7EC 10 3E 62 01 01 EF FB E7 \n7EC 21 EF 8E 00 00 00 00 00 \n7EC
    22 00 0F 1D 3E 21 1E 20 \n7EC 23 1F 20 1E 1F 00 4B C3 \n7EC 24 11 C2 A9 00 00
    87 00 \n7EC 25 05 95 71 00 05 8B 78 \n7EC 26 00 04 2F 41 00 04 08 \n7EC 27 A2
    02 B4 C8 C0 00 02 \n7EC 28 E8 00 00 00 00 0B B8 "
```

There is **no separate request-bytes field** — the request is implicit in `command_id`
(header/rax/DID/flow-control flag), since it is constant for a given command; only the multi-frame
`response` and `expected_values` are recorded per case. This one file alone holds 166 recorded test
cases covering all 32 signals carried by that command. **Scope note for "vector validation"**: OBDb's
vectors validate response *decoding*; they do not separately record/validate the request bytes a
client must construct (headers, DID, flow-control setup) — that part is only implied by metadata, not
exercised as a byte-for-byte vector.

**Coverage, quantified**: the repo defines 34 commands / 389 unique signal IDs. Across all 6
year-folders there are **164 command-test YAML files** (42 unique header/DID combinations, since
renamed/superseded DIDs across facelift years are tested separately). **33 of 34 commands (97%)**
have at least one test file in some model year — the single uncovered command is `7E2.7EA` service
`21` PID `01` (gear/brake/pedal signals, non-battery). That maps to **381 of 389 signals (98%)**
belonging to a tested command.

---

## 4. OBDb — generations / model years

[`Hyundai-IONIQ-5/generations.yaml`](https://github.com/OBDb/Hyundai-IONIQ-5/blob/main/generations.yaml)
holds a `references` list (external links) plus a `generations` list, each entry
`{name, start_year, end_year, description}` (`end_year: null` = still in production). It splits by
**facelift era**, not by pack variant or trim — two entries total:

```yaml
generations:
  - name: "First Generation (Pre-Facelift)"
    start_year: 2021
    end_year: 2024
    description: "...Available with 58 kWh Standard Range or 72.6/77.4 kWh Long Range
      battery packs, in RWD (single motor) or AWD (dual motor) configurations..."
  - name: "First Generation (Facelift)"
    start_year: 2024
    end_year: null
    description: "...A larger battery pack option (84 kWh, up from 77.4 kWh) was
      introduced along with minor improvements to power efficiency and aerodynamics..."
```

This confirms our context doc's pack list (58/72.6/77.4 kWh pre-facelift, 84.0 kWh facelift) from a
primary source independent of the sources that doc already cited. Critically — and this is the same
fact as verdict (c) above from a different angle — **the pack-size differences live only inside this
free-text `description` string**. There is no `battery_kwh` or similar structured field, and the
58/72.6/77.4 kWh split is not even distinguished by year: all three coexist within the single
2021–2024 generation entry.

---

## 5. OBDb — licensing

- **Hyundai-IONIQ-5**: `gh api repos/OBDb/Hyundai-IONIQ-5 -q .license` → `{"key":"cc-by-sa-4.0",
  "spdx_id":"CC-BY-SA-4.0", "name":"Creative Commons Attribution Share Alike 4.0 International"}`.
- **Porsche-Taycan** (the 2nd EV repo sampled in §2): same → CC-BY-SA-4.0.
- **Schema/tooling repo `.schemas`** (holds `signals.json`, the Python validator, `cli.py`):
  metadata reports CC-BY-SA-4.0; the raw
  [`.schemas/LICENSE`](https://github.com/OBDb/.schemas/blob/main/LICENSE) file was fetched directly
  and opens "Attribution-ShareAlike 4.0 International", confirming the metadata.
  `.vehicle-template`, `.make-template`, and `SAEJ1979` are also CC-BY-SA-4.0.

**Variance found (genuine, not uniform org-wide)**:

- **`vscode-obdb`** (the VS Code extension) is licensed **Apache-2.0**, not CC-BY-SA-4.0 — a real
  split between "data" repos and pure-code tooling.
- **`.meta`** (contributor scripts: `create_make.sh`, `update_all_makes.sh`, etc.) has **no LICENSE
  file at all** — confirmed both by an empty `.license` field and by a root listing with no `LICENSE`
  entry.
- **`obdb.community`** (the community website's source) likewise has **no LICENSE file**.

**Verdict**: CC-BY-SA-4.0 is confirmed consistent across every vehicle-data repo and the schema-spec
repo checked, but it is **not org-wide** — auxiliary code/tooling repos vary between a different OSI
license (Apache-2.0) and no license at all.

### What this means for abrp (MIT)

Recorded here as a fact check against the licensing lane already noted in map #72: abrp's own
`LICENSE` is MIT (root of this repo). The map's Notes already state the intended handling — "Imported
profile data may carry CC-BY-SA (OBDb) — keep imported data in a separately-licensed data directory,
attribute in NOTICE.md, never paste GPL/CC-BY-SA source; decode facts (DIDs, offsets, scalings) are
freely reusable, re-implemented" — and `docs/research/ioniq5-obd-telemetry.md` (line 140) takes the
same stance for the same reason. `NOTICE.md` already exists in this repo with exactly the table shape
(`Data | Provider | Licence | Verified`) that a future OBDb (or other) data import would slot into;
this ticket does not add a row, since no import decision has been made yet — it only confirms the
mechanism is already in place and the CC-BY-SA fact is real, not assumed. This same
reuse-facts-not-files framing applies with equal or greater force to the two secondary CSV sources
below (JejuSoul, Esprit1st), which carry **no license at all** rather than a share-alike one — see §6.

---

## 6. Secondary corpus

### 6.1 [JejuSoul/OBD-PIDs-for-HKMC-EVs](https://github.com/JejuSoul/OBD-PIDs-for-HKMC-EVs)

Flat per-vehicle folders of Torque Pro CSV files (Name, ShortName, ModeAndPID, Equation, Min/Max,
Units, Header) plus a GitHub Pages doc site. Created 2016-12-09, last pushed **2021-06-10** — before
the Ioniq 5's PID sets existed. Root listing covers Hyundai Kona EV & Kia Niro EV, Ioniq EV
(28 kWh)/HEV/PHEV, Kia Niro HEV/PHEV, Optima PHEV, Ray EV, and Soul EV (27/30/64 kWh). **There is no
Ioniq 5 folder at all.** The Kona/Niro EV folder's `extendedpids/` subfolder includes
`003_Kona&Niro_EV_BMS.csv`, `004_Kona&Niro_EV_BMS_cell_data.csv`, and an `ABRP_Kona&Niro_PIDs.csv` —
an ABRP-specific PID subset, confirming this dataset was already being adapted for ABRP consumption
on the Kona/Niro platform. Its value for us is **provenance, not coverage**: the Kona/Niro BMS PIDs
here are the literal ancestor several Ioniq 5 CSVs (including Esprit1st's, below) were adapted from,
plus it covers adjacent older Hyundai/Kia EV and hybrid models OBDb's per-vehicle repos may or may
not include. **License**: `gh api repos/JejuSoul/OBD-PIDs-for-HKMC-EVs --jq '.license'` → `null`; no
LICENSE file in the root listing. The README's disclaimer paragraph ("...IS PROVIDED AS IS...") is a
liability disclaimer, not a license grant. **No license exists; all-rights-reserved by default.**

### 6.2 [Esprit1st/Hyundai-Ioniq-5-Torque-Pro-PIDs](https://github.com/Esprit1st/Hyundai-Ioniq-5-Torque-Pro-PIDs)

Two flat Torque Pro CSVs at repo root, `TorqueIONIQ5AWD74kWh.csv` and `TorqueIONIQ5AWD77kWh.csv` (269
non-comment rows in the 74kWh file). Created 2022-03-21, actively updated (last push 2026-03-12).
Ioniq 5 AWD only — no RWD file, no 58kWh base pack, no 84kWh facelift file. Fields include per-cell
voltages for all 180 cells (`0x220102`), 16 module temperatures, SoH
(`((z<8)+aa)/10` at `0x220105`, confirming that DID/header as SoH-bearing), tire pressure/temp,
odometer, and ABRP-specific derived fields. The CSV's own changelog comments are dated, direct
evidence of a real correction history:

```
~Intial version based on Kona and e-Niro EV
~20210919 V5 Fixed double cells, now 180 cells according spec (thanks to SoulEVSpy developer)
~20210927 V6 Removed PIDs with wrong Aux battery values (thanks to EV Watchdog developer)
~Removed 000_Auxillary Battery SOC,Aux SOC,2102,X,0,100,%,7E2
~20211027 V7 Adapted some Min/Max values, including ABRP (thanks to @HTWUSER for spotting)
```

This documents a wrong Aux-battery PID at **header 7E2** (not 7E5) that was removed rather than
fixed — plausibly because Torque Pro's flat CSV format cannot express a multi-step UDS session
precondition, so the community abandoned the PID rather than solving the extended-session requirement
(flagged as inference, not confirmed). The README states outright the dataset was built to "forward
to ABRP... for real time route planning." **License**: `.license` → `null`; only `README.md` + two
CSVs in the repo. **No license exists; all-rights-reserved by default.**

### 6.3 [OVMS — Hyundai Ioniq 5 / Kia EV6 module](https://github.com/openvehicles/Open-Vehicle-Monitoring-System-3/tree/master/vehicle/OVMS.V3/components/vehicle_hyundai_ioniq5)

C++ firmware component (not CSV) — a live, running vehicle-integration module, very actively
maintained (pushed 2026-08-30). [Rendered docs](https://docs.openvehicles.com/en/latest/components/vehicle_hyundai_ioniq5/docs/index.html)
confirm scope: "Hyundai Ioniq 5 / Kia EV6... Vehicle Type: HION5... Credits: EVNotify." Metrics
include BMS SoC/SoH, cell voltage max/min + cell number, per-zone battery temps, pack power, aux
(12V) SoC, LDC (DC-DC converter) voltage/current/temperature, OBC pilot duty, door/seatbelt/light
state, TPMS, and trip energy. The poll table (`vehicle_ioniq_polls[]`) confirms header pairs matching
our context doc: `0x7e4→0x7ec` for BMS DIDs `0x0101`–`0x010C`, `0x7e2→0x7ea` for VMCU,
`0x7a0→0x7a8` for TPMS/BCM. **What it adds beyond OBDb**: a working reference decoder with explicit,
empirically-tuned **per-vehicle-state polling intervals** — see §7 — and broader non-battery
telemetry (LDC health, OBC pilot duty, door/seatbelt/light state) than a static signalset captures.
It implements a generic UDS-extended-session mechanism, but only for IOCTRL write commands (lights,
locks) — not for reading aux battery data. Notably, **this module reads aux/12V battery SoC/voltage
from VMCU replies at `0x7ea`, not from an ICCU read at `0x7e5`** — a structurally different path that
sidesteps the extended-session requirement entirely (see §7). The poll-list file header credits both
the 2022 Ioniq5 author and a 2019 predecessor for the earlier Kona/Kia module it was adapted from —
another explicit cross-project lineage trail. **License**: GitHub repo-level metadata reports
`{"key":"other","spdx_id":"NOASSERTION"}`, but the actual root `LICENSE` file is a plain **MIT**
grant ("Copyright (c) 2011-2017 Open Vehicles... Software which uses other licenses will be annotated
appropriately"), and the Ioniq5-specific source files carry that same MIT-style header verbatim.
Effectively MIT for this component; the metadata/file discrepancy is flagged as a minor curiosity
(likely the "other licenses will be annotated" caveat confusing GitHub's detector), not a real
ambiguity — though not every file in the whole monorepo was checked.

### 6.4 [nickn17/evDash](https://github.com/nickn17/evDash)

C++/PlatformIO firmware for ESP32 (M5Stack) boards, one file per vehicle
(`CarHyundaiEgmp.cpp/.h` for the shared E-GMP platform: Ioniq 5, Ioniq 6, EV6; `CarKiaEV9.cpp/.h`
separately; also BMW i3, Peugeot e-208, Renault ZOE, VW ID.3/e-up!). Actively maintained (pushed
2026-08-25) with a 936-line `RELEASENOTES.md` documenting field-driven fixes. Documents the ECU map
in-source (`0x07E4` BMS DIDs, `0x07E5` "Onboard charger"/ICCU) and models the 84 kWh facelift pack
and the Ioniq 6's 53 kWh variant with different cell/temp-sensor counts. **What it adds beyond
OBDb**: the most explicit documented handling of the UDS extended-session dance of the five secondary
projects (see §7), field-validated numeric sanity ranges per signal ("AUX voltage 9.0..16.5 V"), a
documented BLE packet-fragmentation bug (Ioniq 6 issue #107: "the BLE4 notification handler reset the
line buffer on every notification, so an ELM327 line split across two BLE packets was discarded...
power could read once after connect and then stay frozen for the whole drive"), and a crowd-sourced
"contribute" pipeline where real captured CAN/UDS responses from users' cars feed back into demo
profiles. It also warns that holding a UDS diagnostic session open "reduces the chance that evDash
itself keeps the parked car awake" if released promptly — a real-world echo of our own 12V-safety
constraint. README recommends the `OBDLink CX BLE4` adapter specifically. **License**: confirmed
**MIT** both via API (`"license":{"key":"mit"...}`) and an actual `LICENSE` file at repo root.

### 6.5 [langemand/SoulEVSpy](https://github.com/langemand/SoulEVSpy)

**Correction to the ticket's premise**: the ticket describes this as covering "Ioniq 5/6, EV6, EV9."
Primary-source verification contradicts this. The repo is a fork of `pemessier/SoulEVSpy` (created
2016); langemand's fork's **last commit is 2020-10-23** — before the Ioniq 5 (2021), EV6 (2022),
Ioniq 6 (2022), or EV9 (2023) existed. Its own README states: "Works on Kia Soul EV with 27 to 30 kWh
battery (2014-2018), and reads some data from Kia Ray EV, Kia e-Niro, Kia eSoul 2020, Hyundai BlueOn
EV, Hyundai Ioniq EV and Hyundai Kona EV." A recursive tree listing and code search scoped to this
repo for "EV6", "ioniq5", and "BMS" returned no matches. A general web search surfaced a claim of
Ioniq5/6/EV6/EV9 support, but that appears to conflate this repo with an unrelated OVMS docs page
that appeared in the same result set — the primary source (repo content) is authoritative: **this
project does not cover the Ioniq 5 family.** As it actually exists: a Java/Android app, ELM327
command/response layer plus model-specific command sets (`BMS2019Command`, `Vmcu2019Command`, etc.)
selected per car model. **What it adds (for the cars it actually covers)**: it is the one project
among the five that ships **real, multi-message captured OBD/ELM327 session logs plus a replay test
harness** — `eSoul2020.log.txt` (330 lines) and `ioniq.log.txt` (219 lines), driven by
`io/ReplayLoop.java` and `LogFileResponder.java`, exercised by tests like `ObdBmsTest.java`. This is a
genuine full-dialogue capture-and-replay pattern — exactly the kind of artifact §7 discusses — but
only for the original Ioniq EV / e-Soul 2020, not the Ioniq 5. **License**: confirmed **Apache-2.0**
via API and an actual `LICENSE` file, inherited from and matching the upstream `pemessier/SoulEVSpy`.

---

## 7. Gaps / open questions for the format decision

**Format-expressiveness gaps in OBDb's own schema** (from §2): no ISO-TP block-size/STmin field
(verdict b, Partial) and no structured pack-variant-constant field (verdict c, No) — the 72.6 vs
77.4 vs 84 kWh distinction that matters to us lives only in `generations.yaml` free text, not in any
machine-readable field a decoder could branch on. Any format we adopt or design needs to solve this
ourselves; OBDb doesn't hand it to us.

**Open question — does OBDb's own Ioniq 5 signalset use `din`?** §2(a) showed the `din`/`dout`
mechanism is real and used elsewhere in the org (Porsche-Taycan, Nissan-Leaf), but neither research
pass confirmed whether [OBDb/Hyundai-IONIQ-5](https://github.com/OBDb/Hyundai-IONIQ-5) itself flags
an ICCU/aux-SoC command with `din` the way Taycan does. Worth a direct check before treating OBDb's
Ioniq 5 profile as already solving the ICCU quirk for us.

**No full sequential CAN-dialogue capture exists for the Ioniq 5/6/EV6/EV9 anywhere audited.** OBDb's
test cases (§3) are per-command response captures, not full session traces (car wake-up, protocol
init, interleaved polling across ECUs). Of the five secondary projects, only SoulEVSpy ships real
multi-message session logs with a replay harness — but only for the pre-Ioniq5 Soul EV/original Ioniq
EV (§6.5). evDash's demo mode has real hex snippets sourced from user captures, but as single
hardcoded per-DID responses, not a portable trace file. **Nothing in this corpus gives us a realistic
end-to-end session trace to replay against our own poller for the Ioniq 5.**

**BLE-adapter/dongle-level quirks are thin across the board.** Nobody in the corpus documents
OBDLink-specific fast paths (no hits for "STPX" in OVMS's or evDash's source, release notes, or
READMEs). evDash documents one concrete BLE-transport bug (packet fragmentation, §6.4) and names a
preferred dongle; older projects (JejuSoul, SoulEVSpy) recommend Konnwei KW902-class Bluetooth SPP
dongles for Torque Pro. None documents OBDLink CX behavior specifically, or non-standard AT command
sequences beyond the generic ELM flow-control setup our own context doc already covers.

**Polling cadence has two concrete artifacts, neither independently validated for our hardware.**
OBDb's `freq` field (§2e) and OVMS's per-PID, per-vehicle-state interval table (e.g. battery cell page
polled every 59s off/idle, 9s driving, not at all while charging) are the only two hard
cadence artifacts found. Both are contributor/maintainer judgment calls (data-cost and MCU-load
trade-offs for OVMS; a declared target for OBDb), not a bench-validated "safe max rate" for our own
OBDLink CX + Ioniq 5 combination — that remains an open empirical question tied to the map's 12V
safety constraint.

**The ICCU `10 03` extended-session quirk is documented/handled by exactly one of the six sources
audited**: evDash (§6.4), explicitly implementing the tester-present → extended-session →
wait-for-positive-response sequence for header `7E5`/DID `22E011`. OVMS avoids the problem entirely by
reading aux battery data from VMCU `0x7EA` instead of ICCU `0x7E5` (§6.3) — a different solution, not
a confirmation of the same fact. Esprit1st's changelog (§6.2) shows the community hit a related but
distinct wrong-PID problem at header **7E2** and removed it rather than fixing it. JejuSoul and
SoulEVSpy predate the Ioniq 5. Combined with the OBDb open question above, this specific quirk looks
genuinely under-covered in the wild — worth independent verification rather than assuming any one
source has already solved it for us.

**No hardware-in-the-loop or bench-validated ground truth exists anywhere in this corpus.** Every
correction found traces to community reverse-engineering, forum reports, or crowd-sourced field
captures — e.g. Esprit1st's changelog credits "SoulEVSpy developer," "EV Watchdog developer," and a
named forum user by handle, not a lab process; OVMS credits named maintainers and "EVNotify." evDash's
contribute pipeline (real captured data from many independent cars, with automated range-clamping) is
the closest thing to systematic validation, but it is crowd-sourced field validation, not bench/HIL
validation against a known-good reference. This matches our own context doc's caveat that byte offsets
should be "verified on live car or test vectors before trusting" — none of these sources raises that
bar to bench-grade proof.

**Licensing splits along a permissive/restrictive line that maps directly to reuse strategy** (facts
only, no lane decision made here): OVMS and evDash are **MIT** — directly compatible with this
(MIT) repo, so their source could in principle be referenced or adapted more directly than
fact-extract-and-reimplement. OBDb (CC-BY-SA-4.0), JejuSoul, and Esprit1st (both unlicensed,
all-rights-reserved) cannot be pasted in; only the underlying facts (DIDs, offsets, scaling formulas)
are usable, re-implemented independently, per the existing project stance (§5.3). SoulEVSpy
(Apache-2.0) sits in between — permissive enough to reference, with its own attribution/notice
obligations if code were ever adapted, but its coverage doesn't reach our car anyway (§6.5).
