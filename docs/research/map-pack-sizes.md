# Map Pack sizes — PMTiles extracts per country (issue #18)

Date: 2026-08-27. Source archive: `https://build.protomaps.com/20260827.pmtiles` (Protomaps daily planet build, spec v3, MVT, maxzoom 15, gzip). Tool: `pmtiles` CLI 1.31.2, `pmtiles extract --region=<country>.geo.json --maxzoom=N --dry-run`. Country outlines: `johan/world.geo.json` (low-resolution, metropolitan France only). Sizes are the CLI's reported archive size; tiles are region tile counts.

| Country | z13 | z14 | z15 |
|---|---|---|---|
| Luxembourg | — | 1 445 tiles, **35 MB** | 5 462 tiles, **61 MB** |
| Belgium | — | 17 312 tiles, **327 MB** | 67 994 tiles, **742 MB** |
| Netherlands | — | 24 590 tiles, **465 MB** | 96 760 tiles, **1.1 GB** |
| France (metro) | 67 603 tiles, **1.3 GB** | 267 350 tiles, **2.4 GB** | 1 063 217 tiles, **5.2 GB** |
| Germany | 51 845 tiles, **1.3 GB** | 205 002 tiles, **2.5 GB** | 815 112 tiles, **5.2 GB** |

Totals for the first slice (LU+BE+NL+FR+DE): z14 ≈ **5.7 GB**, z15 ≈ **12.3 GB**; with FR/DE at z13 and the rest at z14 ≈ **3.4 GB**.

Real extract check: Luxembourg z15 downloaded in **13.8 s** wall-clock (75 HTTP range requests, 64 MB transferred, overfetch 0.05), producing a valid 61 392 887-byte archive (5 462 entries, bounds 5.67–6.24 E / 49.44–50.13 N). Extraction cost is therefore network-bound and trivial for a build pipeline; no local planet copy is needed.

Observations for the Map Pack decision (#19):
- z15 roughly doubles z14 for every country; z13 halves z14 again for FR/DE.
- Protomaps' basemap is designed for overzooming: z14 tiles render acceptably to z16+, so z14 is the natural "full detail" cap and z13 a plausible cap for large countries.
- The Region Pack (road graph, ~1–2 GB for the slice, ADR 0001) and the Map Pack are of the same order at z14; a per-country download of Map + Region + Charger Pack is 0.1 GB (LU) to ~4 GB (DE at z14).
- Bbox extracts overestimate badly (Luxembourg bbox z15 = 100 MB vs region 61 MB); always use a region polygon.
