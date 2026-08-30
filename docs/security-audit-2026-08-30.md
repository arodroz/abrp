# Wayfinder Security Audit

**Audit date:** 2026-08-30  
**Evidence snapshot:** `4f17947f49b1d20f06b9afee582e136b391f3d9c`, plus the four uncommitted Swift/concurrency changes present at `2026-08-30T00:12:15+02:00`  
**Audit type:** Repository-grounded static review, threat modeling, dependency review, and local build/test verification  
**Overall risk:** **High until pack releases are authenticated independently of the distribution host**

## Executive summary

Wayfinder has a comparatively small attack surface: it has no account system, application backend, WebView, inbound deep links, or custom URL handlers. The core is written in Rust, its one explicit `unsafe` operation is a read-only memory map, Apple transport security remains enabled, pack downloads use HTTPS, and pack artifacts are streamed through SHA-256 verification before installation. Recent hardening also added fail-closed pipeline behavior, catalog path validation, structural `.rpack` checks, journaled installation, and background-download recovery.

The principal security weakness is that these checks do not establish **authenticity**. The app accepts hashes from the same mutable bucket that serves the files, while the two style documents are not listed in the hashed catalog at all. Anyone who compromises the bucket, publisher credentials, catalog-generation path, or TLS termination point can replace the catalog and every artifact with a mutually consistent malicious release. Because those artifacts determine roads, charger availability, and map presentation, this is an integrity failure in the product's most important trust boundary.

The publishing path also contradicts its own immutability guarantee: `rclone copyto` overwrites an existing epoch object by default, and `packs/index.json` is updated through an unlocked read-modify-write. Finally, the installer performs a sequence of individually atomic replacements, not one atomic release switch; ignored recovery errors can still cause the new installed record to certify mixed or incomplete bytes.

### Finding totals

| Severity | Count | Meaning in this audit |
|---|---:|---|
| Critical | 0 | No issue demonstrated unauthenticated code execution, secret loss, or equivalent systemic compromise. |
| High | 3 | Compromise or inconsistency can materially corrupt the trusted navigation-data path. |
| Medium | 9 | Meaningful integrity, availability, privacy, release, or platform-compliance weakness. |
| Low | 3 | Defense-in-depth, maintenance, or privacy-footprint weakness. |

### Priority actions

1. Sign a canonical release manifest offline and verify it in the app with an embedded public key; include every pack and style asset and add rollback protection.
2. Make epoch publication actually immutable and serialize or conditionally update mutable catalog pointers.
3. Install each release into a versioned directory and atomically switch one small active-release pointer.
4. Add strict size/schema limits and run full `.rpack` semantic verification before a release becomes active.
5. Add an app-owned privacy manifest and make the Open-Meteo location disclosure and local location-data lifecycle explicit.

## Scope and assumptions

### In scope

- Rust workspace: pack parsing, routing, energy modeling, optimization, UniFFI, and the pack-building pipeline.
- Swift/iOS application: catalog fetch, background download, install/recovery, pack loading, location access, trip logs, weather lookup, search recents, MapLibre integration, and lifecycle code.
- Supply chain: Cargo and Swift packages, generated UniFFI binary delivery, bootstrap/build/publish scripts, and GitHub Actions.
- Data handling: precise locations, trip traces, recent destinations, downloaded map/routing data, and outbound network requests.
- Repository configuration and generated iOS build output available locally.

### Out of scope and limitations

- No credentials were supplied, so Garage/S3 policies, Tailscale ACLs, DNS, TLS termination, code-signing identities, and App Store Connect settings were not inspected.
- The live pack host was not penetration-tested, load-tested, or modified.
- This was not a binary reverse-engineering audit of MapLibre, the generated Rust XCFramework, or the untracked `prototype/` build caches.
- No physical-device attack, jailbreak, network interception, fuzzing campaign, or dynamic instrumentation was performed.
- Dependency review used the resolved manifests and RustSec; absence of a published advisory is not proof that a dependency is defect-free.
- The worktree changed concurrently. Findings cite the snapshot above. Four uncommitted Swift 6 migration edits appeared after the main test run and are called out where relevant.

## System and threat model

### Security-relevant assets

- Correct road graph, turn costs, contraction hierarchy, geometry, charger list, and offline map/style data.
- Availability of route planning and recovery from interrupted multi-gigabyte downloads.
- Precise origin, destination, route, recent destinations, and trip-log traces.
- Publisher credentials, bucket contents, mutable catalog pointers, native app binary provenance, and release history.

### Trust boundaries and data flow

```text
Geofabrik / Protomaps / charger feeds / elevation tiles / local tools
                              |
                              v
                  Rust pack-building pipeline
                              |
                              v
                  dist/<region>/catalog + packs
                              |
                     publish-packs.sh
                              |
                              v
               Garage/S3 + HTTPS/Tailscale endpoint
                    |                    |
       index/catalog JSON         styles + large packs
                    \                    /
                     v                  v
                Swift validator -> installer/staging/journal
                                      |
                                      v
                           iOS Documents directory
                               |              |
                     Rust mmap/parser     MapLibre

CoreLocation / MapKit -> app state -> trip log in Documents
                                \-> exact midpoint/time -> Open-Meteo
```

### Credible threat actors

- An attacker who obtains bucket, publisher-machine, CI/action, or upstream build-input control.
- A malicious or compromised upstream feed/tool that produces plausible but incorrect artifacts which the publisher then legitimately hashes.
- A local user or process able to replace/sideload app-container files, or a person with access to device backups/shared trip logs.
- A network attacker constrained by HTTPS and, for pack traffic, the documented private-tailnet deployment.
- A malicious pull request or compromised third-party GitHub Action running with workflow-token permissions.

## Existing security strengths

- Default ATS/HTTPS behavior is retained; no arbitrary-load exception was found.
- The pack host is centralized in one client and is documented as private-tailnet infrastructure ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L171)).
- Region IDs, SHA-256 strings, leaf filenames, remote relative paths, and local descendants are validated before use ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L90)).
- Large artifacts use streaming CryptoKit SHA-256 checks and background downloads rather than loading complete files into memory ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L283)).
- Recent work made pipeline feed failures and incomplete catalogs fail closed by default, with explicit tests.
- `.rpack` opening now checks magic/version, bounds, alignment, duplicate and overlapping sections, fixed-section lengths, and snap-grid offset shape ([`reader.rs`](../core/packs/src/reader.rs#L52)).
- Rust prevents broad memory-unsafety classes; the reviewed runtime parser has one narrow, read-only mmap `unsafe` block ([`reader.rs`](../core/packs/src/reader.rs#L53)).
- Shell-outs use structured `Command` arguments rather than string-built shell commands ([`map_pack.rs`](../pipeline/src/map_pack.rs#L20), [`osm_import.rs`](../pipeline/src/osm_import.rs#L509)).
- Cargo dependencies are locked, MapLibre is pinned to exact version `6.29.0`, and the Rust toolchain is now pinned.
- Trip logs require explicit user initiation and explicit share/delete actions. Weather uses an ephemeral URL session.
- No application secrets, private keys, tokens, passwords, or credentials were found by a high-confidence repository pattern scan.
- The default Release iOS device build and all Rust quality gates passed at the tested snapshot; detailed results appear below.

## Detailed findings

## High severity

### SEC-001 — Pack releases are hashed but not authenticated

**Impact:** Compromise of the pack origin or publishing path can silently alter roads, travel costs, charger availability, geometry, and map presentation while still passing every client-side integrity check.

**Evidence**

- `PackArtifact.sha256` is decoded from the remotely fetched catalog, and the catalog itself carries no signature ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L24), [`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L171)).
- The installer compares downloaded bytes with that remote value ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L283)). A compromised origin can therefore replace both value and bytes.
- `style-light.json` and `style-dark.json` are fetched from an inferred path, are absent from `PackCatalog`, and are only hashed locally after download for journal recovery ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L374)).
- The publisher uploads unsigned JSON pointers and artifacts directly to the same remote ([`publish-packs.sh`](../scripts/publish-packs.sh#L24)).
- No trusted-release key, signature, transparency log, or last-seen epoch enforcement was found.

**Attack scenario:** An attacker obtains Garage credentials, compromises the publish workstation, controls the reverse proxy/TLS endpoint, or modifies the catalog-generation output. They publish a modified road graph and charger set with matching hashes. The app presents and routes over the malicious data without a trust failure.

**Recommendation**

- Produce one canonical manifest per region/epoch containing region ID, epoch, schema/format versions, exact filenames, lengths, SHA-256 hashes, style assets, source-provenance identifiers, and build identity.
- Sign the canonical bytes with an offline or isolated release key. Embed only the public verification key in the app.
- Verify the signature before making any artifact request; reject unknown algorithms/keys and malformed or duplicate fields.
- Persist the highest accepted epoch per region and require an explicit user/admin recovery path for rollback.
- Support key rotation with a small, versioned trust-root format; do not fetch the trust root from the same bucket.

**Verification:** Tests must demonstrate rejection of a modified manifest, modified artifact, modified style, wrong key, wrong region, duplicate field, noncanonical encoding, and older signed epoch. A valid signed release must install offline from fixtures.

Reference: [OWASP MASWE-0011: Missing Cryptographic Key Protection](https://mas.owasp.org/MASWE/MASVS-CRYPTO/MASWE-0011/).

### SEC-002 — The publisher can overwrite “immutable” epochs and lose concurrent index updates

**Impact:** A rerun, compromised workstation, or concurrent publish can replace versioned objects or silently remove another region from the global index, defeating resumable-download consistency and release provenance.

**Evidence**

- The script states that epoch paths never change, but uses plain `rclone copyto` for every epoch object ([`publish-packs.sh`](../scripts/publish-packs.sh#L24)). `copyto` overwrites an existing destination by default.
- It does not use `--immutable`, a conditional request, a remote-object preflight, or post-upload verification.
- `packs/index.json` is read, transformed, and overwritten without an ETag/generation condition or lock ([`publish-packs.sh`](../scripts/publish-packs.sh#L40)). Two publishers can both read version N and each write a different N+1; the last writer wins.
- Any missing, unreadable, or invalid index is replaced with an empty index, so a transient authorization/network/corruption problem can erase existing entries ([`publish-packs.sh`](../scripts/publish-packs.sh#L44)).

**Recommendation**

- Upload epoch objects with immutable semantics (`--immutable --checksum` at minimum) and fail if an existing object differs.
- Stage an entire release under a new prefix, verify remote sizes/hashes, then publish its signed manifest.
- Treat index read/parse failure as fatal. Serialize pointer publication, or use object-version/ETag conditional writes in a small publisher service.
- Retain object versioning and an append-only release log where supported.

**Verification:** Two simultaneous publishes for different regions must leave both entries. Re-publishing identical bytes may be idempotent; re-publishing different bytes to the same epoch must fail. Injected index-read failure must leave the remote index unchanged.

Reference: [`rclone copyto` documentation](https://rclone.org/commands/rclone_copyto/).

### SEC-003 — Multi-file pack activation is not atomic

**Impact:** A crash or filesystem error can leave artifacts/styles from different releases active together, while recovery may write a new installed record that certifies an incomplete release.

**Evidence**

- The implementation stages all content, but commits through a loop of per-file replacements ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L335)). Each file is atomic; the set is not.
- Recovery ignores `applyReplace` errors with `try?` and later ignores `saveRecord` errors ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L474)). If a staged file still exists but replacement fails, execution continues and can save the new record.
- When recovery becomes impossible after some replacements, it deletes the record rather than restoring prior bytes. That is safer than a lying old record, but it confirms that rollback is unavailable.
- Styles are shared global filenames and “whichever region installs last” wins ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L374)). A region release is therefore not self-contained.

**Recommendation:** Store every release in `Application Support/Packs/<region>/<epoch>/`, including styles. After complete signature, hash, schema, and semantic verification, atomically replace one small `active-<region>.json` pointer (or symlink-equivalent supported by the design). Keep the previous version until activation succeeds; garbage-collect later. Recovery should validate the target directory and either switch the pointer once or leave the prior pointer untouched.

**Verification:** Add failure injection before/after every write, fsync, rename, and record update. After each simulated termination, readers must see either the complete old release or complete new release, never a mixture.

## Medium severity

### SEC-004 — Catalog validation does not enforce the complete release contract

**Impact:** A malformed or compromised catalog can pass validation while aliasing files, clobbering another region's root-level artifact, consuming unexpected formats, or triggering inconsistent install/delete behavior.

**Evidence**

- The Swift models omit `catalog_format`, artifact `format`, `protomaps_build`, and build timestamp fields generated by the pipeline ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L12), [`catalog.rs`](../pipeline/src/catalog.rs#L90)).
- Validation allows zero-byte artifacts and does not require a positive epoch, unique region IDs, unique artifact paths/files, or the exact filename expected for each kind ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L90)).
- A leaf filename is constrained to Documents but is not namespaced or bound to its region/kind. A catalog can therefore designate another installed region's leaf file and later overwrite/delete it.
- The current remote-path validator is materially better than a prefix-only check: it pins `packs/` and rejects dot components. That protection should be retained.

**Recommendation:** Decode and strictly validate the full signed schema. Require known catalog/pack formats, positive bounded epoch/lengths, exact kind-to-filename/path derivation, exactly one artifact per kind, uniqueness across index and catalog, and consistent region/epoch values. Prefer deriving paths locally from validated identifiers instead of trusting redundant remote strings.

**Verification:** Property/fuzz tests should cover duplicate keys, duplicate artifacts, wrong kind suffix, cross-region filenames, zero/negative/overflow lengths, old/new format versions, Unicode separators, percent encoding, and every `.`/`..` representation.

### SEC-005 — Network and parser inputs lack resource limits

**Impact:** A compromised origin or corrupted local file can cause disk exhaustion, excessive allocation, prolonged CPU use, or an app crash.

**Evidence**

- Catalog/index/style fetches use `URLSession.data(from:)` with no maximum response length; `PackArtifact.bytes` is displayed/validated but not enforced during download ([`PackCatalog.swift`](../app/Wayfinder/Sources/PackCatalog.swift#L188), [`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L374)).
- Background artifact downloads do not preflight free space or cancel when received bytes exceed the declared length ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L567)).
- Charger JSON is loaded into a complete `Data`/byte vector and deserialized into a complete vector ([`planner.rs`](../core/ffi/src/planner.rs#L60), [`corridor.rs`](../core/optimiser/src/corridor.rs#L126)).
- The charger schema does not impose an explicit maximum station count, string length, power, or coordinate range at the runtime boundary.

**Recommendation:** Define release-level and per-kind byte ceilings, require `Content-Length` when feasible, enforce declared and received bytes during streaming, reserve disk headroom, cap small JSON responses, and add charger count/string/numeric bounds. Treat violations as a failed release and clean staged data.

**Verification:** Test oversized headers, chunked bodies exceeding the limit, decompression/JSON amplification, huge arrays/strings, `NaN`/infinity/out-of-range coordinates, low disk, and cancellation cleanup.

### SEC-006 — Deep `.rpack` validation exists but is not used on the app path

**Impact:** A malformed pack that bypasses or predates installer hashing can trigger routing panics or incorrect graph traversal; Release Rust uses `panic = "abort"`, turning a panic into process termination.

**Evidence**

- `Rpack::open` now provides strong cheap layout validation, while checksum and O(nodes+edges) semantic validation are separate opt-in methods ([`reader.rs`](../core/packs/src/reader.rs#L331)).
- The app's `Planner::new` calls only `Rpack::open`; it does not call `verify_checksums` or `verify_structure` ([`planner.rs`](../core/ffi/src/planner.rs#L36)).
- Tests prove the deep validator detects non-monotone CSR indexes, out-of-range nodes/edges/order, and invalid geometry, but those tests do not make the production call path invoke it ([`rpack_roundtrip.rs`](../pipeline/tests/rpack_roundtrip.rs#L478)).
- Release builds abort on panic ([`Cargo.toml`](../Cargo.toml#L25)). Rust bounds checks protect memory safety, but availability and result integrity remain exposed.

**Recommendation:** Run full checksum and semantic verification once in the trusted activation path, cache the result keyed by signed artifact digest, and refuse to activate invalid packs. Keep `open` cheap for later starts. Extend verification to every router/snap invariant and fuzz `open`, `verify_structure`, `snap`, and route construction.

**Verification:** Mutate every index/range field in a valid fixture and assert clean rejection without panic, excessive allocation, or long runtime. Add `cargo-fuzz`/libFuzzer corpora and a maximum execution budget.

### SEC-007 — Upstream data and build tools are not reproducibly authenticated

**Impact:** A compromised upstream feed, rolling tool release, or developer bootstrap endpoint can produce poisoned output that is subsequently hashed and treated as a legitimate Wayfinder release.

**Evidence**

- Bootstrap executes the rustup network installer and updates a toolchain, installs unpinned Homebrew tools, and downloads Geofabrik PBFs without a pinned checksum ([`bootstrap-mac.sh`](../scripts/bootstrap-mac.sh#L16)).
- The map builder asks an external `pmtiles` binary to extract from a Protomaps URL without recording/verifying the source object's digest ([`map_pack.rs`](../pipeline/src/map_pack.rs#L14)).
- The new pipeline behavior fails closed by default, which is a strong improvement, but `--allow-partial` can deliberately omit failed inputs. That degraded provenance is not encoded in the catalog and the publisher does not reject it.

**Recommendation:** Maintain a source manifest with provider URL, immutable version, expected digest, retrieval time, license, and tool/container digest. Pin build tools and build in a controlled environment. Mark partial/degraded outputs in the signed manifest and make production publication reject them by policy.

**Verification:** Rebuild from the same source manifest in a clean environment and compare canonical outputs or documented nondeterministic fields. Wrong source/tool digest and partial input must prevent a production publish.

### SEC-008 — The native Rust library release path is outside CI provenance

**Impact:** An iOS release can link a stale, locally built, or tampered `planner_ffi.xcframework` that was not produced from the reviewed source revision.

**Evidence**

- PlannerKit references a local binary target ([`Package.swift`](../app/PlannerKit/Package.swift#L13)); the artifact is generated locally and excluded from normal source review.
- Generated Swift bindings and the binary are separate products of [`build-xcframework.sh`](../scripts/build-xcframework.sh), creating drift risk.
- CI runs only Rust tests and Clippy; it does not rebuild/compare the XCFramework, regenerate bindings, build/archive the iOS app, produce an SBOM/attestation, or preserve release hashes ([`ci.yml`](../.github/workflows/ci.yml#L8)).

**Recommendation:** Create a trusted release job that checks out a pinned revision, uses pinned Rust/Xcode/UniFFI inputs, rebuilds both architectures and bindings, runs tests, records the XCFramework digest, creates an SBOM/provenance attestation, and feeds exactly that artifact into the signed app release.

**Verification:** A clean release must reproduce the recorded library/binding hashes or explain controlled nondeterminism. CI must fail if checked/generated bindings differ or if an unrecorded binary is substituted.

### SEC-009 — Trip logging discloses a precise location/time sample to Open-Meteo

**Impact:** Starting and stopping a local trip log automatically sends a precise midpoint latitude, longitude, and hour to a third party, exposing a derived trip location without an in-product disclosure or opt-out.

**Evidence**

- On stop, TripLogStore automatically requests the temperature for the midpoint sample/time ([`TripLogStore.swift`](../app/Wayfinder/Sources/TripLogStore.swift#L87), [`TripLogStore.swift`](../app/Wayfinder/Sources/TripLogStore.swift#L158)).
- The client sends unrounded `latitude`, `longitude`, `start_date`, and `end_date` query values to `api.open-meteo.com` ([`OpenMeteo.swift`](../app/Wayfinder/Sources/OpenMeteo.swift#L25)).
- An ephemeral session limits local HTTP cache/cookie persistence, which is positive, but does not change what the remote service receives.
- The location usage string describes maps, routes, charging stops, and GPS trace recording, but not third-party weather disclosure ([`project.yml`](../app/Wayfinder/project.yml#L22)).

**Recommendation:** Round/coarsen the coordinate and time to the minimum precision required, explain the provider and purpose before enabling the lookup, provide an opt-out, and document provider retention. Ensure App Store privacy answers match the actual provider arrangement; Apple's “collected” definition depends in part on whether data is retained beyond servicing the request.

**Verification:** Inspect an intercepted request and confirm bounded precision, no stable identifier/cookies, no request when disabled, and deletion/cache behavior. Review the final privacy label and policy against the actual service contract.

Reference: [Apple App Privacy Details](https://developer.apple.com/app-store/app-privacy-details/).

### SEC-010 — Precise local data has no explicit backup, protection, or retention policy

**Impact:** Device backups or prolonged local retention can expose trip traces and destination history beyond the user's expectations; multi-gigabyte public packs may also unnecessarily inflate backups.

**Evidence**

- Trip logs containing the GPS trace are stored under `Documents/trip-logs` with atomic writes but no explicit file-protection class or backup exclusion ([`TripLog.swift`](../app/Wayfinder/Sources/TripLog.swift#L49)).
- Packs, staging data, journals, and records also use Documents ([`PackInstaller.swift`](../app/Wayfinder/Sources/PackInstaller.swift#L589)). Apple treats Documents as user data that is normally backed up.
- Up to five exact recent destination coordinates persist indefinitely in `UserDefaults`, with no clear-all/retention control ([`RouteEditorView.swift`](../app/Wayfinder/Sources/RouteEditorView.swift#L21), [`RouteEditorView.swift`](../app/Wayfinder/Sources/RouteEditorView.swift#L178)).
- iOS supplies default Data Protection, so this is not a claim that files are plaintext on a locked device. The issue is that the app does not select or verify the stricter lifecycle appropriate to each class of data.

**Recommendation:** Move reproducible packs to Application Support/Caches as appropriate and exclude them from backup. Define whether trip logs should sync; apply `completeFileProtection` or a documented alternative, and set backup behavior intentionally. Add “delete all trip logs” and “clear recent destinations,” plus a documented retention default.

**Verification:** Inspect file resource values/protection attributes on a locked device, archive/restore behavior, and UI deletion. Confirm no obsolete staging/log files remain after cancellation or deletion.

References: [Apple: Using the File System Effectively](https://developer.apple.com/documentation/foundation/using-the-file-system-effectively), [Apple: Encrypting Your App's Files](https://developer.apple.com/documentation/uikit/encrypting-your-app-s-files).

### SEC-011 — The application has no app-owned privacy manifest

**Impact:** The app may fail App Store privacy validation and lacks an auditable declaration for required-reason APIs and collected-data behavior.

**Evidence**

- No `PrivacyInfo.xcprivacy` exists in the Wayfinder source or the built app root.
- The app directly uses `UserDefaults` in multiple places, while Apple's required-reason API guidance includes UserDefaults declarations.
- The embedded MapLibre framework contains its own manifest, but a dependency manifest does not replace the app's declaration for the app's own calls and data practices.

**Recommendation:** Add an app-target `PrivacyInfo.xcprivacy` through XcodeGen, declare only the reason codes actually applicable to the app's own API use, and accurately declare collected data/tracking based on retention and third-party processing. Archive the app and review Xcode's aggregate privacy report before release.

**Verification:** Confirm the manifest is present at the app bundle root, validate it with the current Xcode archive workflow, and reconcile it with source behavior and App Store Connect answers.

References: [Apple: Describing use of required-reason APIs](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api), [Apple TN3183](https://developer.apple.com/documentation/technotes/tn3183-adding-required-reason-api-entries-to-your-privacy-manifest).

### SEC-012 — GitHub Actions lacks least-privilege and supply-chain controls

**Impact:** A compromised mutable action tag or malicious workflow change has avoidable opportunity to influence CI, while vulnerable dependencies and iOS/native drift are not automatically detected.

**Evidence**

- Workflow actions use mutable major/stable references rather than full commit SHAs ([`ci.yml`](../.github/workflows/ci.yml#L12)).
- The workflow does not declare top-level or job-level `permissions`; explicit `contents: read` is sufficient for the present checks.
- There is no RustSec/cargo-deny, CodeQL/static analysis, secret scanning configuration, fuzz smoke test, iOS build, generated-binding comparison, or provenance job.
- Positive control: no workflow secrets are used, jobs run on GitHub-hosted runners, and current Rust tests/Clippy use `--locked`.

**Recommendation:** Pin third-party actions by full commit SHA, declare `permissions: contents: read`, add dependency/advisory policy and native/iOS build checks, enable repository secret scanning/dependency review where available, and separate privileged release workflows from pull-request code.

**Verification:** Inspect effective workflow-token permissions, test Dependabot/dependency-review behavior with a fixture advisory, and require review for workflow-file changes.

Reference: [GitHub: Security hardening for GitHub Actions](https://docs.github.com/en/actions/reference/security/secure-use).

## Low severity

### SEC-013 — The lockfile contains two RustSec warnings

**Impact:** These are not currently demonstrated exploitable vulnerabilities in the shipped app path, but they represent maintenance and potential unsoundness debt.

**Evidence**

- `cargo audit` scanned 186 dependencies and reported `bincode 1.3.3` as unmaintained ([RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141)). It is used by the build pipeline's slice importer, not the iOS runtime ([`slice_import.rs`](../pipeline/src/slice_import.rs#L64)).
- It also reported `memmap2 0.5.10` as affected by an unchecked pointer-offset unsoundness warning ([RUSTSEC-2026-0186](https://rustsec.org/advisories/RUSTSEC-2026-0186)). This older version is transitive through `osmpbf 0.3.8`; the app's pack reader uses patched `memmap2 0.9.11`.
- No call to the specifically affected older functions was confirmed in Wayfinder code, so this is rated Low rather than treated as demonstrated memory corruption.

**Recommendation:** Update/replace `osmpbf`, plan migration away from unmaintained bincode 1.x for untrusted inputs, and run `cargo audit` or `cargo deny` in CI with an explicit, expiring allowlist for accepted warnings.

**Verification:** The advisory gate should be clean or contain a documented owner, reachability rationale, and expiry for each exception.

### SEC-014 — Swift 6 concurrency hardening is not yet reproducibly enabled

**Impact:** Compiler-enforced actor isolation is not a reliable release gate for background URLSession, CoreLocation, MapKit, and MapLibre delegate interactions.

**Evidence**

- The committed generated Xcode project still declares Swift 5, while an uncommitted `project.yml` change selects Swift 6 with complete strict concurrency. Regenerating the project is therefore required before the intended setting affects normal builds.
- A forced Swift 6/complete-concurrency Release build failed during this audit on delegate/actor-isolation crossings. Uncommitted `@preconcurrency` remediations then appeared in PackInstaller, PlanStore, and TripLogStore, but the worktree was changing and the full gate was not revalidated.
- `@preconcurrency` suppresses compile-time checking at a boundary; its safety rests on the documented guarantee that each delegate callback actually arrives on the main executor, which should be asserted/tested rather than assumed indefinitely.

**Recommendation:** Regenerate and commit the project, make the Swift 6 Release build a CI gate, use explicit `nonisolated` delegate bridges that hop to `MainActor` where callback threading is not guaranteed, and add Thread Sanitizer/runtime assertions in an appropriate test configuration.

**Verification:** A clean checkout must generate the same project and pass a Swift 6 complete-concurrency device build without warnings/errors. Exercise background relaunch, location updates, search completion, and MapLibre callbacks.

### SEC-015 — “Offline” map styles still contact GitHub Pages

**Impact:** Map display can disclose device IP/use timing to an additional provider and fail when online font/sprite assets are unavailable.

**Evidence:** Both bundled style templates point `glyphs` and `sprite` at `https://protomaps.github.io/basemaps-assets/...` ([`style-light.json`](../pipeline/assets/styles/style-light.json#L1), [`style-dark.json`](../pipeline/assets/styles/style-dark.json#L1)). The PMTiles source may be local, but labels/icons are not fully offline.

**Recommendation:** Bundle required glyph/sprite subsets or publish them as authenticated release assets on the controlled origin, include their hashes in the signed manifest, and disclose any deliberately retained external service.

**Verification:** With all networking disabled after install, the map should render its supported labels/icons without attempted external requests.

## Verification results

| Check | Result | Notes |
|---|---|---|
| `cargo test --workspace --all-features --locked` | Pass | 141 tests passed; 5 data-dependent golden/performance tests were ignored by design. |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass | No Clippy warnings at the tested Rust snapshot. |
| `cargo fmt --all -- --check` | Pass | Rust formatting clean. |
| `bash -n scripts/*.sh` | Pass | Shell syntax clean. |
| `git diff --check` | Pass | No whitespace errors at the tested snapshot. |
| Release iOS device build, signing disabled | Pass | `xcodebuild ... -configuration Release -destination generic/platform=iOS CODE_SIGNING_ALLOWED=NO build`. |
| Forced Swift 6 + complete concurrency build | Fail / remediation in progress | Failed on concurrency isolation before the later uncommitted delegate changes; generated Xcode project remains Swift 5 until regenerated. |
| `cargo audit` | Pass with 2 warnings | No vulnerability-level advisory; warnings for `bincode 1.3.3` and transitive `memmap2 0.5.10`. |
| High-confidence secret scan | Pass | No committed application secret/key/token pattern found. This does not replace provider-side secret inventory. |
| Privacy manifest inspection | Fail | MapLibre has a dependency manifest; Wayfinder has no app-owned manifest in source/built app root. |

## Recommended remediation roadmap

### P0 — Before distributing trusted navigation packs

- Resolve SEC-001 with signed, rollback-protected, complete release manifests.
- Resolve SEC-002 with immutable epoch uploads and concurrency-safe pointer publication.
- Resolve SEC-003 with versioned release directories and one atomic activation pointer.
- Resolve SEC-004 and SEC-005 so a signed manifest still cannot encode unsafe names, formats, or resource sizes.
- Run SEC-006 semantic verification before activation.

### P1 — Before an external/App Store release

- Make native artifacts reproducible and CI-attested (SEC-008).
- Document/coarsen/control the Open-Meteo disclosure (SEC-009).
- Set explicit storage, backup, protection, and deletion behavior (SEC-010).
- Add and validate the app privacy manifest (SEC-011).
- Pin Actions and apply least privilege/advisory gates (SEC-012).
- Finish and gate the Swift 6 migration (SEC-014).

### P2 — Ongoing hardening

- Authenticate upstream inputs and pin build tools (SEC-007).
- Resolve or formally time-bound RustSec warnings (SEC-013).
- Bundle/self-host authenticated map glyphs and sprites (SEC-015).
- Add fuzzing for binary/JSON parsers, release failure-injection tests, and periodic restore/privacy tests.

## Suggested security acceptance criteria

A release should not be considered security-ready until all of the following are true:

- A client with only the embedded public key can reject any changed release byte, including styles, without trusting the bucket's catalog.
- The same epoch cannot be replaced with different bytes, and concurrent regional publishes cannot lose one another.
- Killing the app or forcing an I/O failure at any activation step always leaves one complete release active.
- Oversized or semantically invalid catalogs, packs, styles, and charger data fail within documented memory/disk/time budgets.
- A clean CI checkout produces and attests the exact native binary and generated bindings shipped in the app.
- Users and App Store disclosures accurately describe location storage and the Open-Meteo request; local data has testable deletion/backup/protection behavior.
- Rust, Swift 6, iOS Release, advisory, and privacy-manifest gates pass from a clean checkout.

## Authoritative references

- [OWASP MASVS — Code Quality](https://mas.owasp.org/MASVS/10-MASVS-CODE/)
- [OWASP MASWE-0011 — Missing Cryptographic Key Protection](https://mas.owasp.org/MASWE/MASVS-CRYPTO/MASWE-0011/)
- [Apple — App Privacy Details](https://developer.apple.com/app-store/app-privacy-details/)
- [Apple — Describing use of required-reason APIs](https://developer.apple.com/documentation/bundleresources/describing-use-of-required-reason-api)
- [Apple — TN3183: Adding required-reason API entries](https://developer.apple.com/documentation/technotes/tn3183-adding-required-reason-api-entries-to-your-privacy-manifest)
- [Apple — Using the File System Effectively](https://developer.apple.com/documentation/foundation/using-the-file-system-effectively)
- [Apple — Encrypting Your App's Files](https://developer.apple.com/documentation/uikit/encrypting-your-app-s-files)
- [GitHub — Security hardening for GitHub Actions](https://docs.github.com/en/actions/reference/security/secure-use)
- [`rclone copyto`](https://rclone.org/commands/rclone_copyto/)
- [RustSec RUSTSEC-2025-0141](https://rustsec.org/advisories/RUSTSEC-2025-0141)
- [RustSec RUSTSEC-2026-0186](https://rustsec.org/advisories/RUSTSEC-2026-0186)
- [MapLibre Native advisories](https://github.com/maplibre/maplibre-native/security/advisories)
- [MapLibre Native security policy](https://github.com/maplibre/maplibre-native/security/policy)

## Conclusion

Wayfinder's code is not broadly exposed like a network service, and its recent parser, pipeline, and installer hardening is substantive. The remaining top risk is architectural: the system treats a mutable distribution origin as both the source of content and the source of truth about that content. Signing a complete manifest, making publishing immutable, and activating a fully verified version with one atomic switch would turn the current collection of good checks into an end-to-end security property. Until those three changes are complete, a compromised or inconsistent release path can still become trusted navigation state.
