# ABRP / Wayfinder Codebase Audit

**Audit date:** 2026-08-29  
**Revision:** `f6e83aa` on `main`, plus the untracked `prototype/` tree present during the audit  
**Scope:** Rust workspace, pack-building pipeline, iOS/Swift application, Swift package and generated FFI boundary, build/publish scripts, CI, architecture records, tests, dependencies, and repository hygiene

## Executive summary

The codebase has a strong algorithmic core and unusually good design documentation for its size. The Rust workspace is split along sensible domain boundaries, the Swift/Rust interface is deliberately coarse, the release profile is appropriate for an mmap-heavy mobile engine, and the existing Rust test suite exercises routing correctness, pack corruption, energy gates, optimiser behavior, and cache behavior. All 113 default/all-feature Rust tests passed, Clippy passed with warnings denied, both Swift package tests passed, four local real-pack golden scenarios passed, the release warm-plan gate passed at 271.7 ms, and the current arm64 simulator app build succeeded.

The main risk is not the route solver. It is the lifecycle around the large data packs. The installer claims an all-or-nothing transaction but replaces files sequentially; its background session cannot reconstruct an interrupted install after process relaunch; and an older region load can win a race after the user switches regions. The pipeline and publisher have corresponding integrity gaps: missing charger feeds can still produce a successful pack, partial builds can create an incomplete catalog, and the publish script can overwrite paths documented as immutable while racing updates to the shared index.

No critical issue or known vulnerable Rust package was confirmed. The audit records:

| Severity | Count | Meaning in this report |
|---|---:|---|
| High | 6 | Credible user-facing, data-integrity, or release-integrity failure that should be fixed before broad distribution |
| Medium | 10 | Bounded correctness, security-hardening, maintainability, or observability gap |
| Low | 4 | Reproducibility, documentation, formatting, or workspace-hygiene debt |

The recommended order is:

1. Make pack installation and active-region loading transactional and generation-safe.
2. Make pipeline/catalog/publishing failures fail closed.
3. Validate the catalog contract and surface installer/trip-log failures to the user.
4. Expand CI to enforce formatting, Swift 6 concurrency, generated binding freshness, and the arm64 app build.
5. Address dependency, licensing, documentation, and workspace hygiene.

## Audit target and caveats

The repository advanced from `879ea36` to `f6e83aa` while the audit was in progress. The final review includes the trip-log/weather work and fixture committed in `f6e83aa`, plus the pre-existing untracked `prototype/` directory. Those concurrent changes were reviewed and preserved; this audit only adds this Markdown file.

The review was evidence-driven but is not a formal security certification. It did not publish to the live object store, perform a multi-gigabyte live installer run, boot/install the app into a simulator, exercise a physical iPhone, fuzz binary formats, or calculate line/branch coverage. Live autotests such as `install-smoke`, map/editor/card/settings smoke tests, and background relaunch behavior remain outside the executed scope because they require external services, installed app state, or a device.

## System map

| Area | Responsibility | Main entry points |
|---|---|---|
| `core/packs` | `.rpack` format, mmap reader/writer, structural access | `Rpack::open`, `Rpack::verify_checksums` |
| `core/routing` | CH and reference shortest-path routing | `Router` |
| `core/energy` | vehicle and leg energy model | `edge_energy_wh`, calibration types |
| `core/optimiser` | corridor assembly, charging-stop search, plan cache | `plan`, `plan_with_cache` |
| `core/ffi` | Coarse UniFFI boundary exposed to Swift | `Planner` |
| `pipeline` | OSM/elevation/charger/map ingestion and catalog output | `build_packs` |
| `app/PlannerKit` | Generated Swift bindings plus a small client wrapper | `PlannerClient` |
| `app/Wayfinder` | SwiftUI/MapLibre UI, pack lifecycle, location, trip logs | `PlanStore`, `PackInstaller`, `TripLogStore` |
| `scripts` | macOS bootstrap, XCFramework generation, pack publishing | three shell scripts |

The dependency flow is intentionally one-way:

```text
raw data -> pipeline -> versioned pack artifacts/catalog -> object storage
                                                        -> PackInstaller
                                                        -> Documents
                                                        -> PlannerKit/Planner
                                                        -> PlanStore/UI
```

That architecture is sound. Most high-severity findings arise where a step currently advertises a stronger integrity guarantee than it actually enforces.

## Verification results

| Check | Result | Notes |
|---|---|---|
| `cargo test --workspace --locked` | Pass | 113 passed; five real-data/performance tests ignored by default |
| `cargo test --workspace --all-features --locked` | Pass | Also compiled the feature-gated FFI CLI and all targets; 113 passed, five ignored |
| Local real-pack golden correctness tests | Pass | Four of four corridor scenarios passed against `~/abrp-data/dist/corridor` |
| Warm-plan performance test, debug profile | Fail | 4,845.6 ms against a 1,000 ms threshold; this is a profile/harness mismatch, not a release result |
| Warm-plan performance test, release profile | Pass | 271.7 ms against the 1,000 ms threshold |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass | No Clippy warnings |
| `cargo fmt --all -- --check` | **Fail** | Formatting drift in `core/routing/src/router.rs` and `pipeline/src/chargers.rs` |
| Rust dependency advisory scan | Pass with warnings | No known vulnerabilities; two advisory warnings described in M-09 |
| `swift test --package-path app/PlannerKit` | Pass | Two tests; relies on the local ignored XCFramework and local corridor data |
| Default arm64 iOS simulator build | Pass | iPhone 17 Pro / iOS 26.5, Debug, signing disabled |
| Swift 6 complete-concurrency app build | **Fail** | `PackInstaller`'s actor-isolated `URLSessionDownloadDelegate` conformance does not satisfy Swift 6 requirements |
| Generic dual-architecture simulator build | Fail, documented limitation | The local XCFramework has an arm64 simulator slice only; `app/README.md` already requires `ARCHS=arm64` |
| `bash -n` on all three scripts | Pass | Syntax only; `shellcheck` was not installed |
| `git diff --check` | Pass | No whitespace errors in the pre-existing working-tree diff |

### Test-suite shape

The 113 Rust tests break down as follows:

| Crate/suite | Passed |
|---|---:|
| Energy | 10 |
| FFI | 9 |
| Optimiser | 28 |
| Pack model/format | 16 |
| Pipeline | 35 |
| CH correctness | 4 |
| `.rpack` round-trip/corruption | 6 |
| Routing | 5 |

This is good coverage of the pure Rust domain. The asymmetry is at the product boundary: there are only two Swift package tests, and the app's meaningful smoke tests are launch-argument autotests that are not run in CI.

## High-severity findings

### H-01 — The installer commit is not all-or-nothing

**Evidence.** The file header and install documentation promise that a failed or cancelled install leaves the previous install fully usable ([`PackInstaller.swift` lines 1–7](../app/Wayfinder/Sources/PackInstaller.swift#L1)); ADR 0011 requires staging, checksum verification, and an atomic rename ([ADR 0011 lines 50–53](adr/0011-pack-hosting-and-install.md#L50)). The implementation stages and hashes downloads correctly, but then removes and moves each destination one at a time before fetching styles and saving the installed record ([`PackInstaller.swift` lines 195–207](../app/Wayfinder/Sources/PackInstaller.swift#L195)).

If the second/third move, either style fetch, or record write fails, the filesystem can contain a mixture of old and new pack files while metadata still describes the previous install. Removing a destination before moving its replacement also creates a direct loss window.

**Impact.** A transient I/O or network failure during commit can make a previously working region unloadable or pair a Region Pack, Charger Pack, and Map Pack from different builds. This violates the central integrity invariant for multi-gigabyte resumable installs.

**Recommendation.** Stage the complete region—including styles and its installed record—under a versioned directory, then switch a single small pointer or directory entry atomically. If the platform cannot atomically exchange directories in the desired layout, retain backups and implement a tested rollback. Inject failures after every commit step to prove the old version remains loadable.

### H-02 — An older asynchronous region load can overwrite the active region

**Evidence.** `setActiveRegion` clears state and immediately starts another load ([`PlanStore.swift` lines 186–208](../app/Wayfinder/Sources/PlanStore.swift#L186)). `load` launches an unstructured detached task and captures only file paths ([lines 211–236](../app/Wayfinder/Sources/PlanStore.swift#L211)). `didLoad` and `didFail` apply results without checking the region, task identity, or a load generation ([lines 239–249](../app/Wayfinder/Sources/PlanStore.swift#L239)). The separate `generation` protection used for replanning does not protect pack loading.

**Impact.** If region A loads slowly, the user switches to region B, and A completes last, the store can expose A's planner and charger set while `activeRegion`, map style, and pack status say B. Plans can then use the wrong road graph and charger inventory.

**Recommendation.** Keep a cancellable load task and monotonically increasing load generation. Capture `(region, generation)` before detaching and guard both before publishing either success or failure. Add a deterministic test with two controlled loaders completing in reverse order.

### H-03 — The background URLSession cannot recover an install after process relaunch

**Evidence.** The session uses a background identifier ([`PackInstaller.swift` lines 50 and 252–258](../app/Wayfinder/Sources/PackInstaller.swift#L50)), but the region, continuation, and destination are held only in memory ([lines 79–82](../app/Wayfinder/Sources/PackInstaller.swift#L79)). Download callbacks require the in-memory `pendingDestination` and otherwise discard the result ([lines 328–352](../app/Wayfinder/Sources/PackInstaller.swift#L328)). No persisted transaction descriptor, task-to-artifact mapping, session restoration on launch, or application background-session completion handler exists. Apple's [background-download lifecycle documentation](https://developer.apple.com/documentation/foundation/downloading-files-in-the-background) requires recreating the session with the same identifier on launch and handling the application delegate's background-session completion callback.

**Impact.** iOS can continue or relaunch a background transfer, but the app cannot associate the callback with a region/artifact or resume the async continuation after its process was killed. A core promise in ADR 0011—OS-managed resume for multi-gigabyte downloads—is therefore not durable across relaunch.

**Recommendation.** Persist the install transaction and each URLSession task identifier before starting the task. Recreate the background session during application launch, enumerate/reconcile outstanding tasks and staged files, and implement the background event completion lifecycle. Test by reconstructing a new installer instance between download start and delegate completion.

### H-04 — Charger feed failures still produce a successful, possibly empty pack

**Evidence.** Each region declares required national feeds ([`build_packs.rs` lines 82–143](../pipeline/src/bin/build_packs.rs#L82)). Every parser error is downgraded to a warning ([lines 257–313](../pipeline/src/bin/build_packs.rs#L257)); a missing Region Pack likewise only disables geographic filtering ([lines 320–331](../pipeline/src/bin/build_packs.rs#L320)). The job then writes whatever records remain, including zero records, and returns success ([lines 334–354](../pipeline/src/bin/build_packs.rs#L334)).

**Impact.** A changed upstream schema, missing country feed, or damaged source file can silently publish incomplete charger coverage. For `eu-west`, losing one feed can remove a country's fast chargers without failing the build; losing the map-derived bounds can also admit out-of-region phantom candidates.

**Recommendation.** Make required feeds and a valid region boundary fail closed. Record per-feed parse/kept counts and require an explicit `--allow-partial` escape hatch for intentional degraded builds. Add tests for a missing feed, malformed feed, zero-result feed, and missing Region Pack.

### H-05 — A partial build can silently create an incomplete catalog

**Evidence.** `read_existing` converts every I/O and JSON parse error into “no existing catalog” ([`catalog.rs` lines 88–94](../pipeline/src/catalog.rs#L88)). A partial run without an existing valid catalog defaults `osm_snapshot_epoch` to `0`, the Protomaps build to an empty string, and the artifact map to only the artifacts produced in that run ([lines 96–141](../pipeline/src/catalog.rs#L96)). `build_packs` always writes that catalog after any selected job set ([`build_packs.rs` lines 406–437](../pipeline/src/bin/build_packs.rs#L406)).

**Impact.** `--jobs chargers` in a fresh directory or beside a corrupt catalog exits successfully with a catalog that does not represent an installable region. If published, clients receive invalid epoch metadata or missing artifacts.

**Recommendation.** Distinguish “not found” from unreadable/invalid. Permit partial replacement only when a valid existing catalog has the same region and all required invariant fields/artifacts. Validate the final catalog before writing, and use a temporary file plus atomic rename. Add absent, corrupt, wrong-region, and incomplete-catalog tests.

### H-06 — Publishing can overwrite immutable epochs and lose index entries

**Evidence.** ADR 0011 says epoch-addressed files never change ([lines 30–35](adr/0011-pack-hosting-and-install.md#L30)), and the script repeats that guarantee ([`publish-packs.sh` lines 24–26](../scripts/publish-packs.sh#L24)). It nevertheless unconditionally uses `rclone copyto` for every epoch object ([lines 27–32](../scripts/publish-packs.sh#L27)); the official [`rclone copyto` documentation](https://rclone.org/commands/rclone_copyto/) says an existing nonidentical file is overwritten unless an option such as `--immutable` changes that behavior. The shared `index.json` is updated through an unconditional read-modify-write with no ETag/conditional write or lock ([lines 40–51](../scripts/publish-packs.sh#L40)); all read/parse failures are treated as an empty index.

**Impact.** Re-running an epoch after artifacts change can make a resumed range download splice bytes from different objects—the exact failure immutability is intended to prevent. Concurrent region publishes can overwrite one another's index changes, and a recoverable index-read failure can lead to publishing an index containing only the current region.

**Recommendation.** Refuse to publish an existing epoch unless every remote checksum matches exactly. Upload and validate all immutable objects before moving mutable pointers. Serialize index publication or use conditional writes against an observed version/ETag; never convert arbitrary read failures into an empty production index.

## Medium-severity findings

### M-01 — Installed metadata can outlive missing or corrupt files

The installed JSON record is treated as the source of truth for UI state. When its stored SHA matches the catalog, installation skips the artifact without checking that the file exists or still hashes correctly ([`PackInstaller.swift` lines 159–192](../app/Wayfinder/Sources/PackInstaller.swift#L159)). Update discovery also compares catalog metadata only ([lines 130–135](../app/Wayfinder/Sources/PackInstaller.swift#L130)), while runtime location checks only file existence under conventional names ([`Packs.swift` lines 16–27](../app/Wayfinder/Sources/Packs.swift#L16)).

A deleted, truncated, externally replaced, or differently named artifact can therefore remain “Installed,” and reinstalling an unchanged catalog will not heal it. Validate file existence, regular-file status, expected naming/size, and checksum at an explicit repair boundary. At minimum, missing files must force redownload.

### M-02 — Catalog-controlled identifiers and paths are decoded but not validated

`PackCatalog` ignores the catalog format/version fields, and fetches do not validate `index_format`, requested region versus returned `region_id`, artifact kind/filename conventions, byte counts, SHA syntax, or path ancestry ([`PackCatalog.swift` lines 7–71](../app/Wayfinder/Sources/PackCatalog.swift#L7)). `region`, `artifact.file`, and `artifact.path` are appended directly into local and remote URLs ([`PackInstaller.swift` lines 159–180](../app/Wayfinder/Sources/PackInstaller.swift#L159), [lines 277–292](../app/Wayfinder/Sources/PackInstaller.swift#L277)). A local Foundation check confirmed that appending `../escape` and standardizing the URL escapes the intended base directory.

The host is HTTPS and private-network scoped, so this is not an open-internet arbitrary filesystem issue. It is still an avoidable blast-radius increase if a catalog is malformed or its origin is compromised: paths can overwrite/delete unrelated files within the application sandbox, including other Documents data. Validate the full catalog contract before any filesystem mutation; require safe region IDs, leaf-only filenames, exact artifact names/formats, a base-host/path prefix, nonnegative sizes, and lowercase 64-hex SHA-256 values. Resolve and assert every destination remains a descendant of the intended directory.

### M-03 — Installer and deletion failures are hidden from the user

The Pack settings buttons discard all installation/update errors with `try?` ([`SettingsForm.swift` lines 224–237](../app/Wayfinder/Sources/SettingsForm.swift#L224)). Delete does the same in the UI ([line 64](../app/Wayfinder/Sources/SettingsForm.swift#L64)), while `PackInstaller.delete` itself suppresses every file-removal error and still marks the row uninstalled ([`PackInstaller.swift` lines 232–247](../app/Wayfinder/Sources/PackInstaller.swift#L232)). `lastIndexFetchFailed` is recorded but not rendered.

Checksum, network, storage, and cleanup failures can therefore look like a button that did nothing or a successful deletion. Add a small observable operation error/state and keep UI bookkeeping aligned with actual filesystem outcomes. Deletion should report partial cleanup rather than declaring success.

### M-04 — The mmap reader leaves cheap cross-section invariants unchecked

`Rpack::open` checks magic/version, table bounds, alignment, required sections, section element sizes, related array lengths, and snap-grid offsets ([`reader.rs` lines 45–263](../core/packs/src/reader.rs#L45)). It does not reject duplicate/overlapping sections or sections overlapping the header/table; it also does not validate CSR monotonicity/endpoints, edge targets/geometry ranges, reverse-edge indices, or CH-order value ranges at the reader boundary. Runtime accessors slice directly from those values and can panic ([lines 342–375](../core/packs/src/reader.rs#L342)). Full CRC verification is explicit and is called only in tests, not by the production `Planner` constructor ([`reader.rs` lines 294–306](../core/packs/src/reader.rs#L294), [`planner.rs` lines 43–55](../core/ffi/src/planner.rs#L43)).

Writer-side model validation and installer SHA-256 verification reduce normal exposure, and scanning a multi-gigabyte pack at every open would conflict with the paging design. The remaining risk applies to sideloaded, externally modified, or transactionally mixed files and becomes an app termination because the FFI profile uses `panic = "abort"`. Validate cheap metadata and index invariants during `open`; keep full checksums at install/repair time rather than necessarily on every startup. Add malformed-reader tests for each unchecked invariant.

### M-05 — The accepted Swift 6 concurrency contract is neither configured nor buildable

ADR 0004 says the app adopts Swift 6 strict concurrency ([ADR 0004 lines 13–18](adr/0004-rust-boundary-uniffi.md#L13)), but the XcodeGen project sets `SWIFT_VERSION` to `5.0` ([`project.yml` lines 32–41](../app/Wayfinder/project.yml#L32)). The default build passes. When audited with `SWIFT_VERSION=6 SWIFT_STRICT_CONCURRENCY=complete`, compilation fails because `PackInstaller`'s main-actor-isolated `URLSessionDownloadDelegate`/`URLSessionTaskDelegate` methods satisfy nonisolated protocol requirements ([`PackInstaller.swift` lines 328–360](../app/Wayfinder/Sources/PackInstaller.swift#L328)).

Decide whether Swift 6 is still the near-term contract. If yes, fix the delegate isolation deliberately—rather than suppressing it blindly—then set and enforce the language/concurrency mode in the generated project and CI. If not, amend the ADR so the weaker build is explicit.

### M-06 — Trip logging can silently save unusable capture data

Recording enters `.recording` before location authorization is known, and the store has no authorization-change or location-error handler ([`TripLogStore.swift` lines 45–65](../app/Wayfinder/Sources/TripLogStore.swift#L45), [lines 144–176](../app/Wayfinder/Sources/TripLogStore.swift#L144)). `ingest` accepts every nonnegative timestamp without enforcing monotonicity, approximate 1 Hz sampling, coordinate validity, or an accuracy threshold ([lines 118–138](../app/Wayfinder/Sources/TripLogStore.swift#L118)). Empty logs are intentionally saved, and `saveErrorMessage` is never shown anywhere outside the autotest.

With denied permissions, a CoreLocation failure, duplicate/out-of-order fixes, or poor GPS, the UI can appear to record normally and later save data that calibration cannot trust. Require/observe authorization before starting, expose capture and save failures, show sample health, and define ingestion rules that match the `tlog-1` consumer. Unit-test denied permission, no fixes, duplicate timestamps, out-of-order timestamps, invalid coordinates/accuracy, and disk-write failure.

### M-07 — Destination search remains biased to the corridor after a region switch

The search completer always uses the global corridor region derived from `RegionBounds.box(for: "corridor")` ([`SearchModel.swift` lines 7–20](../app/Wayfinder/Sources/SearchModel.swift#L7), [lines 33–38](../app/Wayfinder/Sources/SearchModel.swift#L33)). It is not updated from `PlanStore.activeRegion`.

After selecting `eu-west`, suggestions can remain biased toward Benelux rather than the active pack. Inject the active region/bounds into the search model and update the completer when the region changes; test corridor and eu-west centers/spans.

### M-08 — CI covers the Rust core but not release readiness

The sole CI job runs Rust tests and Clippy on Ubuntu ([`.github/workflows/ci.yml` lines 8–17](../.github/workflows/ci.yml#L8)). It does not run formatting—the check currently fails—advisory policy, all features explicitly, the release performance gate, Swift tests, generated-binding freshness, XcodeGen generation, Swift 6 concurrency, an arm64 app build, or script linting. The Swift tests also require a gitignored local XCFramework and corridor pack, so they are not reproducible from checkout alone. The XCFramework-generation script overwrites a committed `Planner.swift` beside an ignored binary artifact, with no automated stale-artifact check ([`build-xcframework.sh` lines 5–11 and 68–79](../scripts/build-xcframework.sh#L5)).

Add a cheap Rust format/advisory stage and a macOS stage that builds the XCFramework from source, checks the generated binding diff, runs Swift tests, regenerates the Xcode project, and builds the arm64 app under the intended concurrency mode. Keep device/live-data gates separate, but make their invocation and expected profile explicit.

### M-09 — Dependency policy has two advisory warnings

The lockfile scan found no known vulnerabilities. It did report:

- `bincode 1.3.3` is unmaintained under [RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141.html); the advisory has no patched 1.x release.
- `memmap2 0.5.10` is affected by an unsound range-calculation API under [RUSTSEC-2026-0186](https://rustsec.org/advisories/RUSTSEC-2026-0186). The workspace's direct `memmap2` is already 0.9.11; the old copy arrives transitively through `osmpbf 0.3.8` into the offline pipeline/dev dependency graph.

No use of the advisory's affected range methods was confirmed, so this is exposure rather than a demonstrated exploit. Track an `osmpbf` update/replacement and decide whether to accept, replace, or isolate `bincode`. Add an advisory policy to CI so new vulnerabilities fail while explicitly reviewed warnings remain visible.

### M-10 — The declared MIT license has no license text in the repository

The workspace metadata declares `license = "MIT"` ([`Cargo.toml` lines 12–15](../Cargo.toml#L12)), but no `LICENSE`, `LICENSE-MIT`, or `COPYING` file is tracked. `NOTICE` provides attribution but is not the project license grant.

Add the intended license text and ensure application/distribution notices include third-party obligations. This is small but should precede external distribution.

## Low-severity findings

### L-01 — Formatting is not clean

`cargo fmt --all -- --check` reports formatting diffs in `core/routing/src/router.rs` and `pipeline/src/chargers.rs`. No source formatting was changed during this audit. Run the formatter and add the check to CI so drift cannot recur.

### L-02 — Entry-point documentation is sparse and internally stale

The root [`README.md`](../README.md) contains only a title and “ABRP.” The app README says search/results/settings are future work and that there is no in-app installer, then later documents the implemented installer ([`app/README.md` lines 3–7 and 37–66](../app/README.md#L3)). It also carries detailed manual smoke-test instructions without one canonical “verify this checkout” command.

Replace the root placeholder with a short architecture, prerequisites, data-artifact model, build/test matrix, and links to ADRs. Update the app README to match the current feature set and clearly distinguish automated CI checks from local/device/live-service gates.

### L-03 — Toolchain/bootstrap reproducibility is incomplete

There is no `rust-toolchain.toml`; CI and the bootstrap script follow moving `stable`, and the bootstrap explicitly updates it ([`bootstrap-mac.sh` lines 15–21](../scripts/bootstrap-mac.sh#L15)). The app setup requires XcodeGen and publishing requires rclone, but the bootstrap installs neither while installing several other pipeline tools ([lines 23–25](../scripts/bootstrap-mac.sh#L23)). Some bootstrap messaging still describes the earlier vertical slice.

Pin a known-good Rust channel/toolchain, document the supported Xcode/Swift versions, and either install or explicitly check every required command. This will make local and CI outcomes comparable.

### L-04 — The workspace contains a large untracked prototype tree

`prototype/` is untracked, is not covered by `.gitignore`, and measured approximately 754 MB across 3,129 files, largely build/package artifacts. It makes `git status`, searches, editor indexing, backups, and accidental staging noisier. Because it may be user-owned work, the audit did not remove or ignore it.

Move it outside the repository or add narrowly scoped ignore rules for the intended prototype source/build layout. Do not blanket-ignore paths until any source worth keeping is identified.

## Security and privacy observations

Positive controls already present:

- Pack and API traffic uses HTTPS.
- Downloaded artifacts are hashed with streaming SHA-256 before commit, avoiding whole-file memory use.
- The pack host is deliberately scoped to home/VPN access for v1.
- Trip Logs are documented and implemented as local, share-sheet-exportable files rather than automatic uploads.
- No embedded private key, password, or application secret was found in the reviewed source/configuration scan.
- The Rust dependency scan found no known vulnerable package.

Residual risks are concentrated in catalog trust/path validation (M-02), release-object immutability (H-06), incomplete data acceptance (H-04/H-05), and the transitive advisory exposure (M-09). SHA-256 values delivered by the same unsigned catalog protect against accidental corruption but not a compromised catalog origin; HTTPS and host security remain the trust root. For the current private v1 deployment that can be a conscious choice, but the trust model should be revisited before public hosting.

## Architecture and maintainability assessment

### What is working well

- **Deep domain boundaries.** Packs, routing, energy, optimisation, FFI, data ingestion, and iOS responsibilities are separate without excessive framework code.
- **A coarse FFI surface.** One `Planner` object crosses the language boundary, keeping record marshaling away from per-edge hot paths.
- **Performance-aware data design.** The pack reader uses mmap/zero-copy slices, checked offset arithmetic, fixed-size records, and explicit checksum verification; large-file hashing is streamed in both Rust and Swift.
- **Strong pure-core tests.** CH output is checked against a reference solver, pack corruption/truncation paths are exercised, and energy/optimiser behavior has concrete gates.
- **Measured performance work.** Research gate documents record cold/warm latency and memory, and the release warm-plan test passed comfortably in this environment.
- **Good cancellation/replan intent.** Plan generation guards and an atomic cancellation flag protect the expensive planner path, even though pack loading needs the same pattern.
- **Pinned key application dependencies.** `Cargo.lock` is tracked and MapLibre is pinned exactly to 6.29.0.
- **Useful architecture records.** Eleven accepted ADRs make intended invariants explicit enough to audit implementation drift.

### Pressure points

- `core/optimiser/src/search.rs`, `core/optimiser/src/corridor.rs`, `pipeline/src/chargers.rs`, `pipeline/src/elevation.rs`, and `PlanStore.swift` are the largest hand-written modules. Their size alone is not a defect, but future work should preserve their current conceptual boundaries rather than add more unrelated responsibilities.
- `Planner.swift` is generated and should be reviewed through a freshness check, not manually maintained.
- Operational correctness is spread across pipeline code, JSON conventions, shell publishing, remote object semantics, installer state, and local metadata. Those pieces need shared validation fixtures because individual unit tests cannot prove the end-to-end invariant.

## Test gaps mapped to findings

| Missing test/gate | Findings covered |
|---|---|
| Installer fault injection after every download/commit/style/record step | H-01, M-01, M-03 |
| Installer reconstruction after process relaunch/background completion | H-03 |
| Reverse-order region loader completion | H-02 |
| Malformed catalog/index contract and path traversal cases | M-02 |
| Required charger feed absent/malformed/zero; Region Pack absent | H-04 |
| Partial catalog with missing/corrupt/wrong-region base | H-05 |
| Publish same epoch twice; concurrent two-region publish; failed index read | H-06 |
| Reader fuzz/property cases for duplicate/overlapping sections and invalid CSR/edge indices | M-04 |
| Swift 6 complete-concurrency build | M-05 |
| Trip-log denied permission, location error, no fixes, invalid/out-of-order fixes, write failure | M-06 |
| Search bounds after active-region change | M-07 |
| Generated `Planner.swift` and XCFramework freshness | M-08 |

## Recommended remediation plan

### Phase 1 — Protect installed and published data

1. Redesign `PackInstaller` around a persisted transaction and a single atomic activation point.
2. Add a load-generation/task guard to `PlanStore`.
3. Make charger feeds, region bounds, and partial catalogs fail closed.
4. Prevent immutable epoch overwrites and make index publication conditional/serialized.

**Exit criteria:** injected install failures leave the old region usable; process relaunch resumes/reconciles downloads; stale loads cannot publish; incomplete inputs cannot produce a publishable catalog; repeated/concurrent publishing cannot mutate epochs or lose regions.

### Phase 2 — Harden contracts and make failures visible

1. Validate every index/catalog field, path, format, and region/artifact relationship before downloading.
2. Reconcile installed metadata with files and add an explicit repair path.
3. Surface pack operation errors and make deletion state truthful.
4. Add cheap reader invariants at `Rpack::open` and keep full hashing at install/repair boundaries.
5. Harden Trip Log authorization, ingestion quality, and error presentation.

**Exit criteria:** malformed metadata is rejected before filesystem mutation; missing/corrupt installed files are detected and repairable; user-visible operations cannot fail silently; malformed sideloaded packs return errors rather than panicking; unusable trip captures are not presented as successful.

### Phase 3 — Make the repository continuously reproducible

1. Move the app to the chosen Swift concurrency contract and enforce it.
2. Add format, advisory, generated-binding, Swift test, XcodeGen, and arm64 build checks to CI.
3. Pin/document toolchains and complete bootstrap prerequisites.
4. Add the MIT license text and refresh the READMEs.
5. Cleanly separate or ignore prototype build artifacts.

**Exit criteria:** a fresh supported machine can follow one documented path to reproduce all non-device checks, and CI catches every failure observed by this audit.

## Overall conclusion

Wayfinder's core computation and architectural direction are credible. The code is not broadly release-ready yet because the pack lifecycle's stated consistency model is stronger than its current implementation, and the release pipeline can successfully emit or publish states the client should never see. Those issues are concentrated and fixable without redesigning the routing/energy/optimiser core.

The most valuable next move is to treat “one valid region version becomes active atomically” as an end-to-end invariant spanning pipeline validation, immutable publishing, persisted download state, filesystem activation, and generation-safe loading. Once that invariant has executable tests, the remaining findings are conventional hardening and repository-quality work.

## Remediation record (2026-08-30)

All findings were remediated (or explicitly accepted, below) across five commits on main; every gate the audit ran is green afterwards: 141 Rust tests (`--locked`, and `--all-features`), clippy `-D warnings`, `cargo fmt --check`, the arm64 simulator app build now under **Swift 6 complete strict concurrency**, and all eight launch-argument autotest modes on the simulator, including the live `install-smoke`.

| Finding | Status | Commit | Notes |
|---|---|---|---|
| H-01 installer not all-or-nothing | Fixed | 55ee180 | Journaled roll-forward commit: full staging + verification, per-file atomic `replaceItemAt`, launch reconciliation that completes or cleanly abandons (dropping the record if bytes are mixed) |
| H-02 stale region load wins | Fixed | 55ee180 | `loadGeneration` guard mirroring the replan guard |
| H-03 no relaunch recovery | Fixed | 55ee180 | Persisted download manifest keyed by task id, session reattach + orphan resume, app-delegate background completion handler; a transfer completing across a relaunch lands and resumes its install |
| H-04 charger feeds fail open | Fixed | 8b87912 | Required feeds/bounds fail closed; explicit `--allow-partial`; per-feed counts always logged |
| H-05 silent partial catalog | Fixed | 8b87912 | Corrupt ≠ absent; partial runs need a valid same-region base; final catalog validated, written atomically |
| H-06 mutable epochs / lossy index | Fixed | bf3f4bc | md5-equality refusal to overwrite (Garage exposes no sha256 via rclone), `--immutable`, upload-before-pointers ordering, abort on unreadable index, ownership-safe lock; six fault scenarios tested against a local remote |
| M-01 metadata outlives files | Fixed | 55ee180 | Install re-hashes destinations; needs-repair rows; delete is files-first |
| M-02 unvalidated catalog | Fixed | 55ee180 | `PackCatalogValidator` at the fetch boundary; `packs/`-pinned remote paths; Documents-descendant destinations |
| M-03 hidden installer errors | Fixed | 55ee180 | `lastOperationError` + index-failure rendering in the Packs UI; partial deletes reported |
| M-04 reader invariants | Fixed | 8b87912 | Section overlap/duplicate rejection at `open` (O(sections)); O(n) index validation in opt-in `verify_structure()` — kept off the cold-start path deliberately. `ch_middle_node` range checking noted as a possible future addition |
| M-05 Swift 6 contract | Fixed | d4e22f0 | Adopted: `SWIFT_VERSION 6.0` + complete concurrency; `@preconcurrency` delegate conformances with documented delivery guarantees |
| M-06 silent unusable captures | Fixed | d4e22f0 | Authorization gating, revocation/error handling, ingest contract (validity, monotonic t, ~1 Hz), surfaced errors |
| M-07 corridor-pinned search | Fixed | d4e22f0 | Completer bias follows the active region |
| M-08 CI gaps | Fixed | bf3f4bc | fmt + rustsec advisory steps; macOS job: xcframework from source, generated-binding freshness diff, xcodegen, arm64 build. PlannerKit's Swift tests stay local (need real pack data) |
| M-09 advisories | Accepted | — | `osmpbf` 0.3.8 is the newest release and hard-pins `memmap2 0.5` (no upgrade exists; offline pipeline only). `bincode 1.3.3` (unmaintained) is confined to the legacy `slice_import` dev bin. CI's advisory step fails on vulnerabilities and keeps these visible as warnings |
| M-10 missing license text | Fixed | 4f17947 | MIT text added |
| L-01 fmt drift | Fixed | 8b87912 | Formatted; CI now enforces |
| L-02 stale READMEs | Fixed | 4f17947, bf3f4bc | Root README written; app README matches the current app with a three-tier verification matrix |
| L-03 toolchain reproducibility | Fixed | 4f17947, bf3f4bc | `rust-toolchain.toml` pins 1.98.0; bootstrap installs xcodegen + rclone and follows the pin |
| L-04 untracked prototype tree | Fixed | 4f17947 | Narrow ignore rules for the artifact dirs only; sources live on the `prototype/*` branches |

Residual, consciously kept: the catalog remains the trust root behind HTTPS on a private tailnet host (revisit before public hosting, as §Security notes); a mid-download in-process continuation still does not survive relaunch — the manifest + orphan-resume path re-enters the install instead, skipping verified staged work; the publish lock serializes one Mac, per the single-publisher assumption it documents.
