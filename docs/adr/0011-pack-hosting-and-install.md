# 11. Pack hosting and install: self-hosted Garage bucket, epoch-addressed catalog

Date: 2026-08-29
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/37

## Context

ADR 0007/0008 deferred hosting ("sideload now, a static bucket later") and fixed
what the decision stands on: a two-region catalog (`lu-dev`, `eu-west`), one
logical install per region id as three independent files, Region + Map Packs
sharing an OSM snapshot epoch with a quarterly floor, Charger Packs monthly on
their own, formats URL-agnostic and range-friendly. The pipeline already emits a
per-region `catalog.json` with bytes + sha256 per artifact. Scale: `eu-west`
≈ 6.3 GB per epoch (~5.7 GB Map Pack; the Region Pack churns near-100 % each
epoch because CH contraction ordering cascades, so delta schemes can only ever
help the Map Pack). The driver runs a homelab (Coolify on Hetzner-class
hardware) with a Garage instance — an S3-compatible object store — already
deployed; the homelab is reachable from home Wi-Fi/VPN, not the open internet.

## Decision

1. **Host on the homelab Garage now; Cloudflare R2 is the designated switch.**
   A public-read bucket served over Garage's web endpoint behind the proxy at
   `wayfinder-packs.home.anteras.org`. The app only fetches plain HTTPS with
   Range headers; the pipeline uploads via S3 tooling (rclone). Switching to R2
   is the same layout and tooling against a different endpoint, behind the same
   hostname — the app's one seam is that base URL. LAN/VPN-only reachability is
   accepted for v1: multi-GB refreshes happen from home anyway.
2. **Immutable epoch-addressed layout with tiny mutable pointers.**
   `packs/<region_id>/<epoch>/<artifacts + catalog.json>` never changes under
   its URL — a range download resumed days later can never splice two versions.
   Discovery: mutable `packs/index.json` (region list: id, name, latest epoch,
   total bytes) → mutable `packs/<region_id>/catalog.json` (copy of the latest
   epoch's catalog, extended with artifact paths) → immutable files.
3. **Retention: exactly one previous epoch**, pruned manually once the new
   epoch is verified on-device. A rollback path without unbounded growth.
4. **Full-file replace reaffirmed; no deltas or chunking in v1** (ADR 0007
   stands). Chunked Map Pack refresh (casync-style content-defined chunks) is
   deferred to the map's fog with a measurement trigger: once two real
   `eu-west` epochs exist, measure tile-byte overlap; it graduates only if
   overlap is high and quarterly full downloads prove annoying in practice.
5. **Install/refresh UX.** A Packs section in the settings sheet lists regions
   from the index with state (not installed / downloading / installed with
   epoch date and size), plus a non-blocking map empty-state hint when nothing
   is installed. Downloads use background `URLSession` sessions (OS-managed
   resume via Range), Wi-Fi-only by default with an "Allow cellular" toggle.
   Refresh discovery is a check of `index.json` on launch with a badge;
   downloads are always user-initiated — never auto-fetch gigabytes.
6. **Install atomicity.** Region + Map Packs install all-or-nothing per region
   at a shared epoch: stage to a temp dir, verify sha256, atomic rename; a
   partial download leaves the old install fully usable. The Charger Pack may
   refresh alone on its monthly cadence.
7. **`corridor` enters the catalog first and the sideloaded copy is not
   adopted.** Publishing corridor (1.3 GB) proves bucket + catalog + installer
   end-to-end before betting on a 6.3 GB `eu-west` transfer; deleting the
   sideloaded files and reinstalling through the app is the installer's natural
   smoke test. Checksum-adoption of unmanaged files is complexity serving a
   one-time situation. Corridor retires from the catalog once `eu-west` is
   verified on-device.

## Consequences

- The hosting choice is deliberately cheap to reverse (one hostname, one
  uploader endpoint); the `index.json`/`catalog.json` shapes are the real
  commitment — they become an app-facing contract.
- Until the R2 switch, pack installs only work where the homelab resolves
  (home network or VPN) — a conscious v1 trade.
- The pipeline gains a publish step (rclone to the bucket, pointer rewrite);
  the app gains a download manager but no S3 client, no auth, and no delta
  machinery.
- CONTEXT.md gains **Catalog** as a term.
