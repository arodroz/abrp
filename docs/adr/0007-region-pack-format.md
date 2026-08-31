# 7. Region Pack format: single-file mmap archive of pre-merged super-regions

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/16

## Context

ADR 0001 committed to on-device CH over Region Packs "via mmap" and deferred the
format. The vertical slice (#15) supplied the measurements: LU+BE+NL = 1.55 M
junction nodes / 3.40 M directed edges, 16 s contraction on the M4, 456 ms Plan
on-device; fully-resident CH cost 266 MB for three countries, extrapolating past
the 1 GB bar for Benelux+FR+DE; streaming deserialization + mmap'd geometry cut
peak RSS 1.47 GB → 665 MB; nearest-node snapping was a hidden 3.3 s until
indexed. CH shortcut correctness does not survive stitching independently
contracted graphs at borders, contradicting the glossary's "several packs loaded
together". Turn-restriction research (#17) chose arc-expansion feeding a
node-based CH kernel, so turn support changes the pack's *entities*, not its
*layout*.

## Decision

1. **Pre-merged super-regions, never stitched.** The pipeline merges the OSM
   extracts and contracts once per curated region; one Region Pack covers the
   whole trip region. v1 catalog: `lu-dev` (format unit test) and `eu-west`
   (Benelux+FR+DE). No per-country packs, no on-device composition.
2. **Single `.rpack` file, mmap'd whole.** Header: magic, format major/minor,
   OSM snapshot epoch, region id/name, per-section offset/length/checksum.
   Sections are flat fixed-width arrays readable in place (RoutingKit-style):
   nodes (lat, lon), CSR adjacency, CH order/levels/shortcuts, edge
   aggregates, geometry, snap grid. Load = mmap + header check; resident
   memory = pages actually touched. Atomic update = replace the file.
3. **Hot/cold split.** Hot edge array carries what search touches: length,
   speed, road class, total ascent and total descent (energy is asymmetric in
   η, net climb is not enough). Cold geometry section stores polyline vertices
   as (lat, lon, elev i16 m) — paged in only for the winning route's rendering,
   SoC/elevation chart, and corridor projection.
4. **Snap grid baked in** as a pack section; open is O(1), no load-time build.
5. **Node-based v1, arc-expanded v2.** v1 ships the slice's node-based graph
   (restrictions stay deferred per ADR 0001). When navigation needs turn
   restrictions, the pipeline arc-expands (per #17) and ships format v2: same
   section layout over expanded entities plus one arc→road-edge mapping
   section. The header's major version is the entire migration story; the app
   refuses mismatched majors.
6. **Updates and hosting.** Full-pack replace, on-demand with a quarterly
   floor; no deltas. Distribution v1 is sideload (devicectl/Xcode); the format
   is URL-agnostic and range-friendly so a static bucket (Cloudflare R2) is a
   drop-in when hosting is worth having.

## Consequences

- The RoutingKit-style port and the pack format are one design: the arrays the
  query kernel reads are the bytes on disk. No deserialization step exists to
  regress; the slice's serialization-hygiene findings become structural.
- `eu-west` stays inside the 1 GB bar because residency tracks query working
  set, not pack size; the arc-expanded v2 estimate (~850–950 MB peak corridor)
  remains legal but motivates keeping the basemap's memory in check (#19).
- Chargers and tiles stay out (Charger Packs per ADR 0005, Map Packs per
  ADR 0002); their packaging decisions are unaffected.
- The fixed catalog trades download flexibility for provable correctness; a new
  trip region is a pipeline run, not an app feature.
- CONTEXT.md's Region Pack entry is amended: packs are pre-merged in the
  pipeline, never stitched on the phone.

## Format 2.0 (2026-08-31, wayfinder #65)

Carries the minimal turn-by-turn guidance tag set through to the reader, so a
later maneuver-generation ticket has data to work with. Six new sections:
`STRING_OFFSETS`/`STRING_BLOB` (an interned string table, id 0 = empty),
`EDGE_ATTRS` (unique name/ref pairs), `EDGE_GUIDE` (one attr index per
`EDGES_HOT` slot, `GUIDE_NONE` for shortcuts), `DEST_SIGNS` (sparse
destination signage keyed by edge slot), and `EXIT_REFS` (sparse
motorway-junction exit refs keyed by node id). `EdgeHot` also gains a
`guide_flags` byte (highway class + link + roundabout), carved out of what
was `_pad` -- the struct's size is unchanged. `road_class` (the energy
crate's urban-surcharge signal) is untouched; `guide_flags` is a separate,
independent byte.

The new sections are written after every v1 section, so a v1 reader's fixed
section list is still satisfied by a v2 file's first eight entries -- this
reader accepts both `format_major == 1` and `== 2`, with guidance absent
(empty accessors, `has_guidance() == false`) on the former. A future v1
reader (the installed base at this pack's rollout) refuses a v2 file via the
existing major-version check, unaffected by this change. This is why: the
live bucket and installed regions are v1; new epochs ship v2, and devices
upgrade to a v2-aware app on their own schedule.

Note this reuses the "v2" version number Decision 5 reserved for
arc-expansion (turn restrictions): that migration hasn't happened yet, and
guidance metadata doesn't need it, so it ships first under the same major.
Turn restrictions/arc-expansion, when built, will need major 3.

Deliberately excluded: `turn:lanes`, pronunciation, `mini_roundabout`, and
any baked maneuver classification. Classification (which fork is "slight"
vs. "sharp", which exit gets called out) stays route-time code reading these
raw tags, not a pipeline-baked field -- so tuning the maneuver heuristics
never bumps the pack format.
