# Turn-by-turn guidance from our own OSM-derived graph

Wayfinder ticket: https://github.com/arodroz/abrp/issues/64 (part of #28)
Date: 2026-08-30

Turn-by-turn is in scope per the ADR 0012 amendment (38017bd) and gates M4.
This document answers the ticket's five questions from primary sources —
OSRM, Valhalla, and GraphHopper source and docs; the OSM wiki; taginfo; Apple
developer documentation; the Mapbox navigation SDK source; HERE SDK docs —
plus direct measurement of guidance-tag coverage on our own eu-west input
PBFs. Every load-bearing claim carries its source; unverifiable items are
flagged in §7.

**The five answers in one paragraph.** (1) All three open engines classify
turns from the same inputs — turn angle between edge bearings, name/ref
continuity, road-class continuity, a roundabout flag, and link (ramp) class —
with published numeric thresholds; OSRM bakes classification at graph-build
time, Valhalla and GraphHopper derive it at route time from per-edge
attributes, and GraphHopper's query-time model over CH-unpacked edges is
architecturally identical to our planner, so it is the blueprint to copy,
with OSRM's constants as the tuning library. (2) The pack needs eight
attribute families to survive the pipeline (name, ref, full highway class +
link bit, oneway, roundabout flag, destination/destination:ref,
way `junction:ref` + node exit refs, optionally turn:lanes); measured on the
real eu-west PBFs this costs **~260 MB on the 4.37 GB rpack (+6 %)**,
dominated by one 4-byte-per-edge-slot indirection, with all strings fitting
in a ~40 MB table. (3) Instruction text is everywhere a template table keyed
by maneuver type + modifier with substitutable tokens — hard-code the English
table, never concatenate names in code, and localization later is a data
problem. (4) Apple provides the TTS engine and an audio-session vocabulary
purpose-built for nav prompts (`.playback` + `.voicePrompt` + ducking with
transient activation) but no guidance engine at all — prompt timing, stale
prompt replacement, and phrasing are hand-rolled, with Mapbox's open SDK as
the reference implementation. (5) Turn-by-turn layers onto the #59–61
progress engine exactly as ADR 0012 anticipated: the maneuver list is a pure
function of the Plan computed at plan() time, a step tracker is one more
consumer of snapped distance-along-route, and replans swap the step array
atomically under the existing generation guard.

---

## 1. How the engines generate maneuvers

### 1.1 The architecture split: what runs at build time vs route time

- **OSRM bakes guidance at graph-build time.** `Extractor::run()` calls
  `ProcessGuidanceTurns()` during **osrm-extract**, after edge expansion,
  writing per-turn instructions to `.osrm.edges`, intersection bearings to
  `.osrm.icd`, and turn-lane data to `.osrm.tld`/`.osrm.tls` — before
  partition/contract ever run
  (https://github.com/Project-OSRM/osrm-backend/blob/master/src/extractor/extractor.cpp).
  Guidance data is keyed to edge-based turns and is **completely independent
  of the speedup technique**: CH and MLD consume the same pre-annotated turn
  files; contraction neither reads nor alters guidance. At query time OSRM
  only assembles and collapses steps (§1.3).
- **Valhalla bakes *attributes*, derives *maneuvers* at request time.**
  Mjolnir (tile build) stores names, use flags (ramp/turn-channel), road
  class, signs, turn lanes, internal-intersection and fork flags in tiles;
  odin builds all maneuvers per request from the computed path: "Odin
  inspects the trip leg nodes and edges in reverse order to form an initial
  list of maneuvers … collapses the initial maneuver list … and adds text and
  verbal instructions" (https://valhalla.github.io/valhalla/concepts/;
  component split per the README: Mjolnir "turning open data into Valhalla
  graph tiles", Odin "generate manoeuvres and narrative based on a path" —
  https://github.com/valhalla/valhalla).
- **GraphHopper derives everything at query time.** The graph stores only
  edge attributes (name, roundabout, road_class, road_class_link,
  street_destination…); `InstructionsFromEdges.calcInstructions` runs per
  request over the **unpacked** path edges, so it works identically with
  flexible routing, CH, and LM
  (https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/routing/InstructionsFromEdges.java).

**Consequence for us:** our router already returns the winning route as
unpacked original edges in travel order (`core/routing/src/router.rs`), and
the CSR + cold geometry give every intersection's outgoing edges and
bearings. The GraphHopper/Valhalla model — attributes in the pack, maneuver
classification as a route-time pass in `core/` — fits our design with zero
coupling to CH, and avoids OSRM's cost of storing a classification per
*turn pair* (our arc-expansion is still deferred). Recommendation: **copy
GraphHopper's `InstructionsFromEdges` shape, tuned with OSRM's constants,
plus Valhalla's sign model for signage.**

### 1.2 Turn-angle classification (exact thresholds)

- **OSRM** (0°/360° = U-turn, 180° = straight), `getTurnDirection` in
  `include/guidance/turn_instruction.hpp`: (0,60) SharpRight; [60,140)
  Right; [140,160) SlightRight; [160,200] Straight (±20°); (200,220]
  SlightLeft; (220,300] Left; (300,360) SharpLeft; else UTurn. Supporting
  constants (`include/extractor/intersection/constants.hpp`):
  `STRAIGHT_ANGLE=180`, `MAXIMAL_ALLOWED_NO_TURN_DEVIATION=3°`,
  `NARROW_TURN_ANGLE=40°`, `GROUP_ANGLE=60°`, `FUZZY_ANGLE_DIFFERENCE=25°`,
  `MERGABLE_ANGLE_DIFFERENCE=95°`, `PRIORITY_DISTINCTION_FACTOR=1.75`; and
  (`include/guidance/constants.hpp`) `DISTINCTION_RATIO=2`,
  `MAX_ROUNDABOUT_RADIUS=15 m`, `INCREASES_BY_FOURTY_PERCENT=1.4`,
  `MAX_SLIPROAD_THRESHOLD=250 m`.
- **Valhalla** (turn degree 0 = straight), LUT in `src/baldr/turn.cc`:
  kStraight 350–10; kSlightRight 11–44; kRight 45–135; kSharpRight 136–159;
  kReverse 160–200; kSharpLeft 201–224; kLeft 225–315; kSlightLeft 316–349.
  Odin's coarser relative buckets (`DetermineRelativeDirection`,
  `src/odin/maneuversbuilder.cc`): straight >329 or <31; plus forwardness
  helpers `is_forward` >314 or <46, `is_wider_forward` >304 or <56,
  `kIsStraightestBuffer=10`.
- **GraphHopper** (radians of heading delta, 0 = straight),
  `InstructionsHelper.calculateSign`: |δ|<0.2 rad (≈11°) CONTINUE; <0.8
  (≈46°) TURN_SLIGHT_*; <1.8 (≈103°) TURN_*; else TURN_SHARP_*; sign of δ
  picks left/right
  (https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/routing/InstructionsHelper.java).

### 1.3 Whether an intersection emits an instruction at all

- **OSRM** (extract time, `src/guidance/turn_handler.cpp`):
  `isObviousOfTwo` suppresses when the continuation is (a) of superior road
  class (`PRIORITY_DISTINCTION_FACTOR=1.75`), (b) perfectly straight with the
  same name, or (c) much narrower than the alternative: deviation ratio
  >1.4 (`INCREASES_BY_FOURTY_PERCENT`) *and* difference >25°
  (`FUZZY_ANGLE_DIFFERENCE`). `isEndOfRoad` (T-junction): right within 40°
  of 90°, left within 40° of 270°, separated by >80°. At query time the step
  list is post-processed (order in `include/engine/api/route_api.hpp`):
  collapse segregated turns → trim ≤1 m start/end segments → roundabouts →
  `collapseTurnInstructions` (`MAX_COLLAPSE_DISTANCE=30 m`,
  `include/engine/guidance/collapsing_utility.hpp`) — merging sliproad steps
  into the following turn, suppressing name oscillations and staggered
  intersections → lane anticipation → suppress short name segments.
- **Valhalla** (request time): `CanManeuverIncludePrevEdge` starts a new
  maneuver on travel-mode change, roundabout enter/exit, turn-channel or
  internal-intersection state change, exit sign, ramp state change, fork,
  tee, u-turn, or name change; `Combine()` then collapses turn channels
  (`kShortTurnChannelThreshold=0.036 km`), internal intersections, same-name
  straight continuations, and "obvious" maneuvers. A non-fork node emits a
  maneuver only if the path edge is not forward while a forward traversable
  intersecting edge exists, or is forward but not straightest (buffer 10°)
  while a forward traversable significant-class edge exists
  (`IsIntersectingForwardEdge`, `src/odin/maneuversbuilder.cc`; also
  `kShortForkThreshold=0.05 km`, `kShortContinueThreshold=0.6 km`).
- **GraphHopper** (query time, `InstructionsFromEdges.getTurn`): IGNORE when
  ≤1 allowed turn (unless the road itself bends >~46° with visible
  alternatives — "actual turn even though only possible turn"); suppress a
  real turn when the name is unchanged and all alternatives are ≥2× slower
  (`outgoingEdgesAreSlowerByFactor(2)`); near-straight forks become
  KEEP_LEFT/KEEP_RIGHT unless road-class + link continuity says follow the
  main road; CONTINUE if |δ|<0.1 rad and the alternative deviates >0.15 rad
  with the same name. Name comparison: two empty names are *not* similar.

### 1.4 Roundabouts

Detection is everywhere the way-level `junction=roundabout` (or `circular`)
flag — never geometry. Exit counting is a traversal count of departing
edges:

- **GraphHopper**: entering a flagged edge opens a `RoundaboutInstruction`;
  while circulating, the exit number increments whenever the current node
  has any outgoing non-roundabout edge; on exit the count is forced ≥1
  (`InstructionsFromEdges`, and
  https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/routing/util/parsers/OSMRoundaboutParser.java).
- **Valhalla**: `roundabout_exit_count` starts at 1 on entry and adds
  right-traversable outbound edges while circulating (drive-on-right);
  only kRoundaboutEnter/kRoundaboutExit — no rotary distinction.
- **OSRM** additionally classifies (extract time,
  `src/guidance/roundabout_handler.cpp`): **rotary** when uniquely named and
  radius >15 m; **roundabout turn** ("turn left at the roundabout") when ≤4
  nodes, radius <15 m, distinct exit bearings; else plain roundabout, with
  exit counting done at query time in `handleRoundabouts`.
- `highway=mini_roundabout` (node): OSRM treats it only as an obstacle/turn
  point (`src/extractor/obstacles.cpp`), Valhalla and GraphHopper ignore it
  entirely (verified absence by grep). Safe to skip in the pack.

### 1.5 Forks, merges, ramps, exits, signage

- **OSRM** fork: straightmost candidate ≤40° from straight, 2–3 roads
  separated by ≥60° (`GROUP_ANGLE`) from neighbors, compatible classes;
  motorway handler emits OffRamp/OnRamp/Merge from link-class edges (merge
  within 2×40° of straight); sliproads: ≤80° from straight, next
  intersection within 250 m scaled by class, fused into the following turn.
- **Valhalla** splits ramp semantics at build: link chains are reclassified
  from exit nodes, and a link becomes a **turn channel** (vs ramp) only if
  oneway, total length <200 m (`kMaxTurnChannelLength`,
  `valhalla/baldr/graphconstants.h`), class worse than trunk, and no exit
  signage (`src/mjolnir/linkclassification.cc`) — this drives "bear right"
  vs "take the ramp". A node-level fork flag is baked at build
  (`src/mjolnir/graphbuilder.cc`). Exits: kExitRight/Left when leaving a
  highway on a ramp or when an exit-number sign exists. Signs are stored in
  tiles from way `junction:ref` **or the motorway_junction node's `ref`**,
  `destination`, `destination:ref`, `destination:street`
  (`CreateSignInfoList`, graphbuilder.cc).
- **GraphHopper** has no ramp/exit types — forks give KEEP_LEFT/RIGHT with
  `road_class_link` continuity as the only ramp signal; destination signage
  and `motorway_junction` refs attach to instructions as extra info; unnamed
  link roads defer naming to the next major road.

### 1.6 U-turns

OSRM: the getTurnDirection fall-through near 0°/360°. Valhalla: turn degree
180 → kUturnLeft/Right by driving side; "pencil-point" u-turns on oneway
pairs with common base name at 179–226° (mirror 134–181°). GraphHopper:
merges two successive same-hand turns into U_TURN_* when the connector is
<35 m (`MAX_U_TURN_DISTANCE`), the exit edge is oneway, names match, and
total rotation is 180°±10 %.

### 1.7 Lane guidance (for scoping)

OSRM parses `turn:lanes` in the profile and matches lanes to turns at
extract (`src/guidance/turn_lane_handler.cpp`); Valhalla stores per-lane
16-bit masks in tiles (`valhalla/baldr/turnlanes.h`) and activates them per
maneuver in odin; **GraphHopper's open-source core has no turn:lanes support
at all** (verified absence). Precedent therefore exists for shipping v1
without lane guidance.

---

## 2. Data requirements through our pipeline, and Region Pack v2

### 2.1 What the importer keeps today, and what guidance needs

`pipeline/src/osm_import.rs` currently reads `highway` (drivable filter +
default speed + binary urban flag), `maxspeed`, `oneway`, and
`junction=roundabout` (only as implied oneway). Everything else is dropped.
Two structural facts work strongly in our favor:

- Ways are split at junction nodes and never merged across ways, so **every
  pack edge maps to exactly one OSM way**: way-level attributes are
  well-defined per edge with no resegmentation.
- Turn angles need no new data: entry/exit bearings come from the cold
  GEOMETRY section (per-directed-edge polylines already stored), and the
  intersecting-edge set comes from the CSR — filtered to originals via the
  existing `ch_middle_node == NONE` marker.

**Minimal tag set that must survive into the pack** (what the engines
actually read — OSRM `profiles/lib/way_handlers.lua`, Valhalla
`lua/graph.lua`, GraphHopper parsers):

| Tag | Level | Feeds | Priority |
|---|---|---|---|
| `name` | way | "onto X", continuity suppression | required |
| `ref` (semicolon-separated) | way | road numbers, motorway naming | required |
| `highway` full class + `_link` bit | way | ramp/exit/merge detection, class continuity | required (today collapsed to 1 urban bit) |
| `oneway` | way | already kept (directed edges) | done |
| `junction=roundabout\|circular` | way | roundabout enter/exit + exit counting | required (today only implies oneway) |
| `destination`, `destination:ref` | way (links/oneways) | "toward Bruxelles", exit branch | required for motorway UX |
| `junction:ref` | way | exit numbers (OSRM's source) | recommended |
| `highway=motorway_junction` `ref` | node | exit numbers (Valhalla's second source) | recommended |
| `turn:lanes(:forward/:backward)` + `lanes` | way | lane guidance | optional, defer (GraphHopper precedent) |
| `name:pronunciation`, `int_ref`, `destination:street` | way | polish | defer |
| `type=restriction` relations | relation | routing correctness, not narration | already planned (ADR 0007 pt 5, arc-expansion) |

Node tags `traffic_signals`/`stop`/`give_way` are cost inputs, not
narration, in both OSRM (time penalties via the obstacle map) and Valhalla
(no "at the traffic lights" phrases in en-US locale) — skip for guidance.
Details and wiki/taginfo citations in §7's companion notes below (Key:name,
Key:ref, Key:destination, Tag:highway=motorway_junction, Key:turn,
Relation:restriction on wiki.openstreetmap.org; global counts from the
taginfo API: name on 79.6 M ways, ref 15.1 M, destination 715 K ways,
turn:lanes 1.20 M ways).

### 2.2 Measured coverage on our own inputs

Measured directly on the five eu-west Geofabrik PBFs (2026-08 snapshots,
LU+BE+NL+FR+DE) with the importer's exact drivable-class filter and way-id
dedup (scratch tool; same filter as `osm_import.rs`):

| Measure | Value | Rate |
|---|---|---|
| Drivable ways (importer filter, deduped) | 9,755,555 | matches the pipeline's 9.76 M |
| … with `name` | 7,675,180 | **78.7 %** |
| … with `ref` | 2,452,126 | 25.1 % |
| `*_link` ways | 210,746 | 2.2 % |
| … links with `destination`/`destination:ref` | 85,580 | 40.6 % of links |
| Any way with `destination*` | 158,206 | 1.6 % |
| … with `turn:lanes` | 189,064 | 1.9 % |
| `junction=roundabout` ways | 220,907 | 2.3 % |
| `highway=motorway_junction` nodes | 22,411 (12,402 with exit `ref`) | — |
| `mini_roundabout` nodes | 17,764 | (skipped per §1.4) |
| Unique `name` strings | 1,297,018 | 22.65 MB payload, avg 17.5 B |
| Unique `ref` strings | 28,333 | 0.17 MB |
| Unique destination combos | 79,044 | 2.51 MB |
| Unique `turn:lanes` values | 2,049 | 63 KB |

The headline: names are near-universal on drivable roads (78.7 %, vs the
misleading global taginfo 21 % over all highway ways), destination signage
concentrates exactly where guidance needs it (40 % of ramp ways), and the
entire unique-string universe of Benelux+FR+DE is ~25 MB — string content is
a non-problem; the per-edge indirection is the real cost.

### 2.3 Region Pack v2: layout and size estimate

The pack is mmap'd Pod arrays (ADR 0007, `core/packs/src/format.rs`);
strings need a table. Constraints from `pipeline/src/ch.rs`: the final CSR
interleaves originals and shortcuts row-by-row (originals first within a
row, but global original-edge indices are not preserved), so a per-edge
attribute must be addressed by final edge slot. Proposed v2 sections (format
major bump, which ADR 0007 already declares as the migration story):

1. **STRINGS**: one UTF-8 blob + `u32` offset array; id 0 = empty. Holds
   names, refs, destination texts, exit refs. All string payloads measured
   below.
2. **EDGE_ATTRS**: compact records `{ name_id: u32, ref_id: u32 }` (8 B),
   one per *unique* (name, ref) pair — deduplicated, so this table is small
   (≤ unique names + refs).
3. **EDGE_GUIDE**: `u32` per `EDGES_HOT` slot indexing EDGE_ATTRS
   (`NONE` for shortcuts). This is the simple, mmap-friendly choice: cold
   section, touched only for the winning route's pages, so residency cost is
   nil; disk cost dominates the estimate. (A rank-index variant — per-node
   prefix counts of originals + a compact originals-only array — saves ~30 %
   of this section at the cost of a second indirection; not worth it at
   these sizes.)
4. **Guide flags**: full highway class (4 b) + link bit + roundabout bit +
   spare, in **one of `EdgeHot`'s three existing pad bytes** — zero bytes
   added, hot-array layout unchanged (32 B stays 32 B). The energy crate's
   `road_class` contract is untouched.
5. **DEST_SIGNS**: sparse sorted `{ edge_slot: u32, dest_id: u32,
   dest_ref_id: u32 }` (12 B) for edges carrying destination signage;
   binary-searched at route time.
6. **EXIT_REFS**: sparse sorted `{ node_id: u32, ref_id: u32 }` (8 B) for
   motorway_junction nodes (only those that survive as graph junctions).

**Size estimate (eu-west: 25,338,215 original directed edges + 27.6 M
shortcuts = 52.94 M EDGES_HOT slots; measured string stats above):**

| Section | Bytes | Basis |
|---|---|---|
| STRINGS (payload + u32 offsets) | ~31 MB | 25.35 MB measured payload, ~1.4 M strings |
| EDGE_ATTRS (unique name×ref pairs, 8 B) | ~12 MB | ≤1.5 M pairs (bounded by uniques) |
| EDGE_GUIDE (u32 × 52.94 M slots) | **211.8 MB** | the dominant term |
| Guide flags | 0 | EdgeHot pad byte |
| DEST_SIGNS (12 B, sparse) | ~3.6 MB | ~300 K directed edges with signage |
| EXIT_REFS (8 B, sparse) | ~0.2 MB | ≤22,411 nodes |
| **Total v2 delta** | **~260 MB** | **+5.9 % on the 4.37 GB rpack; +2.6 % on the 10.06 GB epoch** |

Corridor scale (3.40 M + 3.98 M = 7.38 M slots): EDGE_GUIDE 29.5 MB +
roughly a third of the string/attr tables ≈ **~40 MB on 573 MB (+7 %)**.
lu-dev: negligible (<2 MB). If turn:lanes is ever wanted, the measured data
cost is startlingly small (~3 MB sparse map + 63 KB of unique strings for
all of eu-west) — the cost of lane guidance is implementation complexity,
not pack bytes.

**What v2 deliberately leaves out:** turn:lanes (lane guidance is a
separate, sparse section when wanted — GraphHopper ships without it),
pronunciation, int_ref, mini_roundabout, named-junction narration, and any
baked maneuver classification (that stays route-time code, so heuristic
tuning never bumps the pack format — the same reason Valhalla keeps
narrative out of tiles).

Pipeline cost: the tags ride the existing Pass A way scan (they are already
in the decoded PBF blobs); the only new pipeline work is string interning
and the node-tag scan for exit refs, both trivial next to elevation and CH
prepare.

---

## 3. Instruction text and localization

All three engines converge on the same shape: **structured maneuver record →
per-locale template table keyed by (type, modifier/variant) → token
substitution**. No engine bakes sentences into graph data.

- **OSRM** emits `StepManeuver` (fields `location`, `bearing_before`,
  `bearing_after`, `type`, `modifier`, `exit`) with 16 documented types
  (`turn`, `new name`, `depart`, `arrive`, `merge`, `on ramp`, `off ramp`,
  `fork`, `end of road`, `continue`, `roundabout`, `rotary`,
  `roundabout turn`, `notification`, `exit roundabout`, `exit rotary`) and 8
  modifiers (`uturn`, `sharp right`, `right`, `slight right`, `straight`,
  `slight left`, `left`, `sharp left`); steps carry `name`, `ref`,
  `destinations`, `exits`, `rotary_name`, `driving_side`, `pronunciation`,
  `intersections[].lanes`
  (https://github.com/Project-OSRM/osrm-backend/blob/master/docs/http.md;
  the docs warn new types may appear — consumers should fall back to
  `turn`). Text lives in **osrm-text-instructions**: per-language JSON keyed
  `v5 → type → modifier → variant` with variants `default`/`name`/
  `destination` (+`exit`/`exit_destination` for off-ramps) and tokens
  `{way_name}`, `{destination}`, `{exit_number}`, `{modifier}`,
  `{rotary_name}`, `{nth}`, `{distance}`…; selection precedence
  destination+exits → destination → exits → name → default; `{destination}`
  speaks only `destinations[0].split(',')[0]`; name/ref deduped
  (`if (name === step.ref) name = ''`), ref alone on motorways; ordinals and
  lane phrases ("xo" → "Keep right") are per-locale constants; grammar =
  per-language regex rules over way names
  (https://github.com/Project-OSRM/osrm-text-instructions,
  languages/translations/en.json, index.js).
- **Valhalla (odin)** produces per-maneuver display `instruction` plus four
  verbal strings: `verbal_transition_alert_instruction` (early heads-up,
  "prepare the user for the forthcoming transition"),
  `verbal_pre_transition_instruction` (immediately before, fullest form),
  `verbal_post_transition_instruction` (immediately after, "Continue on U.S.
  2 22 for 3.9 miles"), and `verbal_succinct_transition_instruction`
  (shortened, no street names — in code, absent from the API table);
  `verbal_multi_cue` chains "…then turn left" for closely spaced maneuvers.
  Sign model: `exit_number_elements` / `exit_branch_elements` (from
  destination:ref) / `exit_toward_elements` (from destination) /
  `exit_name_elements`, each with `consecutive_count`. `street_names` vs
  `begin_street_names` distinguish "consistent along the maneuver" from
  "names at the transition". ~35 locales; templates like "Take exit
  <NUMBER_SIGN> on the <RELATIVE_DIRECTION> onto <BRANCH_SIGN> toward
  <TOWARD_SIGN>." with localized `ordinal_values`; translators reorder tags
  (https://raw.githubusercontent.com/valhalla/valhalla/master/docs/docs/api/route/api-reference.md;
  locales/en-US.json @3.4.0; docs/docs/contributing/locales.md).
- **GraphHopper** is the minimal model: integer sign codes symmetric around
  0 (CONTINUE=0, TURN_SLIGHT_RIGHT=1 … U_TURN_RIGHT=8, KEEP_LEFT=-7,
  ROUNDABOUT enter/exit ±6, FINISH=4) plus name/destination extras;
  text rendered from translation keys ("turn_onto %1$s") client- or
  server-side; "Only turn instructions are handled in the server-side
  routing engine"
  (https://github.com/graphhopper/graphhopper/blob/master/web-api/src/main/java/com/graphhopper/util/Instruction.java,
  docs/core/translations.md).

**For our EN-only app:** the Plan's steps should carry the *structured*
record (type, modifier, exit count, name_id, ref_id, dest ids, distances) —
over UniFFI as typed fields, exactly like the existing typed Plan — and one
English template table in the app (or `core/`) renders display and verbal
strings. Keep name/ref/destination/exit as separate fields (every engine
does, so locales can reorder/join later); ordinals are locale logic;
generate at least distinct pre-transition and post-transition verbal forms
(Valhalla's split) even in English. Localization later is then a template
file + ordinal table, not a data-model change.

---

## 4. iOS voice guidance

**What Apple provides:** a TTS engine and an audio-session vocabulary — and
nothing route-level. `MKRoute.Step.instructions` is a plain localized
display string; no voice variants, no timing engine
(https://developer.apple.com/documentation/mapkit/mkroute/step/instructions).
CarPlay templates are visual-only: `CPNavigationSession` /
`CPManeuver.instructionVariants` take app-supplied text/symbols and the
review criteria require the app to play its own audio; nav apps need the
managed `com.apple.developer.carplay-maps` entitlement
(https://developer.apple.com/documentation/carplay/cpmaneuver;
CarPlay App Programming Guide PDF).

**The mandated audio recipe** (CarPlay App Programming Guide, "Voice
prompts": navigation apps "must use the following audio session
configuration"): category `.playback`, mode **`.voicePrompt`** (iOS 12+,
"an app that plays short prompts… different routing behaviors when your app
connects to certain audio devices, such as CarPlay" —
https://developer.apple.com/documentation/avfaudio/avaudiosession/mode-swift.struct/voiceprompt),
options **`.duckOthers` + `.interruptSpokenAudioAndMixWithOthers`** (duck
music, pause podcasts/audiobooks;
https://developer.apple.com/documentation/avfaudio/avaudiosession/categoryoptions-swift.struct/duckothers).
**Transient activation** is the load-bearing pattern: activate the session
only when a prompt is ready, deactivate with `.notifyOthersOnDeactivation`
when done so paused spoken audio resumes ("Don't hold on to the active state
for more than few seconds if audio prompts are not playing"). Mapbox
hard-codes exactly this config and defers deactivation by 1 s so
back-to-back prompts don't thrash
(`Sources/MapboxNavigationCore/Audio/AVAudioSessionHelper.swift` in
https://github.com/mapbox/mapbox-navigation-ios). With `.playback` (no
recording) audio rides Bluetooth A2DP; the guide separately warns against
enabling recording in CarPlay. Background prompts require `UIBackgroundModes
= audio` (asserted by Mapbox's RouteVoiceController). Check
`AVAudioSession.promptStyle` before each prompt (None → silent, Short →
tone, Normal → full prompt — Apple's hint during Siri/calls); handle
`AVAudioSession.interruptionNotification` (began/ended + `.shouldResume`).

**AVSpeechSynthesizer specifics:** `speak()` enqueues FIFO (no priorities,
no replace); `stopSpeaking(at: .immediate/.word)` clears the queue;
utterances carry `rate`, `pitchMultiplier`, `volume`, `preUtteranceDelay`,
`postUtteranceDelay` (spacing knob between queued prompts); voices selected
by BCP-47 (`AVSpeechSynthesisVoice(language:)`), `.enhanced`/`.premium`
qualities exist only if the user downloaded them in Settings; delegate
`didFinish` vs `didCancel` is how you know when to un-duck; set
`usesApplicationAudioSession = true` so speech routes through the app's
`.voicePrompt` session (`false` spawns a separate system-managed session);
`mixToTelephonyUplink` can speak into an active call; `write()` renders to
buffers; **no SSML** (Mapbox: plain text is "appropriate for speech
synthesizers that lack support for [SSML], such as AVSpeechSynthesizer");
IPA pronunciation via attributed strings
(`AVSpeechSynthesisIPANotationAttribute`). All at
https://developer.apple.com/documentation/avfaudio/avspeechsynthesizer and
linked pages.

**Prompt timing is hand-rolled.** Reference numbers:

- Mapbox today bakes per-step voice instructions server-side: each step
  carries `voiceInstructions[]` with `distanceAlongGeometry` = meters before
  the upcoming maneuver at which to speak ("In a quarter mile, take the
  ramp…" at 375.7 m; on a 98 m step, a combined "Head southeast …, then turn
  right…" at step start and the bare instruction at 83 m)
  (https://docs.mapbox.com/api/navigation/directions/). The client just
  speaks whatever progress surfaces.
- Mapbox's legacy client-side heuristics (v0.6.0
  `MapboxCoreNavigation/Constants.swift`) are the hand-rolled blueprint:
  alerts by **time-to-maneuver** — medium at 70 s out (only if step ≥400 m,
  driving), high at 15 s out (step ≥100 m) — plus
  `RouteControllerManeuverZoneRadius = 40 m` as the "at the maneuver" zone
  and ≤30° heading test for completion; phrasing "In %@, %@" for early
  alerts, bare instruction at high alert, "%@, then %@" when the next step
  is short, "Continue on %@ for %@" after a maneuver.
- HERE (Navigate Edition) uses four distance/time tiers per transport mode
  and speed profile — Range, Reminder, Distance, Action
  (`ManeuverNotificationTimingOptions`); documented default: highway car
  notifications 1300 m before the maneuver
  (https://docs.here.com/here-sdk/docs/ios-navigation-voice-guidance); HERE
  composes text/timing, the app feeds any TTS.

**Priority/staleness handling (all hand-rolled; Mapbox's current SDK as
reference):** never enqueue — if speaking, stop `.immediate` and speak the
newer prompt (a closer-to-the-turn prompt always wins); track superseded
utterances so a canceled old prompt's `didCancel` doesn't un-duck mid-new-
prompt; hard-stop speech before the reroute tone; distance rephrasing
("in 200 meters" vs "now") falls out of the tiered instruction list rather
than text rewriting; muting is a flag that interrupts in-flight speech.
Notably Mapbox implements neither `interruptionNotification` observation nor
`promptStyle` — an app following Apple's guide adds both itself
(`Sources/MapboxNavigationCore/VoiceGuidance/SystemSpeechSynthesizer/SystemSpeechSynthesizer.swift`).

---

## 5. Step tracking at runtime: layering on #59–61

The ADR 0012 amendment already names the layering, and it survives contact
with the engine research intact: **banners and voice are pure consumers of
the progress engine.** Concretely:

1. **Maneuver generation is part of plan(), not of driving.** A route-time
   pass in `core/` walks each Leg's unpacked original edges (the router
   already returns them), classifies junctions per §1, and attaches to each
   Leg a step array: `{ type, modifier, exit_count, name_id→String,
   ref/dest Strings, maneuver_location, dist_from_leg_start_m }`, distances
   from the same cumulative-haversine walk the SoC scrub uses (the #43
   finding: never index-fraction math on CH-unpacked geometry). The Plan
   record over UniFFI grows a `steps` array per Leg — nothing else changes.
2. **The step tracker is one more consumer of snapped progress.** #59's
   snap already yields distance-along-Leg; current step = first step whose
   maneuver distance exceeds it; distance-to-maneuver = difference. Banner
   updates ride the same ≥1 s throttle as ETA (#60). The voice controller
   consumes distance-to-maneuver + current speed and fires tiered prompts
   (Mapbox's 70 s/15 s/40 m or HERE's four tiers as starting values),
   marking each (step, tier) spoken-once. This mirrors Mapbox's structure,
   where voice/banners hang off route progress events
   (`RouteVoiceController.handle(routeProgressState:)`).
3. **Leg advance stays the Leg stepper; steps are within-Leg.** The ~40 m
   arrival zone advancing Legs at Charging Stops (#60) is untouched; step
   index resets to 0 on Leg advance. (Same number as Mapbox's
   ManeuverZoneRadius = 40 m — a happy coincidence to keep.)
4. **Replans re-step atomically.** #61's silent replan swaps the Plan under
   the existing generation guard; because steps are a pure function of the
   Plan, the step array swaps with it — there is no separate re-step
   operation and no index reuse across generations (a step is identified by
   (plan generation, leg, index); never carry a step index across a swap).
   Gotchas from the SDKs: stop any in-flight utterance on replan
   (Mapbox hard-stops before its reroute tone — our brief "Route updated"
   toast is the visual analog); suppress the "depart"-class first
   instruction after a replan mid-drive (speak the next real maneuver
   instead); while off-route-but-not-yet-replanned, freeze banner/voice
   (progress is undefined off the polyline) — detection is already
   decoupled from replan in #61, so this is a consumer-side mute.
5. **What stays out:** no per-turn arc expansion is needed for any of this
   (restrictions change route *legality*, ADR 0007 pt 5 v2 — guidance reads
   the route after the fact); no CarPlay in this effort (entitlement +
   templates are a separate product decision); lane guidance deferred with
   GraphHopper precedent.

---

## 6. Recommended shape for the build tickets

1. **Pipeline + pack (format major bump, "rpack-2")**: importer keeps
   name/ref/class+link/roundabout/destination/junction:ref + node exit
   refs; writer emits STRINGS, EDGE_ATTRS, EDGE_GUIDE, DEST_SIGNS,
   EXIT_REFS sections + the EdgeHot pad-byte flags; reader exposes them
   zero-copy. Size per §2.3 (~+6 % eu-west). lu-dev is the format unit
   test as ever.
2. **Planner**: route-time maneuver classifier in `core/` (GraphHopper's
   getTurn shape; OSRM's angle table §1.2 and obviousness rules §1.3;
   roundabout exit counting §1.4; link/ramp handling via the class+link
   bits; Valhalla's sign fields); golden-step tests on the corridor pack
   (fixed routes asserted step-by-step, the golden-Plan pattern).
3. **App**: step tracker + banner as progress consumers (§5); voice
   controller with the Apple recipe (§4: .playback/.voicePrompt/ducking,
   transient activation, replace-don't-queue, promptStyle, interruption
   notifications); English template table (§3). drive-smoke grows step
   assertions with synthetic fixes; voice logic testable store-side by
   asserting *which* prompt would fire, not audio.

## 7. What could not be established from primary sources

- **HERE's full default tier-distance table**: legacy developer.here.com
  URLs 301-redirect and the API-reference deep links are unreachable;
  only the tier names and the 1300 m highway default are documented live.
- **Mapbox's current server-side tier table** (the exact distances behind
  voiceInstructions) is not published; only examples in the API docs.
- **A "drivable roads only" name-coverage figure from OSM primary sources**
  does not exist (taginfo's 21 % of all highway ways includes footways
  etc.) — superseded by our own measurement in §2.2.
- **AVSpeechUtterance rate constant numeric values** (commonly cited
  0.0/0.5/1.0) are not stated on Apple's doc pages.
- **Bluetooth HFP-vs-A2DP latency numbers**: no Apple primary source; only
  the structural fact that `.playback` avoids the HFP path.
- Valhalla's `verbal_succinct_transition_instruction` is verified in source
  but absent from the official API reference; Valhalla en-US phrasing was
  verified against tag 3.4.0 (master generates locales from .po at build).
- OSRM's `getInstructionForObvious` full body and `anticipateLaneChange`
  constants were not read line-by-line; exact engine consumption of
  `mini_roundabout` beyond OSRM's obstacle handling is a verified absence.
