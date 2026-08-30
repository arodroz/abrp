# Security audit 2026-08-30 — triage at HEAD

**Triage date:** 2026-08-30, against `ea42a47` (main).
**Audited snapshot:** `4f17947` — three merges behind this triage, and critically **before** the
codebase-audit remediation commits landed, which already fixed or narrowed several findings.
Companion to [security-audit-2026-08-30.md](security-audit-2026-08-30.md); tracker issue: #56.

**Deployment context the severities must be read against** (the audit's own scope notes
acknowledge parts of this): the pack bucket is reachable **only inside the tailnet** (ADR 0011),
the app is **sideloaded on the developer's own devices** — no App Store, no third-party users —
and there is exactly **one publisher machine**. Several High/Medium ratings are conditional on
that changing; the audit's own P0 framing ("before distributing trusted navigation packs")
agrees.

## Verdicts

| ID | Audit severity | Verdict at `ea42a47` | Notes |
|---|---|---|---|
| SEC-001 pack releases hashed, not authenticated | High | **Confirmed (by design)** | ADR 0011 anchors integrity on the bucket's own sha256s; no signing, no rollback protection, styles outside the catalog. Real architectural gap; today mitigated by tailnet-only exposure. Remediation is ADR-level design work (signed release manifest, embedded public key), not a patch. |
| SEC-002 publisher can overwrite epochs / lose index | High | **Mostly fixed before the audit was read** | The H-06 remediation already added: per-object existence + md5 preflight with **abort on differing content**, `--immutable` on upload, post-upload verify, fail-closed index handling (abort on unreadable/invalid; only a genuinely absent index starts fresh), and a local mkdir lock. Residual: cross-**machine** races are excluded by the documented single-publisher assumption; no ETag-conditional writes. Residual severity: Low. |
| SEC-003 multi-file activation not atomic | High | **Confirmed** | Commit is a loop of per-file atomic replaces (`PackInstaller.swift:343`); recovery uses `try? applyReplace` (:496) and `try? saveRecord` (:516); styles are shared global filenames. The delete-record-instead-of-rollback behavior is deliberate but confirms no rollback exists. Genuine crash-consistency gap; severity in single-user practice: Medium. |
| SEC-004 catalog validation incomplete | Medium | **Confirmed** | Path/sha/leaf validation is strict (audit credits it), but kind→filename binding, cross-region leaf collisions, positive-epoch/uniqueness checks are absent as described. |
| SEC-005 no resource limits on network/parser inputs | Medium | **Confirmed** | No response-size ceilings, no free-space preflight, no received-vs-declared byte enforcement; charger JSON loads as one vector, no count/string bounds. |
| SEC-006 deep .rpack validation unused on app path | Medium | **Confirmed** | `Planner::new` calls only `Rpack::open` (`core/ffi/src/planner.rs:45`); `verify_checksums`/`verify_structure` exist and are tested but never invoked in production. Cheapest high-value fix: run both once at install/activation. |
| SEC-007 upstream data/tools not authenticated | Medium | **Confirmed** | Bootstrap installs unpinned tools; PBFs/Protomaps builds fetched without pinned digests; `--allow-partial` provenance not encoded in the catalog. |
| SEC-008 native library outside CI provenance | Medium | **Partially stale** | CI's macOS job (added post-snapshot) rebuilds the xcframework, hard-gates binding freshness (`git diff --exit-code`), and builds the app. Residual: the binary the phone runs is still a local build with no attestation/SBOM — relevant only if distribution widens. |
| SEC-009 precise midpoint sent to Open-Meteo | Medium | **Confirmed** | Full-precision lat/lon + hour on every trip-log stop; location usage string doesn't mention the third party. Cheap fix: round coordinates (~2 decimals ≈ 1.1 km; Open-Meteo's model grid is coarser anyway) and disclose. |
| SEC-010 no backup/protection/retention policy | Medium | **Confirmed** | Trip logs + multi-GB packs live in Documents with default backup semantics; five exact recent destinations persist in UserDefaults with no clear-all. Also a practical issue: eu-west (~10 GB) inflating device backups. |
| SEC-011 no app-owned privacy manifest | Medium | **Confirmed; App-Store-only relevance** | No `PrivacyInfo.xcprivacy` in the app target (only MapLibre's). App Store distribution is explicitly out of scope of the current effort. |
| SEC-012 Actions lack least-privilege/supply-chain controls | Medium | **Partially stale** | CI now declares a `permissions:` block and runs `rustsec/audit-check`. Residual: actions pinned by tag, not commit SHA; no CodeQL/dependency-review. |
| SEC-013 two RustSec warnings | Low | **Confirmed; accepted risk — no upstream fix** | `bincode 1.3.3` (pipeline-only) and transitive `memmap2 0.5.10` via `osmpbf 0.3.8` — both verified in Cargo.lock. Remediation attempted 2026-08-30: `osmpbf 0.3.8` is the newest published release and itself pins `memmap2 ^0.5`, so no version bump can clear the advisory. Accepted because both crates run only in the Mac-side pipeline on trusted local inputs, never in the iOS runtime (the pack reader uses `memmap2 0.9.11`). Revisit if osmpbf releases, or replace the crate. `cargo audit` runs in CI (warnings don't fail it). |
| SEC-014 Swift 6 not reproducibly enabled | Low | **Fixed** | `project.yml` is committed at `SWIFT_VERSION: 6.0` + `SWIFT_STRICT_CONCURRENCY: complete` (M-05 remediation) and CI builds the app from a clean checkout. Residual: CI builds Debug/simulator, not Release/device. |
| SEC-015 "offline" styles contact GitHub Pages | Low | **Confirmed** | Both style templates fetch glyphs/sprites from `protomaps.github.io`. |

## Sequencing recommendation

- **Quick wins, in scope now** (small, independent): SEC-006 (invoke the existing deep
  verification at install/activation), SEC-009 (coarsen coordinates + disclose), SEC-010's
  cheap half (exclude packs from backup; "clear recents"/"delete all trip logs"), SEC-012
  residual (SHA-pin actions). SEC-013 turned out to have no upstream fix — see its row.
- **The architectural cluster** — SEC-001 + SEC-003, with SEC-004/005 as the validation spec a
  signed manifest needs anyway — is one coherent design effort ("signed, atomically-activated
  releases"), ADR-sized, worth a wayfinder charting session of its own. It becomes load-bearing
  the moment the bucket or the app reaches anyone but the developer; until then the tailnet is
  the compensating control.
- **The App-Store cluster** (SEC-011, the rest of SEC-008/SEC-010) belongs to a distribution
  effort that the decision map currently rules out of scope; record and defer.
