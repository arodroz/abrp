# 8. Map Pack packaging: z14 PMTiles mirroring the Region Pack catalog

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/19

## Context

ADR 0002 committed to MapLibre Native with self-hosted Protomaps PMTiles and
deferred the offline Map Pack's packaging. The measurements arrived since:
per-country extracts at z14/z15 are LU 35/61 MB, BE 327/742 MB, NL 465 MB/1.1 GB,
FR 2.4/5.2 GB, DE 2.5/5.2 GB (#18) — eu-west ≈ 5.7 GB at z14 vs ≈ 12.3 GB at
z15. The vertical slice (#15) rendered a pipeline-merged LU+BE+NL z14 file
(786 MB) at a sustained 120 fps, overzooming past maxzoom without visible loss.
ADR 0007 fixed the Region Pack catalog to pre-merged super-regions (`lu-dev`,
`eu-west`), sideloaded now with a static bucket later, refreshed on demand with
a quarterly floor.

## Decision

1. **maxzoom 14.** MapLibre overzooms vector tiles transparently at display
   zooms 15+; z15 mostly adds building/landuse detail an EV planner doesn't
   need, at double the storage. A z15 rebuild is a pipeline flag if a later
   navigation UI wants it.
2. **The Map Pack catalog mirrors the Region Pack catalog**: one pre-merged
   PMTiles file per super-region (`lu-dev` ~35 MB, `eu-west` ~5.7 GB), merged
   in the pipeline with `tile-join`, never stitched on-device. MapLibre styles
   bind layers to one source, so one trip region = one tile source, same as
   one Region Pack.
3. **Separate artifacts, one logical install.** The app's unit of installation
   is the region id: installing `eu-west` fetches its Region Pack, Map Pack,
   and Charger Pack as three independent files. No combined archive — a
   tiles-only refresh must not re-ship the graph.
4. **Local-only tile source in v1.** The map requires the Map Pack. style.json
   is a template with a single source id whose URL is injected at runtime:
   `pmtiles://<local path>` now; an `https://` bucket URL (range requests)
   becomes a drop-in when hosting exists. No online-first mode.
5. **Refresh with the Region Pack, from the same OSM snapshot epoch.**
   On-demand with a quarterly floor, full-file replace. The pipeline stamps
   both packs with the shared epoch so the rendered map and the routing graph
   never disagree about which roads exist. Charger Packs keep their own
   monthly cadence (ADR 0005).

## Consequences

- Two pack kinds now share the catalog, the sideload/R2 story, and the update
  policy; the pipeline cuts Region Pack + Map Pack as one epoch-stamped batch.
- `eu-west` costs ~7 GB on disk all-in (tiles + graph + chargers) — accepted
  for a personal-use device; per-country flexibility was traded away
  deliberately, as in ADR 0007.
- The style template's single source id is the only seam the future hosted
  mode touches.
- CONTEXT.md's Map Pack entry is amended: one Map Pack covers the whole trip
  region, pre-merged in the pipeline, never stitched on the phone.
