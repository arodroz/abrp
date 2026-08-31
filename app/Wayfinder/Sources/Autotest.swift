// Launch-argument autotest (wayfinder #39): `--autotest plan-golden` runs the
// same LU -> Amsterdam golden request as
// PlannerKitTests/GoldenPlanTests.swift, off the main thread, against the
// pack sideloaded into Documents, and exits with the pass/fail result. This
// proves a plan() round-trip through the real xcframework on the simulator,
// independent of the (placeholder) UI. `--autotest editor-smoke` (wayfinder
// #40) extends this to drive the route editor's store mutations directly --
// there's no UI-event injection on the sim, so the editor UI itself is
// verified separately, by screenshot. `--autotest card-smoke` (wayfinder #43)
// does the same for the result card + SoC chart + scrub marker.
// `--autotest settings-smoke` (wayfinder #44) drives the settings sheet's
// store-bound fields the same way: each field's own didSet IS the settings
// path, so setting e.g. `store.departSoc` directly exercises exactly what
// the sheet's Slider would trigger. `--autotest perf` (wayfinder #45) measures
// the ADR 0001 M2 gate numbers -- cold-start/plan latency/memory -- through
// PlannerClient directly, like plan-golden. `--autotest install-smoke`
// (wayfinder #47) exercises the installer's real code path against the live
// hosted catalog: fetches the index, installs the small lu-dev region (a
// real ~49MB download), opens a PlannerClient on the installed rpack and
// parses its Charger Pack, then deletes the region and checks cleanup.
// After that live-network part, it runs the codebase-audit remediation
// checks (docs/codebase-audit-2026-08-29.md H-01/H-03/M-01/M-02) offline,
// with synthetic files: journal roll-forward, an unrecoverable journal,
// catalog validator rejections (including the audit's own path-traversal
// proof), and the M-01 needs-repair row flag. Keeping the live part first
// means an offline failure in the new checks is never confused with a
// network-dependent one. It also proves two security quick-wins (issue #56)
// against the live-installed pack: `verifyRegionPack` (SEC-006) passes on
// the real .rpack and rejects a junk-bytes file, and every committed
// artifact is excluded from device backup (SEC-010).
// `--autotest triplog-smoke` (wayfinder #51) drives TripLogStore's capture
// lifecycle directly -- there's no location-fix injection on the sim, so
// synthetic CLLocations are fed straight to `ingest` -- and verifies the
// saved tlog-1 JSON against the schema #52's Rust `calibrate()` will parse.
// It also proves the SEC-010 (issue #56) data-deletion controls:
// `tripStore.deleteAllLogs()` over synthetic logs, and
// `RecentDestination.clearAll()` over a seeded UserDefaults entry.
// `--autotest calibrate-smoke` (wayfinder #53) drives PlanStore's
// refit-and-accept flow directly: synthesizes a qualifying ~130km trip and a
// non-qualifying ~3km trip straight via TripLogStorage.save (capture itself
// is triplog-smoke's job), then proves refreshCalibration's empty-set,
// result, accept (reference override set + persisted + replanned,
// calibrationDismissed set), dismiss-sticky, and dismiss-resets-on-log-change
// paths, plus that a corrupt log file fails the whole call loudly
// (PlannerError.InvalidRequest) instead of being silently skipped.
// `--autotest drive-smoke` (wayfinder #59, Drive Mode core) drives DriveStore
// directly -- there's no CoreLocation fix injection on the sim, so a fresh
// CLLocationManager is passed to PlanStore's own delegate method to adopt an
// origin fix, and synthetic CLLocations are fed straight to
// DriveStore.ingest. Proves: the Go gate both ways (no plan -> false, ready
// plan with a current-location origin -> true, overridden origin after End
// -> false again), entering drive hides the raw location dot and starts
// following, on-route snap + progress on exact polyline vertices, the snap
// pulling in a small (3-10 m) off-route offset while a 300 m offset stays
// raw, capped course smoothing, and the four camera-mode transitions
// (free-look on gesture, recenter, overview, overview back to following). Its
// next steps (wayfinder #61, ADR 0012 point 6) prove off-route detection +
// silent replan: a single noisy excursion doesn't fire one, a sustained (>=5
// s) 50+ m deviation does, the landed plan departs from the deviated
// position at the model's own predicted SoC (not the settings slider), and
// the drive survives the swap with the snap engine re-snapping against the
// new geometry on the very next fix. Its final steps (wayfinder #62, ADR
// 0012 point 7) prove the Trip Log coupling: Go opens the start-SoC prompt
// instead of entering directly, cancelling that prompt cancels the Go with
// no capture left behind, confirming it both enters the drive AND starts
// capture, End/arrival stops capture into the end-SoC prompt, and a
// STANDALONE capture already running (record button) is adopted outright by
// Go with no prompt -- the drive-closed log this produces is byte-for-byte
// the same shape triplog-smoke's button-started one is, since it's the same
// producer. Its last step (wayfinder #63) proves manual mid-drive dash-SoC
// correction: `correctSoc` replans from the snapped position at the entered
// SoC (not the model's curve estimate), the HUD/curve re-anchor to it, and
// it's a no-op once the drive is over. Its newest steps (wayfinder #67) prove
// the maneuver banner: the initial upcoming step and its distance right after
// entry, the countdown decreasing across two fixes approaching an anchor,
// advancing to the next step once passed, the literal EN template table
// pinned against a real corridor-pack landmark (A6 on-ramp signage), the
// "then" preview on a closely-chained pair, muting during a sustained
// off-route excursion, clearing on arrival, and correctly re-deriving after
// an off-route replan swap -- all degrading to "banner stays nil" on a v1
// pack, since `steps` is empty on every leg there.
import CoreLocation
import CryptoKit
import Darwin
import Foundation
import MapLibre
import PlannerKit

enum Autotest {
    static func runIfRequested(store: PlanStore, installer: PackInstaller, tripStore: TripLogStore, driveStore: DriveStore) {
        let args = ProcessInfo.processInfo.arguments
        guard let flagIndex = args.firstIndex(of: "--autotest"),
              flagIndex + 1 < args.count
        else { return }

        switch args[flagIndex + 1] {
        case "plan-golden":
            Task.detached(priority: .userInitiated) {
                await runPlanGolden()
            }
        case "map-smoke":
            Task.detached(priority: .userInitiated) {
                await runMapSmoke(store: store)
            }
        case "editor-smoke":
            Task.detached(priority: .userInitiated) {
                await runEditorSmoke(store: store)
            }
        case "card-smoke":
            Task.detached(priority: .userInitiated) {
                await runCardSmoke(store: store)
            }
        case "settings-smoke":
            Task.detached(priority: .userInitiated) {
                await runSettingsSmoke(store: store)
            }
        case "perf":
            Task.detached(priority: .userInitiated) {
                await runPerf()
            }
        case "install-smoke":
            Task.detached(priority: .userInitiated) {
                await runInstallSmoke(installer: installer)
            }
        case "triplog-smoke":
            Task.detached(priority: .userInitiated) {
                await runTriplogSmoke(tripStore: tripStore)
            }
        case "calibrate-smoke":
            Task.detached(priority: .userInitiated) {
                await runCalibrateSmoke(store: store, tripStore: tripStore)
            }
        case "drive-smoke":
            Task.detached(priority: .userInitiated) {
                await runDriveSmoke(store: store, tripStore: tripStore, driveStore: driveStore)
            }
        default:
            break
        }
    }

    // Copied from PlannerKitTests/GoldenPlanTests.swift's goldenRequest().
    private static func goldenRequest() -> FfiPlanRequest {
        FfiPlanRequest(
            originLat: 49.6116, originLon: 6.1319,
            destLat: 52.3702, destLon: 4.8952,
            waypoints: [],
            departSoc: 0.90,
            arrivalMinSoc: 0.10,
            chargerArrivalMinSoc: 0.10,
            chargerMaxSoc: 0.80,
            stopsBias: 1.0,
            tempC: 20.0,
            headwindMs: 0.0,
            batteryWarmth: 1.0,
            offerStopFreeAlternative: false,
            vehicle: .ioniq5Lr2wd,
            referenceConsumptionWhPerKm: nil
        )
    }

    private static func report(_ name: String, _ ok: Bool, _ detail: String = "") {
        let status = ok ? "PASS" : "FAIL"
        let suffix = detail.isEmpty ? "" : "(\(detail))"
        print("WAYFINDER-AUTOTEST: \(name) \(status)\(suffix)")
    }

    private static func isClose(_ actual: Double, _ expected: Double, tol: Double) -> Bool {
        abs(actual - expected) <= tol
    }

    /// `pct` as a fraction (0.05 == 5%).
    private static func isClosePct(_ actual: Double, _ expected: Double, pct: Double) -> Bool {
        abs(actual - expected) <= abs(expected) * pct
    }

    /// `sleepSeconds` is the screenshot window for editor-smoke: the external screenshot of
    /// the post-DONE UI state is taken while this runs.
    private static func finish(ok: Bool, sleepSeconds: Double = 1.0) async -> Never {
        print("WAYFINDER-AUTOTEST: DONE ok=\(ok)")
        try? await Task.sleep(nanoseconds: UInt64(sleepSeconds * 1_000_000_000))
        exit(ok ? 0 : 1)
    }

    /// The two direct-PlannerClient modes follow the active region (the store-driven modes
    /// already do, via store.load()): the golden LU -> Amsterdam pins hold on any pack
    /// covering the corridor -- proven bit-identical on eu-west at the pack-run QA.
    private static var autotestRegion: String {
        UserDefaults.standard.string(forKey: "activeRegion") ?? "corridor"
    }

    private static func runPlanGolden() async {
        guard let located = Packs.locate(region: autotestRegion) else {
            report("pack-present", false, "Documents/\(autotestRegion).rpack or -chargers.json missing")
            await finish(ok: false)
        }
        report("pack-present", true)

        var ok = true
        do {
            let client = try PlannerClient(regionPackPath: located.rpackURL.path)
            let chargerBytes = try Data(contentsOf: located.chargersURL)
            try client.loadChargers(bytes: chargerBytes, format: "cpack-1")
            report("chargers-loaded", true)

            let start = DispatchTime.now()
            let plan = try await client.plan(goldenRequest())
            let elapsedMs = Double(DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds) / 1_000_000
            print("WAYFINDER-AUTOTEST: cold plan ms=\(String(format: "%.1f", elapsedMs))")

            let stopCountOK = plan.stops.count == 1
            report("stop-count", stopCountOK, "expected 1, got \(plan.stops.count)")
            ok = ok && stopCountOK

            let expectedName = "Hyperfast charge laadpalen Nossegem Zaventem"
            let stopName = plan.stops.first?.name ?? "<none>"
            let nameOK = stopName == expectedName
            report("stop-name", nameOK, "expected \"\(expectedName)\", got \"\(stopName)\"")
            ok = ok && nameOK

            let arrivalSoc = plan.socCurve.last?.soc ?? -1
            let arrivalOK = isClose(arrivalSoc, 0.150, tol: 0.005)
            report("arrival-soc", arrivalOK, "expected 0.150 +/- 0.005, got \(arrivalSoc)")
            ok = ok && arrivalOK

            let totalOK = isClose(plan.totalTimeS, 14830, tol: 60)
            report("total-time-s", totalOK, "expected 14830 +/- 60, got \(plan.totalTimeS)")
            ok = ok && totalOK
        } catch {
            report("plan", false, "\(error)")
            ok = false
        }

        await finish(ok: ok)
    }

    // MARK: map-smoke

    /// Brings up the real map surface (via the app's shared PlanStore, same instance RootView
    /// renders) and checks: the style finishes loading, the pmtiles vector source is present,
    /// the Chargers layer is built from all 1,549 sideloaded chargers, and running the golden
    /// LU -> Amsterdam plan (same request as `runPlanGolden`) adds the route + stops layers.
    @MainActor
    private static func runMapSmoke(store: PlanStore) async {
        store.load()

        let styleLoaded = await waitWithTimeout(seconds: 20) { store.isStyleLoaded }
        report("style-loaded", styleLoaded)
        guard styleLoaded else { await finish(ok: false) }

        let pmtilesSourcePresent = store.mapView.style?.source(withIdentifier: "protomaps") != nil
        report("pmtiles-source-present", pmtilesSourcePresent)

        let expectedChargers = ["corridor": 1549, "eu-west": 40944, "lu-dev": 17][autotestRegion] ?? 1549
        let chargersReady = await waitWithTimeout(seconds: 15) { store.chargerCount > 0 }
        let chargersOK = chargersReady && store.chargerCount == expectedChargers
        report("chargers-count", chargersOK, "expected \(expectedChargers), got \(store.chargerCount)")

        var ok = styleLoaded && pmtilesSourcePresent && chargersOK

        do {
            let plan = try await store.runPlan(goldenRequest())

            let routePresent = store.mapView.style?.layer(withIdentifier: RouteLayer.routeLineId) != nil
            report("route-layer-present", routePresent)
            ok = ok && routePresent

            let stopsPresent = store.mapView.style?.layer(withIdentifier: RouteLayer.stopsCirclesId) != nil
            let stopCountOK = plan.stops.count == 1
            report("stops-layer-present", stopsPresent && stopCountOK, "expected 1 stop, got \(plan.stops.count)")
            ok = ok && stopsPresent && stopCountOK
        } catch {
            report("route-layer-present", false, "\(error)")
            report("stops-layer-present", false)
            ok = false
        }

        await finish(ok: ok)
    }

    // MARK: editor-smoke

    /// Drives the route editor's store mutations directly (no UI-event injection exists on
    /// the sim; the UI itself is verified by screenshot). Proves: the LU -> Amsterdam golden
    /// via the editor's replan path, a Waypoint insertion (ADR 0010 point 4) against its own
    /// golden, that racing edits with no awaits between them land only the last replan (the
    /// generation guard in PlanStore.replan()), and that removing the Waypoint replans back
    /// to the LU -> Amsterdam golden.
    @MainActor
    private static func runEditorSmoke(store: PlanStore) async {
        // Pin the origin before anything else: setOrigin marks the origin overridden, so a
        // CoreLocation fix inside the corridor can never adopt it mid-test -- this is what
        // makes the goldens deterministic regardless of the host machine's location.
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.load()

        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        var ok = true

        // Step 1: LU -> Amsterdam golden, via the editor's first destination selection.
        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let firstPlanLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        let (luAmsOk, luAmsDetail) = store.plan.map(assertLuAmsterdamGolden) ?? (false, "no plan")
        report("golden-lu-ams", firstPlanLanded && luAmsOk, firstPlanLanded ? luAmsDetail : "planVersion never reached 1")
        ok = ok && firstPlanLanded && luAmsOk

        // Step 2: race three replans with no awaits between them -- add Antwerp as a
        // Waypoint (starts a new, cold corridor assembly, ~1s), remove it, add it again.
        // Only the generation guard in PlanStore.replan() should let the last one land.
        let antwerp = CLLocationCoordinate2D(latitude: 51.2194, longitude: 4.4025)
        store.addWaypoint(name: "Antwerp", coordinate: antwerp)
        if let racedAwayId = store.waypoints.last?.id {
            store.removeWaypoint(id: racedAwayId)
        }
        store.addWaypoint(name: "Antwerp", coordinate: antwerp)

        let racesLanded = await waitWithTimeout(seconds: 60) {
            !store.isPlanning && (store.plan.map { assertWaypointGolden($0).0 } ?? false)
        }
        let (waypointGoldenOk, waypointGoldenDetail) = store.plan.map(assertWaypointGolden) ?? (false, "no plan")
        let waypointCountOk = store.waypoints.count == 1
        report(
            "golden-waypoint-after-races", racesLanded && waypointGoldenOk && waypointCountOk,
            racesLanded
                ? (waypointCountOk ? "" : "waypoints.count=\(store.waypoints.count), want 1")
                : "isPlanning=\(store.isPlanning): \(waypointGoldenDetail)"
        )
        ok = ok && racesLanded && waypointGoldenOk && waypointCountOk

        // Step 3: remove the Waypoint; the editor replans back to the LU -> Amsterdam golden.
        let versionBeforeRemoval = store.planVersion
        if let waypointId = store.waypoints.first?.id {
            store.removeWaypoint(id: waypointId)
        }
        let removalLanded = await waitWithTimeout(seconds: 30) {
            store.planVersion > versionBeforeRemoval && !store.isPlanning
        }
        let (removalGoldenOk, removalDetail) = store.plan.map(assertLuAmsterdamGolden) ?? (false, "no plan")
        report(
            "waypoint-removed-replans", removalLanded && removalGoldenOk,
            removalLanded ? removalDetail : "plan never landed after removal"
        )
        ok = ok && removalLanded && removalGoldenOk

        // The 8s post-DONE window is when the external screenshot of the planned-route
        // editor state (LU -> Amsterdam, Waypoint removed) is taken.
        await finish(ok: ok, sleepSeconds: 8)
    }

    /// Pinned observed values, LU -> Amsterdam, depart 0.90 (same golden as plan-golden).
    private static func assertLuAmsterdamGolden(_ plan: FfiPlan) -> (Bool, String) {
        var failures: [String] = []
        if plan.stops.count != 1 { failures.append("stops.count=\(plan.stops.count), want 1") }
        let stopName = plan.stops.first?.name ?? "<none>"
        let expectedName = "Hyperfast charge laadpalen Nossegem Zaventem"
        if stopName != expectedName { failures.append("stop name=\"\(stopName)\"") }
        if !isClose(plan.totalTimeS, 14830, tol: 60) { failures.append("totalTimeS=\(plan.totalTimeS)") }
        let arrivalSoc = plan.socCurve.last?.soc ?? -1
        if !isClose(arrivalSoc, 0.150, tol: 0.005) { failures.append("arrivalSoc=\(arrivalSoc)") }
        return (failures.isEmpty, failures.joined(separator: "; "))
    }

    /// Pinned observed values, LU -> Antwerp Waypoint -> Amsterdam, depart 0.90 (matches
    /// core/optimiser/tests/golden_corridor.rs's `golden_waypoint`).
    private static func assertWaypointGolden(_ plan: FfiPlan) -> (Bool, String) {
        var failures: [String] = []
        if plan.stops.count != 1 { failures.append("stops.count=\(plan.stops.count), want 1") }
        let stopName = plan.stops.first?.name ?? "<none>"
        if !stopName.contains("Nossegem") { failures.append("stop name=\"\(stopName)\"") }
        let chargeS = plan.stops.first?.chargeS ?? -1
        if !isClosePct(chargeS, 1033, pct: 0.10) { failures.append("chargeS=\(chargeS)") }
        if !isClosePct(plan.totalTimeS, 15449, pct: 0.05) { failures.append("totalTimeS=\(plan.totalTimeS)") }
        let arrivalSoc = plan.socCurve.last?.soc ?? -1
        if !isClose(arrivalSoc, 0.174, tol: 0.005) { failures.append("arrivalSoc=\(arrivalSoc)") }
        return (failures.isEmpty, failures.joined(separator: "; "))
    }

    // MARK: card-smoke

    /// Drives the store directly (no UI-event injection exists on the sim; the result card,
    /// chips, itinerary, and SoC chart are verified separately, by screenshot). Proves: the
    /// LU -> Amsterdam golden plan lands with its one stop, the derived dist-from-start for
    /// that stop (ChargingStopVM.stops(from:), wayfinder #43) matches the SoC curve's own
    /// post-charge jump, no stop-free alternative is offered for this non-micro stop despite
    /// `offerStopFreeAlternative` now defaulting to true, and the scrub marker lands near the
    /// stop's own coordinate.
    @MainActor
    private static func runCardSmoke(store: PlanStore) async {
        // Pin the origin before anything else -- same determinism rationale as editor-smoke.
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.load()

        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        var ok = true

        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        let expectedName = "Hyperfast charge laadpalen Nossegem Zaventem"
        let stopName = store.plan?.stops.first?.name ?? "<none>"
        let stopCountOK = store.plan?.stops.count == 1
        let nameOK = stopName == expectedName
        report(
            "plan-landed", planLanded && stopCountOK && nameOK,
            planLanded ? "stops=\(store.plan?.stops.count ?? -1) name=\"\(stopName)\"" : "planVersion never reached 1"
        )
        ok = ok && planLanded && stopCountOK && nameOK
        guard let plan = store.plan else { await finish(ok: false, sleepSeconds: 8) }

        // "stop-distance": cross-check the derived dist-from-start against the SoC curve's own
        // post-charge jump -- the (single) run of consecutive samples where soc increases.
        let stops = ChargingStopVM.stops(from: plan)
        let stopDistFromStartM = stops.first?.distFromStartM ?? -1
        let stopDistanceInRangeOK = stopDistFromStartM > 0 && stopDistFromStartM < plan.totalDistM

        var jumpRunEndDistM: [Double] = []
        var previousWasIncreasing = false
        for i in 1..<plan.socCurve.count {
            let prev = plan.socCurve[i - 1]
            let curr = plan.socCurve[i]
            if curr.soc > prev.soc {
                if previousWasIncreasing {
                    jumpRunEndDistM[jumpRunEndDistM.count - 1] = curr.distM
                } else {
                    jumpRunEndDistM.append(curr.distM)
                }
                previousWasIncreasing = true
            } else {
                previousWasIncreasing = false
            }
        }
        let jumpDistM = jumpRunEndDistM.first ?? -1
        let jumpMatchesOK = stops.count == 1 && stopDistanceInRangeOK
            && jumpRunEndDistM.count == 1 && abs(jumpDistM - stopDistFromStartM) <= 2000
        report(
            "stop-distance", jumpMatchesOK,
            "distFromStartM=\(stopDistFromStartM), jumpDistM=\(jumpDistM), totalDistM=\(plan.totalDistM)"
        )
        report("soc-jump-count", jumpRunEndDistM.count == 1, "expected 1, got \(jumpRunEndDistM.count)")
        ok = ok && jumpMatchesOK && jumpRunEndDistM.count == 1

        // "alternative-absent": the golden's ~16-min stop isn't a micro-stop, so no
        // alternative despite offerStopFreeAlternative now defaulting to true.
        let alternativeAbsentOK = plan.alternative == nil
        report("alternative-absent", alternativeAbsentOK, "alternative=\(String(describing: plan.alternative))")
        ok = ok && alternativeAbsentOK

        // "scrub-marker": selecting the stop's own distance should land the nearest-fraction
        // scrub marker on (or very near) the stop's own coordinate.
        store.selectedDistanceM = stopDistFromStartM
        let scrub = store.mapView.annotations?.first { $0.title == "Scrub" }
        let stopCoordinate = stops.first?.coordinate
        let scrubOK: Bool
        if let scrub, let stopCoordinate {
            scrubOK = abs(scrub.coordinate.latitude - stopCoordinate.latitude) <= 0.05
                && abs(scrub.coordinate.longitude - stopCoordinate.longitude) <= 0.05
        } else {
            scrubOK = false
        }
        report("scrub-marker", scrubOK, scrub == nil ? "no scrub annotation found" : "\(scrub!.coordinate)")
        ok = ok && scrubOK

        // The 8s post-DONE window is when the external screenshot of the EXPANDED card
        // (summary, chips, itinerary, SoC chart with the orange stop rule-mark) is taken.
        store.cardExpanded = true
        await finish(ok: ok, sleepSeconds: 8)
    }

    // MARK: settings-smoke

    /// Drives the settings sheet's store-bound fields directly (no UI-event injection exists
    /// on the sim; the sheet UI itself is verified by screenshot). Proves: the stop-free LU ->
    /// Antwerp golden at the 0.90 default, the depart-30 golden (with its Speed Cap
    /// expectation) after `departSoc` drops to 0.30 -- both per-region, see
    /// `depart30Goldens` -- a genuinely different plan after a temperature change (no pinned
    /// golden exists at -5 °C), and the appearance override actually swapping the map style.
    @MainActor
    private static func runSettingsSmoke(store: PlanStore) async {
        // Pin the origin before anything else -- same determinism rationale as editor-smoke.
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.load()

        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        var ok = true

        // Step 1: LU -> Antwerp at the 0.90 default -- stop-free.
        store.setDestination(name: "Antwerp", coordinate: CLLocationCoordinate2D(latitude: 51.2194, longitude: 4.4025))
        let firstPlanLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        let stopFreeOk = store.plan.map { $0.stops.isEmpty && ($0.socCurve.last?.soc ?? -1) > 0.10 } ?? false
        report(
            "golden-antwerp-stop-free", firstPlanLanded && stopFreeOk,
            firstPlanLanded
                ? "stops=\(store.plan?.stops.count ?? -1) arrivalSoc=\(store.plan?.socCurve.last?.soc ?? -1)"
                : "planVersion never reached 1"
        )
        ok = ok && firstPlanLanded && stopFreeOk

        // Step 2: departSoc = 0.30 -- this didSet-triggered replan IS the settings path.
        // Matches core/optimiser/tests/golden_corridor.rs's golden_speed_cap_exercised.
        let versionBeforeDepartChange = store.planVersion
        store.departSoc = 0.30
        let departPlanLanded = await waitWithTimeout(seconds: 30) {
            store.planVersion > versionBeforeDepartChange && !store.isPlanning
        }
        let (departGoldenOk, departDetail) = store.plan.map(assertDepart30Golden) ?? (false, "no plan")
        report(
            "golden-depart-30", departPlanLanded && departGoldenOk,
            departPlanLanded ? departDetail : "plan never landed after departSoc change"
        )
        ok = ok && departPlanLanded && departGoldenOk

        let expectedSpeedCapKmh = (depart30Goldens[autotestRegion] ?? depart30Goldens["corridor"]!).lastLegSpeedCapKmh
        let lastLegSpeedCapKmh = store.plan?.legs.last?.speedCapKmh
        let speedCapOk = lastLegSpeedCapKmh == expectedSpeedCapKmh
        report(
            "speed-cap", speedCapOk,
            "expected \(String(describing: expectedSpeedCapKmh)), got \(String(describing: lastLegSpeedCapKmh))"
        )
        ok = ok && speedCapOk

        // Step 3: tempC = -5 re-assembles the corridor (~1s); no pinned golden exists at
        // -5 °C, so this only checks that a genuinely different plan landed.
        let recordedChargeTimeS = store.plan?.chargeTimeS ?? -1
        let recordedTotalTimeS = store.plan?.totalTimeS ?? -1
        let versionBeforeTempChange = store.planVersion
        store.tempC = -5.0
        let tempPlanLanded = await waitWithTimeout(seconds: 30) {
            store.planVersion > versionBeforeTempChange && !store.isPlanning
        }
        let tempChangedOk = tempPlanLanded && (
            (store.plan?.chargeTimeS ?? -1) != recordedChargeTimeS
                || (store.plan?.totalTimeS ?? -1) != recordedTotalTimeS
        )
        report(
            "temp-replans", tempChangedOk,
            tempPlanLanded
                ? "chargeTimeS=\(store.plan?.chargeTimeS ?? -1) totalTimeS=\(store.plan?.totalTimeS ?? -1)"
                : "plan never landed after tempC change"
        )
        ok = ok && tempChangedOk

        // Step 4: the appearance override actually swaps the map style -- isStyleLoaded flips
        // false -> true across the swap (PlanStore.applyStyle resets it before the reload).
        store.appearanceOverride = "dark"
        let styleReloaded = await waitWithTimeout(seconds: 15) { store.isStyleLoaded }
        let appearanceOk = styleReloaded && store.isDarkAppearance
        report(
            "appearance-override", appearanceOk,
            "isStyleLoaded=\(store.isStyleLoaded) isDarkAppearance=\(store.isDarkAppearance)"
        )
        ok = ok && appearanceOk

        // The 8s post-DONE window is when the external screenshot of the OPEN settings sheet
        // (medium detent) over the dark-styled map is taken.
        store.showingSettings = true
        await finish(ok: ok, sleepSeconds: 8)
    }

    /// Per-region expectations for the depart-0.30 golden -- the richer eu-west charger set
    /// yields a legitimately different optimal plan than the corridor pack.
    private struct Depart30Golden {
        let stopCount: Int
        /// One name per expected stop, in order; only as many are checked as `stopCount` calls for.
        let stopNames: [String]
        let stopArrivalSoc: Double
        let stopDepartSoc: Double
        let chargeS: Double
        let lastLegArrivalSoc: Double
        let lastLegSpeedCapKmh: Double?
    }

    /// corridor matches core/optimiser/tests/golden_corridor.rs's `golden_speed_cap_exercised`;
    /// eu-west values were pinned from the same request run against the eu-west pack
    /// (wayfinder #50).
    private static let depart30Goldens: [String: Depart30Golden] = [
        "corridor": Depart30Golden(
            stopCount: 1,
            stopNames: ["SuperChargy - Aire de Capellen direction Arlon"],
            stopArrivalSoc: 0.247,
            stopDepartSoc: 0.800,
            chargeS: 993,
            lastLegArrivalSoc: 0.111,
            lastLegSpeedCapKmh: 100.0
        ),
        "eu-west": Depart30Golden(
            stopCount: 2,
            stopNames: ["Ibis Styles - Arlon", "Hyperfast charge laadpalen Nossegem Zaventem"],
            stopArrivalSoc: 0.186,
            stopDepartSoc: 0.800,
            chargeS: 955,
            lastLegArrivalSoc: 0.150,
            lastLegSpeedCapKmh: nil
        ),
    ]

    /// Pinned observed values, LU -> Antwerp, depart 0.30, per active region -- see
    /// `depart30Goldens` for provenance.
    private static func assertDepart30Golden(_ plan: FfiPlan) -> (Bool, String) {
        let golden = depart30Goldens[autotestRegion] ?? depart30Goldens["corridor"]!

        var failures: [String] = []
        if plan.stops.count != golden.stopCount {
            failures.append("stops.count=\(plan.stops.count), want \(golden.stopCount)")
        }
        for (index, expectedName) in golden.stopNames.enumerated() where index < plan.stops.count {
            let stopName = plan.stops[index].name
            if stopName != expectedName { failures.append("stop[\(index)] name=\"\(stopName)\"") }
        }
        let stopArrivalSoc = plan.stops.first?.arrivalSoc ?? -1
        if !isClose(stopArrivalSoc, golden.stopArrivalSoc, tol: 0.005) {
            failures.append("stop arrivalSoc=\(stopArrivalSoc)")
        }
        let stopDepartSoc = plan.stops.first?.departSoc ?? -1
        if !isClose(stopDepartSoc, golden.stopDepartSoc, tol: 0.005) {
            failures.append("stop departSoc=\(stopDepartSoc)")
        }
        let chargeS = plan.stops.first?.chargeS ?? -1
        if !isClosePct(chargeS, golden.chargeS, pct: 0.10) { failures.append("chargeS=\(chargeS)") }
        let lastLegArrivalSoc = plan.legs.last?.arrivalSoc ?? -1
        if !isClose(lastLegArrivalSoc, golden.lastLegArrivalSoc, tol: 0.005) {
            failures.append("last leg arrivalSoc=\(lastLegArrivalSoc)")
        }
        return (failures.isEmpty, failures.joined(separator: "; "))
    }

    // MARK: perf

    /// `--autotest perf` (wayfinder #45) measures what the ADR 0001 M2 gate checks:
    /// cold-start -> first-plan, cold vs warm plan() latency, the departSoc-only replan the
    /// cross-call corridor cache (#38) exists for, and resident memory after the three
    /// plans. Routed through PlannerClient directly, like plan-golden -- no map/store,
    /// keeping the measurement clean of MapLibre. Golden shape asserts (the same LU ->
    /// Amsterdam pins as plan-golden) guard the cold and warm plans so a broken plan can't
    /// report healthy perf; the numbers themselves aren't asserted here -- they're
    /// environment-dependent (sim vs device) and the gate doc holds the verdicts.
    private static func runPerf() async {
        guard let located = Packs.locate(region: autotestRegion) else {
            report("pack-present", false, "Documents/\(autotestRegion).rpack or -chargers.json missing")
            await finish(ok: false)
        }
        report("pack-present", true)

        var ok = true
        do {
            let client = try PlannerClient(regionPackPath: located.rpackURL.path)
            let chargerBytes = try Data(contentsOf: located.chargersURL)
            try client.loadChargers(bytes: chargerBytes, format: "cpack-1")
            report("chargers-loaded", true)

            let coldStart = DispatchTime.now()
            let coldPlan = try await client.plan(goldenRequest())
            let coldElapsedMs = Double(DispatchTime.now().uptimeNanoseconds - coldStart.uptimeNanoseconds) / 1_000_000
            let coldFromLaunchMs = (ProcessInfo.processInfo.systemUptime - WayfinderApp.launchUptime) * 1000
            print("WAYFINDER-AUTOTEST: perf cold_from_launch_ms=\(String(format: "%.1f", coldFromLaunchMs))")
            print("WAYFINDER-AUTOTEST: perf plan_cold_ms=\(String(format: "%.1f", coldElapsedMs))")

            let (coldGoldenOk, coldGoldenDetail) = assertLuAmsterdamGolden(coldPlan)
            report("golden-cold", coldGoldenOk, coldGoldenDetail)
            ok = ok && coldGoldenOk

            let warmStart = DispatchTime.now()
            let warmPlan = try await client.plan(goldenRequest())
            let warmElapsedMs = Double(DispatchTime.now().uptimeNanoseconds - warmStart.uptimeNanoseconds) / 1_000_000
            print("WAYFINDER-AUTOTEST: perf plan_warm_ms=\(String(format: "%.1f", warmElapsedMs))")

            let (warmGoldenOk, warmGoldenDetail) = assertLuAmsterdamGolden(warmPlan)
            report("golden-warm", warmGoldenOk, warmGoldenDetail)
            ok = ok && warmGoldenOk

            // Only departSoc changes -- the cross-call corridor cache (#38) should skip
            // assembly and go straight to search::solve (matches core/ffi/plan_cli.rs's
            // "warm soc=0.85" measurement). No pinned golden exists at this departSoc.
            var socRequest = goldenRequest()
            socRequest.departSoc = 0.85
            let socStart = DispatchTime.now()
            _ = try await client.plan(socRequest)
            let socElapsedMs = Double(DispatchTime.now().uptimeNanoseconds - socStart.uptimeNanoseconds) / 1_000_000
            print("WAYFINDER-AUTOTEST: perf replan_soc_ms=\(String(format: "%.1f", socElapsedMs))")

            let footprintMB = physFootprintMB()
            print("WAYFINDER-AUTOTEST: perf phys_footprint_mb=\(String(format: "%.1f", footprintMB))")
        } catch {
            report("plan", false, "\(error)")
            ok = false
        }

        await finish(ok: ok)
    }

    /// `task_vm_info.phys_footprint` via `task_info` -- the same figure Xcode's memory gauge
    /// reports. Ported from the prototype flyover benchmark's memory probe (`git show
    /// prototype/planner-ui:prototype/ios/SliceProto/BenchmarkFlyover.swift`); its
    /// CADisplayLink fps sampling and camera flyover aren't ported -- fps isn't an ADR 0001
    /// bar.
    private static func physFootprintMB() -> Double {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let kr: kern_return_t = withUnsafeMutablePointer(to: &info) { ptr -> kern_return_t in
            ptr.withMemoryRebound(to: integer_t.self, capacity: Int(count)) { intPtr in
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), intPtr, &count)
            }
        }
        guard kr == KERN_SUCCESS else { return 0 }
        return Double(info.phys_footprint) / 1_048_576.0
    }

    // MARK: install-smoke

    /// Exercises the installer's real code path against the live hosted catalog (wayfinder
    /// #47): fetches the index, installs lu-dev (a real ~49MB download, small enough to run in
    /// an autotest), checks the installed files and record, opens a PlannerClient on the
    /// installed rpack and parses its Charger Pack (lu-dev's known 17 chargers), proves
    /// `verifyRegionPack` (issue #56 / SEC-006) passes on the installed .rpack and rejects a
    /// junk-bytes file, and that every committed artifact is excluded from backup (SEC-010),
    /// then deletes the region and checks the artifact files + record are gone while the
    /// shared style files remain.
    @MainActor
    private static func runInstallSmoke(installer: PackInstaller) async {
        var ok = true
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]

        let index: PackIndex
        do {
            index = try await PackCatalogClient.fetchIndex()
        } catch {
            report("index-fetch", false, "\(error)")
            await finish(ok: false)
        }
        let luDevPresent = index.regions.contains { $0.id == "lu-dev" }
        let regionsOk = index.regions.count >= 2 && luDevPresent
        report("index-regions", regionsOk, "count=\(index.regions.count) lu-dev-present=\(luDevPresent)")
        ok = ok && regionsOk

        do {
            try await installer.install(region: "lu-dev")
            report("install-lu-dev", true)
        } catch {
            report("install-lu-dev", false, "\(error)")
            await finish(ok: false)
        }

        let expectedFiles = ["lu-dev.rpack", "lu-dev-chargers.json", "lu-dev.pmtiles", "style-light.json", "style-dark.json"]
        let filesOk = expectedFiles.allSatisfy { FileManager.default.fileExists(atPath: docs.appendingPathComponent($0).path) }
        report("install-files-present", filesOk, filesOk ? "" : "one or more of \(expectedFiles) missing")
        ok = ok && filesOk

        let record = PackInstaller.loadRecord(region: "lu-dev")
        let catalogEpoch = (try? await PackCatalogClient.fetchCatalog(region: "lu-dev"))?.osmSnapshotEpoch
        let epochOk = record != nil && catalogEpoch != nil && record?.epoch == catalogEpoch
        report(
            "installed-epoch-matches", epochOk,
            "installed=\(String(describing: record?.epoch)) catalog=\(String(describing: catalogEpoch))"
        )
        ok = ok && epochOk

        do {
            let rpackURL = docs.appendingPathComponent("lu-dev.rpack")
            let chargersURL = docs.appendingPathComponent("lu-dev-chargers.json")
            let client = try PlannerClient(regionPackPath: rpackURL.path)
            let chargerBytes = try Data(contentsOf: chargersURL)
            try client.loadChargers(bytes: chargerBytes, format: "cpack-1")
            let chargers = try CPack1.parseChargers(data: chargerBytes)
            let countOk = chargers.count == 17
            report("lu-dev-charger-count", countOk, "expected 17, got \(chargers.count)")
            ok = ok && countOk
        } catch {
            report("lu-dev-charger-count", false, "\(error)")
            ok = false
        }

        // -- SEC-006: verifyRegionPack succeeds on a real, live-catalog-installed .rpack --
        // the pass case the Rust tests couldn't cover (they only have synthetic/corrupt
        // packs). Off the main actor, same as PackInstaller's own call.
        let luDevRpackURL = docs.appendingPathComponent("lu-dev.rpack")
        do {
            try await Task.detached(priority: .userInitiated) {
                try verifyRegionPack(path: luDevRpackURL.path)
            }.value
            report("deep-verify-valid-pack", true)
        } catch {
            report("deep-verify-valid-pack", false, "\(error)")
            ok = false
        }

        // -- SEC-006: verifyRegionPack rejects a file that's just junk bytes, not a real
        // .rpack at all.
        let garbageURL = FileManager.default.temporaryDirectory.appendingPathComponent("autotest-garbage-\(UUID().uuidString).rpack")
        try? Data((0..<256).map { _ in UInt8.random(in: 0...255) }).write(to: garbageURL)
        var garbageRejected = false
        do {
            try await Task.detached(priority: .userInitiated) {
                try verifyRegionPack(path: garbageURL.path)
            }.value
        } catch {
            garbageRejected = true
        }
        report("deep-verify-rejects-garbage", garbageRejected)
        ok = ok && garbageRejected
        try? FileManager.default.removeItem(at: garbageURL)

        // -- SEC-010: every committed lu-dev artifact (plus the shared style files) is
        // excluded from device backup.
        let backupExcludedOk = expectedFiles.allSatisfy { name in
            let values = try? docs.appendingPathComponent(name).resourceValues(forKeys: [.isExcludedFromBackupKey])
            return values?.isExcludedFromBackup == true
        }
        report("backup-excluded", backupExcludedOk)
        ok = ok && backupExcludedOk

        do {
            try installer.delete(region: "lu-dev")
            report("delete-lu-dev", true)
        } catch {
            report("delete-lu-dev", false, "\(error)")
            ok = false
        }

        let deletedFilesGone = ["lu-dev.rpack", "lu-dev-chargers.json", "lu-dev.pmtiles"].allSatisfy {
            !FileManager.default.fileExists(atPath: docs.appendingPathComponent($0).path)
        }
        let recordGone = PackInstaller.loadRecord(region: "lu-dev") == nil
        let stylesRemain = ["style-light.json", "style-dark.json"].allSatisfy {
            FileManager.default.fileExists(atPath: docs.appendingPathComponent($0).path)
        }
        report(
            "delete-cleanup", deletedFilesGone && recordGone && stylesRemain,
            "filesGone=\(deletedFilesGone) recordGone=\(recordGone) stylesRemain=\(stylesRemain)"
        )
        ok = ok && deletedFilesGone && recordGone && stylesRemain

        ok = await runInstallReconciliationAndValidationChecks(installer: installer, docs: docs) && ok

        await finish(ok: ok)
    }

    /// Codebase-audit remediation checks (docs/codebase-audit-2026-08-29.md), all offline: no
    /// network, no new PackInstaller instance (a second background URLSession with the same
    /// identifier as the app's own would be a problem) -- reuses the one already passed in.
    @MainActor
    private static func runInstallReconciliationAndValidationChecks(installer: PackInstaller, docs: URL) async -> Bool {
        var ok = true

        // -- H-01: journal roll-forward -- a staged file + journal survive a simulated crash;
        // reconcileJournal should finish the commit: move the file into place, write the
        // record, and clean up the journal.
        let forwardRegion = "autotest-journal-forward"
        let forwardEpoch = 1
        let forwardFile = "\(forwardRegion).rpack"
        let forwardContent = Data("autotest journal roll-forward content".utf8)
        let forwardSha256 = SHA256.hash(data: forwardContent).map { String(format: "%02x", $0) }.joined()
        let forwardStagingDir = PackInstaller.stagingDirURL(docs: docs, region: forwardRegion, epoch: forwardEpoch)
        let forwardDestURL = docs.appendingPathComponent(forwardFile)
        let forwardRecordURL = docs.appendingPathComponent("installed-\(forwardRegion).json")
        try? FileManager.default.createDirectory(at: forwardStagingDir, withIntermediateDirectories: true)
        try? forwardContent.write(to: forwardStagingDir.appendingPathComponent(forwardFile))
        try? FileManager.default.removeItem(at: forwardDestURL)
        try? FileManager.default.removeItem(at: forwardRecordURL)
        let forwardRecord = InstalledRecord(
            regionId: forwardRegion, epoch: forwardEpoch,
            artifacts: ["region_pack": InstalledArtifactRecord(file: forwardFile, sha256: forwardSha256)]
        )
        let forwardJournal = CommitJournal(
            regionId: forwardRegion, epoch: forwardEpoch,
            entries: [CommitJournalEntry(stagedFile: forwardFile, destinationFile: forwardFile, sha256: forwardSha256)],
            record: forwardRecord
        )
        try? JSONEncoder().encode(forwardJournal).write(to: PackInstaller.journalURL(docs: docs, region: forwardRegion))

        PackInstaller.reconcileJournal(region: forwardRegion, documentsURL: docs)

        let forwardFileLanded = (try? Data(contentsOf: forwardDestURL)) == forwardContent
        let forwardRecordWritten = PackInstaller.loadRecord(region: forwardRegion) == forwardRecord
        let forwardJournalGone = !FileManager.default.fileExists(
            atPath: PackInstaller.journalURL(docs: docs, region: forwardRegion).path
        )
        let forwardOk = forwardFileLanded && forwardRecordWritten && forwardJournalGone
        report(
            "reconcile-journal-roll-forward", forwardOk,
            "fileLanded=\(forwardFileLanded) recordWritten=\(forwardRecordWritten) journalGone=\(forwardJournalGone)"
        )
        ok = ok && forwardOk
        try? FileManager.default.removeItem(at: forwardDestURL)
        try? FileManager.default.removeItem(at: forwardRecordURL)

        // -- H-01: unrecoverable journal -- the staged file is gone AND the destination
        // doesn't sha-match the journal (simulating a crash after the staged copy was
        // consumed some other way, or a transfer that never actually landed). reconcileJournal
        // must abandon the transaction: old file + old record untouched, journal cleaned up.
        let badRegion = "autotest-journal-unrecoverable"
        let badEpoch = 1
        let badFile = "\(badRegion).rpack"
        let badStagingDir = PackInstaller.stagingDirURL(docs: docs, region: badRegion, epoch: badEpoch)
        let badDestURL = docs.appendingPathComponent(badFile)
        let badRecordURL = docs.appendingPathComponent("installed-\(badRegion).json")
        try? FileManager.default.createDirectory(at: badStagingDir, withIntermediateDirectories: true)
        // No staged file is written -- it's "gone".
        let oldContent = Data("old install, still here".utf8)
        try? oldContent.write(to: badDestURL)
        let bogusSha256 = String(repeating: "0", count: 64)
        let oldRecord = InstalledRecord(regionId: badRegion, epoch: 0, artifacts: [:])
        try? JSONEncoder().encode(oldRecord).write(to: badRecordURL)
        let badJournal = CommitJournal(
            regionId: badRegion, epoch: badEpoch,
            entries: [CommitJournalEntry(stagedFile: badFile, destinationFile: badFile, sha256: bogusSha256)],
            record: InstalledRecord(
                regionId: badRegion, epoch: badEpoch,
                artifacts: ["region_pack": InstalledArtifactRecord(file: badFile, sha256: bogusSha256)]
            )
        )
        try? JSONEncoder().encode(badJournal).write(to: PackInstaller.journalURL(docs: docs, region: badRegion))

        PackInstaller.reconcileJournal(region: badRegion, documentsURL: docs)

        let oldFilePreserved = (try? Data(contentsOf: badDestURL)) == oldContent
        let oldRecordPreserved = PackInstaller.loadRecord(region: badRegion) == oldRecord
        let badJournalGone = !FileManager.default.fileExists(atPath: PackInstaller.journalURL(docs: docs, region: badRegion).path)
        let badStagingGone = !FileManager.default.fileExists(atPath: badStagingDir.path)
        let unrecoverableOk = oldFilePreserved && oldRecordPreserved && badJournalGone && badStagingGone
        report(
            "reconcile-journal-unrecoverable", unrecoverableOk,
            "oldFilePreserved=\(oldFilePreserved) oldRecordPreserved=\(oldRecordPreserved) "
                + "journalGone=\(badJournalGone) stagingGone=\(badStagingGone)"
        )
        ok = ok && unrecoverableOk
        try? FileManager.default.removeItem(at: badDestURL)
        try? FileManager.default.removeItem(at: badRecordURL)

        // -- M-02: catalog validator rejections, including the audit's own path-traversal
        // proof (appending "../escape" and standardizing walks outside the base URL/dir).
        let goodRegionIdOk = PackCatalogValidator.validateRegionId("lu-dev")
        let badRegionIdOk = !PackCatalogValidator.validateRegionId("LU_DEV!")
        report("validate-region-id", goodRegionIdOk && badRegionIdOk, "good=\(goodRegionIdOk) badRejected=\(badRegionIdOk)")
        ok = ok && goodRegionIdOk && badRegionIdOk

        let escapeFileRejected = !PackCatalogValidator.validateFileName("../escape")
        let leafFileOk = PackCatalogValidator.validateFileName("lu-dev.rpack")
        report(
            "validate-artifact-file-path-traversal", escapeFileRejected && leafFileOk,
            "escapeRejected=\(escapeFileRejected) leafOk=\(leafFileOk)"
        )
        ok = ok && escapeFileRejected && leafFileOk

        let badShaRejected = !PackCatalogValidator.validateSha256("not-a-sha")
        let goodShaOk = PackCatalogValidator.validateSha256(String(repeating: "a", count: 64))
        report("validate-sha256", badShaRejected && goodShaOk, "badRejected=\(badShaRejected) goodOk=\(goodShaOk)")
        ok = ok && badShaRejected && goodShaOk

        let remotePathEscapes = !PackCatalogValidator.validateRemotePath("../escape", baseURL: PackCatalogClient.baseURL)
        let remotePathStaysUnder = PackCatalogValidator.validateRemotePath(
            "packs/lu-dev/1/lu-dev.rpack", baseURL: PackCatalogClient.baseURL
        )
        report(
            "validate-remote-path-traversal", remotePathEscapes && remotePathStaysUnder,
            "escapeRejected=\(remotePathEscapes) normalOk=\(remotePathStaysUnder)"
        )
        ok = ok && remotePathEscapes && remotePathStaysUnder

        let validArtifact = PackArtifact(
            file: "lu-dev.rpack", bytes: 100, sha256: String(repeating: "a", count: 64),
            path: "packs/lu-dev/1/lu-dev.rpack"
        )
        let escapingArtifact = PackArtifact(
            file: "../escape", bytes: 100, sha256: String(repeating: "a", count: 64),
            path: "packs/lu-dev/1/lu-dev.rpack"
        )
        let malformedCatalog = PackCatalog(
            regionId: "lu-dev", regionName: "LU Dev", osmSnapshotEpoch: 1,
            artifacts: PackArtifacts(regionPack: escapingArtifact, chargerPack: validArtifact, mapPack: validArtifact)
        )
        var malformedCatalogRejected = false
        do {
            try PackCatalogValidator.validate(catalog: malformedCatalog, requestedRegion: "lu-dev")
        } catch {
            malformedCatalogRejected = true
        }
        report("validate-catalog-rejects-malformed-file", malformedCatalogRejected)
        ok = ok && malformedCatalogRejected

        let mismatchedCatalog = PackCatalog(
            regionId: "lu-dev", regionName: "LU Dev", osmSnapshotEpoch: 1,
            artifacts: PackArtifacts(regionPack: validArtifact, chargerPack: validArtifact, mapPack: validArtifact)
        )
        var regionMismatchRejected = false
        do {
            try PackCatalogValidator.validate(catalog: mismatchedCatalog, requestedRegion: "eu-west")
        } catch {
            regionMismatchRejected = true
        }
        report("validate-catalog-rejects-region-mismatch", regionMismatchRejected)
        ok = ok && regionMismatchRejected

        // -- M-01: a record with no backing files must not read as plain "Installed".
        let repairRegion = "autotest-needs-repair"
        let repairRecordURL = docs.appendingPathComponent("installed-\(repairRegion).json")
        let repairRecord = InstalledRecord(
            regionId: repairRegion, epoch: 1,
            artifacts: ["region_pack": InstalledArtifactRecord(file: "\(repairRegion).rpack", sha256: String(repeating: "a", count: 64))]
        )
        try? PackInstaller.saveRecord(repairRecord)
        installer.refreshRows()
        let repairRow = installer.rows.first { $0.id == repairRegion }
        let needsRepairOk = repairRow?.needsRepair == true && repairRow?.installedEpoch == 1
        report("needs-repair-row-flagged", needsRepairOk, "row=\(String(describing: repairRow))")
        ok = ok && needsRepairOk
        try? FileManager.default.removeItem(at: repairRecordURL)

        return ok
    }

    // MARK: triplog-smoke

    /// M-06 (docs/codebase-audit-2026-08-29.md): exercises TripLogStore.ingest's producer
    /// contract on a disposable probe store -- out-of-order timestamps, exact-duplicate
    /// timestamps, invalid (0, 0) coordinates, and sub-0.5s thinning are all dropped, while a
    /// valid fix spaced >= 0.5s past the last kept one is still kept.
    @MainActor
    private static func runIngestContractChecks() -> Bool {
        var ok = true
        let probeStore = TripLogStore()
        probeStore.authorizationStatus = { .authorizedAlways }
        probeStore.startTapped()
        probeStore.confirmStartSoc(50)
        guard let probeStart = probeStore.tripStartDate else {
            report("ingest-contract-probe-start", false)
            return false
        }

        func probeFix(t: Double, lat: Double = 49.6116, lon: Double = 6.1319) -> CLLocation {
            CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: lat, longitude: lon),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
                course: 0, speed: 10, timestamp: probeStart.addingTimeInterval(t)
            )
        }

        probeStore.ingest(probeFix(t: 0)) // kept: first sample
        probeStore.ingest(probeFix(t: 1.0)) // kept: 1.0s after the last kept sample
        let baseline = probeStore.sampleCount
        let baselineOk = baseline == 2
        report("ingest-baseline", baselineOk, "count=\(baseline)")
        ok = ok && baselineOk

        probeStore.ingest(probeFix(t: 0.6)) // out-of-order: before the last kept sample's t=1.0
        let outOfOrderDropped = probeStore.sampleCount == baseline
        report("ingest-out-of-order-dropped", outOfOrderDropped, "count=\(probeStore.sampleCount)")
        ok = ok && outOfOrderDropped

        probeStore.ingest(probeFix(t: 1.0)) // duplicate: exactly the last kept sample's t
        let duplicateDropped = probeStore.sampleCount == baseline
        report("ingest-duplicate-timestamp-dropped", duplicateDropped, "count=\(probeStore.sampleCount)")
        ok = ok && duplicateDropped

        probeStore.ingest(probeFix(t: 1.6, lat: 0, lon: 0)) // invalid: exact (0, 0)
        let invalidCoordDropped = probeStore.sampleCount == baseline
        report("ingest-invalid-coordinate-dropped", invalidCoordDropped, "count=\(probeStore.sampleCount)")
        ok = ok && invalidCoordDropped

        probeStore.ingest(probeFix(t: 1.2)) // thinned: only 0.2s after the last kept sample
        let thinnedDropped = probeStore.sampleCount == baseline
        report("ingest-sub-half-second-thinned", thinnedDropped, "count=\(probeStore.sampleCount)")
        ok = ok && thinnedDropped

        probeStore.ingest(probeFix(t: 1.6)) // kept: 0.6s after the last kept sample
        let keptAfterWindow = probeStore.sampleCount == baseline + 1
        report("ingest-valid-fix-kept", keptAfterWindow, "count=\(probeStore.sampleCount)")
        ok = ok && keptAfterWindow

        return ok
    }

    /// Drives TripLogStore's capture lifecycle directly. Proves: denied authorization refuses
    /// to start recording (M-06 -- docs/codebase-audit-2026-08-29.md), the ingest producer
    /// contract (out-of-order/duplicate timestamps, invalid coordinates, sub-1Hz thinning), the
    /// start/end SoC prompts and their phase transitions, the cancel-end-SoC data-loss guard
    /// (cancelling resumes recording rather than truncating the trace), and that the saved
    /// tlog-1 JSON matches the schema on the fields #52's Rust `calibrate()` will parse --
    /// including a byte-level spot-check that the on-disk keys are really snake_case,
    /// independent of the Swift model -- and, at the end, the SEC-010 (issue #56) data-deletion
    /// controls: `tripStore.deleteAllLogs()` over synthetic logs and
    /// `RecentDestination.clearAll()` over a seeded UserDefaults entry.
    @MainActor
    private static func runTriplogSmoke(tripStore: TripLogStore) async {
        var ok = true

        // -- M-06: denied authorization refuses to start recording.
        tripStore.authorizationStatus = { .denied }
        tripStore.startTapped()
        tripStore.confirmStartSoc(90)
        let deniedRefused = tripStore.phase == .idle && tripStore.captureErrorMessage != nil
        report(
            "denied-start-refused", deniedRefused,
            "phase=\(tripStore.phase) captureErrorMessage=\(tripStore.captureErrorMessage ?? "none")"
        )
        ok = ok && deniedRefused

        // -- M-06: ingest producer-contract checks, on an isolated probe store so they don't
        // perturb the 125-sample golden flow below (a real trip's own tripStartDate/samples).
        ok = runIngestContractChecks() && ok

        tripStore.fetchTemperature = { _, _, _ in 14.5 }
        // Deterministic regardless of the simulator's actual location-permission state --
        // `deniedRefused` above already proved the denial path via the injectable closure.
        tripStore.authorizationStatus = { .authorizedAlways }

        tripStore.startTapped()
        let promptingStart = tripStore.phase == .promptingStartSoc
        report("prompting-start-soc", promptingStart, "phase=\(tripStore.phase)")
        ok = ok && promptingStart

        tripStore.confirmStartSoc(90)
        let recordingAfterStart = tripStore.phase == .recording
        report("recording-after-start", recordingAfterStart, "phase=\(tripStore.phase)")
        ok = ok && recordingAfterStart
        guard let tripStartDate = tripStore.tripStartDate else {
            report("trip-start-date", false)
            await finish(ok: false)
        }

        let originLat = 49.6116
        let originLon = 6.1319
        let latStepPerSecond = 0.00025

        func syntheticLocation(secondOffset: Int) -> CLLocation {
            CLLocation(
                coordinate: CLLocationCoordinate2D(
                    latitude: originLat + latStepPerSecond * Double(secondOffset), longitude: originLon
                ),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
                course: 0, speed: 27.8,
                timestamp: tripStartDate.addingTimeInterval(Double(secondOffset))
            )
        }

        for i in 0..<120 {
            tripStore.ingest(syntheticLocation(secondOffset: i))
        }

        tripStore.stopTapped()
        let promptingEnd = tripStore.phase == .promptingEndSoc
        report("prompting-end-soc", promptingEnd, "phase=\(tripStore.phase)")
        ok = ok && promptingEnd

        tripStore.cancelEndSoc()
        let backToRecording = tripStore.phase == .recording
        report("cancel-end-soc-resumes-recording", backToRecording, "phase=\(tripStore.phase)")
        ok = ok && backToRecording

        for i in 120..<125 {
            tripStore.ingest(syntheticLocation(secondOffset: i))
        }

        tripStore.stopTapped()
        tripStore.confirmEndSoc(82)

        let saved = await waitWithTimeout(seconds: 10) { tripStore.lastSavedURL != nil }
        report("saved", saved, "saveErrorMessage=\(tripStore.saveErrorMessage ?? "none")")
        ok = ok && saved
        guard saved, let url = tripStore.lastSavedURL else { await finish(ok: false) }

        do {
            let data = try Data(contentsOf: url)
            let log = try JSONDecoder().decode(TripLog.self, from: data)

            let formatOk = log.format == "tlog-1"
            report("format", formatOk, "got \(log.format)")
            ok = ok && formatOk

            let vehicleOk = log.vehicle == "ioniq5_lr_2wd"
            report("vehicle", vehicleOk, "got \(log.vehicle)")
            ok = ok && vehicleOk

            let socOk = log.startSocPct == 90 && log.endSocPct == 82
            report("soc", socOk, "start=\(log.startSocPct) end=\(log.endSocPct)")
            ok = ok && socOk

            let tempOk = log.ambientTempC == 14.5
            report("ambient-temp", tempOk, "got \(String(describing: log.ambientTempC))")
            ok = ok && tempOk

            let countOk = log.samples.count == 125
            report("sample-count", countOk, "expected 125, got \(log.samples.count)")
            ok = ok && countOk

            let strictlyIncreasing = zip(log.samples, log.samples.dropFirst()).allSatisfy { $0.t < $1.t }
            report("t-strictly-increasing", strictlyIncreasing)
            ok = ok && strictlyIncreasing

            let first = log.samples.first
            let firstCoordOk = first.map { isClose($0.lat, originLat, tol: 1e-9) && isClose($0.lon, originLon, tol: 1e-9) } ?? false
            report("first-sample-coords", firstCoordOk, "got \(String(describing: first))")
            ok = ok && firstCoordOk

            let lastT = log.samples.last?.t ?? -1
            let lastTOk = isClose(lastT, 124, tol: 1)
            report("last-t", lastTOk, "got \(lastT)")
            ok = ok && lastTOk

            let lastSpeedOk = isClose(log.samples.last?.speedMps ?? -1, 27.8, tol: 0.1)
            report("last-speed", lastSpeedOk, "got \(String(describing: log.samples.last?.speedMps))")
            ok = ok && lastSpeedOk

            let raw = String(data: data, encoding: .utf8) ?? ""
            let snakeCaseOk = raw.contains("\"format\":") && raw.contains("\"start_soc_pct\"")
            report("snake-case-keys", snakeCaseOk)
            ok = ok && snakeCaseOk
        } catch {
            report("decode-saved-log", false, "\(error)")
            ok = false
        }

        tripStore.refreshLogs()
        let listedOk = tripStore.logs.contains(url)
        report("logs-listed", listedOk, "count=\(tripStore.logs.count)")
        ok = ok && listedOk

        do {
            try TripLogStorage.delete(url: url)
        } catch {
            report("delete", false, "\(error)")
            ok = false
        }
        tripStore.refreshLogs()
        let deletedOk = tripStore.logs.isEmpty && !FileManager.default.fileExists(atPath: url.path)
        report("logs-empty-after-delete", deletedOk, "count=\(tripStore.logs.count)")
        ok = ok && deletedOk

        // -- SEC-010: delete-all-logs -- two minimal synthetic logs saved directly via
        // TripLogStorage.save (capture itself is exercised above), then
        // tripStore.deleteAllLogs() removes both the files and the in-memory list.
        let syntheticNow = Int(Date().timeIntervalSince1970)
        let syntheticLogA = TripLog(
            format: "tlog-1", id: UUID().uuidString, vehicle: "ioniq5_lr_2wd",
            startUnix: syntheticNow - 200, endUnix: syntheticNow - 100,
            startSocPct: 80, endSocPct: 75, ambientTempC: nil, samples: []
        )
        let syntheticLogB = TripLog(
            format: "tlog-1", id: UUID().uuidString, vehicle: "ioniq5_lr_2wd",
            startUnix: syntheticNow - 100, endUnix: syntheticNow,
            startSocPct: 75, endSocPct: 70, ambientTempC: nil, samples: []
        )
        do {
            try TripLogStorage.save(syntheticLogA)
            try TripLogStorage.save(syntheticLogB)
        } catch {
            report("delete-all-logs", false, "failed to save synthetic logs: \(error)")
            ok = false
        }
        tripStore.refreshLogs()
        let seededOk = tripStore.logs.count == 2
        report("delete-all-logs-seeded", seededOk, "count=\(tripStore.logs.count)")
        ok = ok && seededOk

        tripStore.deleteAllLogs()
        let allLogsDeletedOk = tripStore.logs.isEmpty && TripLogStorage.list().isEmpty
        report("delete-all-logs", allLogsDeletedOk, "count=\(tripStore.logs.count)")
        ok = ok && allLogsDeletedOk

        // -- SEC-010: clear-recents -- seeds UserDefaults with one valid RecentDestination,
        // then RecentDestination.clearAll() removes the key outright.
        let recentDestination = RecentDestination(name: "Antwerp", lat: 51.2194, lon: 4.4025)
        if let encoded = try? JSONEncoder().encode([recentDestination]), let json = String(data: encoded, encoding: .utf8) {
            UserDefaults.standard.set(json, forKey: "recentDestinations")
        }
        RecentDestination.clearAll()
        let recentsClearedOk = UserDefaults.standard.object(forKey: "recentDestinations") == nil
        report("clear-recents", recentsClearedOk)
        ok = ok && recentsClearedOk

        await finish(ok: ok)
    }

    // MARK: calibrate-smoke

    /// Drives PlanStore's refit-and-accept flow (wayfinder #53) directly -- no UI-event
    /// injection exists on the sim, so the two Trip Logs are synthesized straight via
    /// `TripLogStorage.save` rather than captured (that path is triplog-smoke's job). Proves:
    /// the empty-log-set no-op, a computed result whose trip rows and refit land in the
    /// expected bands, `acceptCalibration()` setting + persisting + replanning the reference
    /// override and marking the proposal dismissed, that the dismissal sticks across a
    /// same-log-set refresh but resets on a genuine log-set change, and that a corrupt log file
    /// fails the whole call loudly instead of being silently skipped.
    @MainActor
    private static func runCalibrateSmoke(store: PlanStore, tripStore: TripLogStore) async {
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.load()

        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        var ok = true

        // Clean slate: no logs, no persisted override.
        TripLogStorage.list().forEach { try? TripLogStorage.delete(url: $0) }
        UserDefaults.standard.removeObject(forKey: "referenceConsumptionWhPerKm")
        store.referenceConsumptionWhPerKm = nil

        // Empty-set: refreshCalibration over no logs clears (and doesn't compute) anything.
        store.refreshCalibration(logURLs: [])
        let emptyOk = store.calibrationResult == nil && store.calibrationErrorMessage == nil
        report("empty-logs-no-result", emptyOk)
        ok = ok && emptyOk

        // Synthesize a qualifying ~130km trip and a non-qualifying ~3km trip -- a straight
        // line at 27.8 m/s (1 Hz, 0.00025 deg lat per sample) so distance and speed agree.
        let now = Int(Date().timeIntervalSince1970)
        func straightLineSamples(count: Int) -> [TripSample] {
            (0..<count).map { i in
                TripSample(t: Double(i), lat: 49.0 + 0.00025 * Double(i), lon: 6.0, speedMps: 27.8, altM: 300, haccM: 5)
            }
        }
        let longId = UUID().uuidString
        let longLog = TripLog(
            format: "tlog-1", id: longId, vehicle: "ioniq5_lr_2wd",
            startUnix: now - 5000, endUnix: now,
            startSocPct: 90, endSocPct: 55, ambientTempC: 15.0, samples: straightLineSamples(count: 4680)
        )
        let shortId = UUID().uuidString
        let shortStartUnix = now - 6000
        let shortLog = TripLog(
            format: "tlog-1", id: shortId, vehicle: "ioniq5_lr_2wd",
            startUnix: shortStartUnix, endUnix: shortStartUnix + 120,
            startSocPct: 90, endSocPct: 89, ambientTempC: 15.0, samples: straightLineSamples(count: 120)
        )
        do {
            try TripLogStorage.save(longLog)
            try TripLogStorage.save(shortLog)
        } catch {
            report("calibration-computed", false, "failed to save synthetic logs: \(error)")
            await finish(ok: false)
        }

        tripStore.refreshLogs()
        guard tripStore.logs.count == 2 else {
            report("calibration-computed", false, "expected 2 saved logs, got \(tripStore.logs.count)")
            await finish(ok: false)
        }

        store.refreshCalibration(logURLs: tripStore.logs)
        let computed = await waitWithTimeout(seconds: 30) { store.calibrationResult != nil }
        report(
            "calibration-computed", computed,
            computed
                ? "medianRatio=\(store.calibrationResult?.medianRatio ?? -1) "
                    + "refit=\(store.calibrationResult?.referenceConsumptionWhPerKm ?? -1)"
                : "calibrationErrorMessage=\(store.calibrationErrorMessage ?? "none")"
        )
        ok = ok && computed
        guard let result = store.calibrationResult else { await finish(ok: false) }

        // Per-trip rows: the long trip qualifies, the short one is used but too short.
        let longFit = result.trips.first { $0.id == longId }
        let shortFit = result.trips.first { $0.id == shortId }
        let longOk = longFit?.used == true && longFit?.qualifying == true
            && (100_000.0...160_000.0).contains(longFit?.distanceM ?? -1)
        let shortOk = shortFit?.used == true && shortFit?.qualifying == false
        let tripRowsOk = result.trips.count == 2 && longOk && shortOk
        report(
            "trip-rows", tripRowsOk,
            "long used=\(String(describing: longFit?.used)) qualifying=\(String(describing: longFit?.qualifying)) "
                + "distanceM=\(String(describing: longFit?.distanceM)); "
                + "short used=\(String(describing: shortFit?.used)) qualifying=\(String(describing: shortFit?.qualifying))"
        )
        ok = ok && tripRowsOk

        let medianOk = (0.5...2.0).contains(result.medianRatio)
        let refitOk = (100.0...400.0).contains(result.referenceConsumptionWhPerKm)
        report(
            "median-band", medianOk && refitOk,
            "medianRatio=\(result.medianRatio) refit=\(result.referenceConsumptionWhPerKm)"
        )
        ok = ok && medianOk && refitOk

        report("accepted", result.accepted, "accepted=\(result.accepted)")
        ok = ok && result.accepted

        // Proposal + accept path: a plan must be in flight for planVersion to bump on accept.
        store.setDestination(name: "Antwerp", coordinate: CLLocationCoordinate2D(latitude: 51.2194, longitude: 4.4025))
        let destinationPlanLanded = await waitWithTimeout(seconds: 30) { store.planVersion >= 1 }
        guard destinationPlanLanded else {
            report("accept-replans", false, "Antwerp plan never landed before accept")
            await finish(ok: false)
        }

        let versionBeforeAccept = store.planVersion
        let clampedRefit = min(max(result.referenceConsumptionWhPerKm, 120), 260)
        store.acceptCalibration()

        let referenceSetOk = store.referenceConsumptionWhPerKm == clampedRefit
        report(
            "accept-sets-reference", referenceSetOk,
            "expected \(clampedRefit), got \(String(describing: store.referenceConsumptionWhPerKm))"
        )
        ok = ok && referenceSetOk

        let persistedValue = UserDefaults.standard.object(forKey: "referenceConsumptionWhPerKm") as? Double
        let persistedOk = persistedValue == clampedRefit
        report("accept-persists", persistedOk, "expected \(clampedRefit), got \(String(describing: persistedValue))")
        ok = ok && persistedOk

        let replanLanded = await waitWithTimeout(seconds: 30) { store.planVersion > versionBeforeAccept }
        let acceptReplansOk = replanLanded && store.calibrationDismissed
        report(
            "accept-replans", acceptReplansOk,
            "planVersion=\(store.planVersion) (was \(versionBeforeAccept)) calibrationDismissed=\(store.calibrationDismissed)"
        )
        ok = ok && acceptReplansOk

        // Dismiss-sticky: refreshing over the SAME log set doesn't reset the dismissal --
        // the reset check in refreshCalibration runs synchronously, before its async fetch.
        store.refreshCalibration(logURLs: tripStore.logs)
        let dismissStickyOk = store.calibrationDismissed
        report("dismiss-sticky-same-logs", dismissStickyOk, "calibrationDismissed=\(store.calibrationDismissed)")
        ok = ok && dismissStickyOk

        // Dismiss-resets: a genuinely different log set (one log deleted) resets it.
        if let urlToDelete = tripStore.logs.last {
            try? TripLogStorage.delete(url: urlToDelete)
        }
        tripStore.refreshLogs()
        store.refreshCalibration(logURLs: tripStore.logs)
        let dismissResetOk = !store.calibrationDismissed
        report("dismiss-resets-on-log-change", dismissResetOk, "calibrationDismissed=\(store.calibrationDismissed)")
        ok = ok && dismissResetOk

        // Error path: a corrupt log file fails the whole call loudly.
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let tripLogsDir = docs.appendingPathComponent("trip-logs", isDirectory: true)
        try? FileManager.default.createDirectory(at: tripLogsDir, withIntermediateDirectories: true)
        try? Data("not json".utf8).write(to: tripLogsDir.appendingPathComponent("tlog-zzz-corrupt.json"))
        tripStore.refreshLogs()
        store.refreshCalibration(logURLs: tripStore.logs)
        let corruptFailedOk = await waitWithTimeout(seconds: 30) { store.calibrationErrorMessage != nil }
        report(
            "corrupt-log-fails-loudly", corruptFailedOk,
            "calibrationErrorMessage=\(store.calibrationErrorMessage ?? "none")"
        )
        ok = ok && corruptFailedOk

        // Cleanup.
        TripLogStorage.list().forEach { try? TripLogStorage.delete(url: $0) }
        UserDefaults.standard.removeObject(forKey: "referenceConsumptionWhPerKm")
        store.referenceConsumptionWhPerKm = nil

        await finish(ok: ok)
    }

    /// Independent linear interpolation of `curve` at `distanceM`, used only to cross-check
    /// `DriveHud.socAtPosition` -- deliberately not calling DriveStore's own (private)
    /// implementation, so this actually proves the HUD's math rather than just its plumbing.
    private static func manualInterpolatedSoc(_ curve: [FfiSocPoint], at distanceM: Double) -> Double {
        guard let first = curve.first, let last = curve.last else { return 0 }
        if distanceM <= first.distM { return first.soc }
        if distanceM >= last.distM { return last.soc }
        for i in 1..<curve.count {
            let previous = curve[i - 1]
            let current = curve[i]
            if distanceM <= current.distM {
                let span = current.distM - previous.distM
                guard span > 0 else { continue }
                let t = (distanceM - previous.distM) / span
                return previous.soc + t * (current.soc - previous.soc)
            }
        }
        return last.soc
    }

    // MARK: drive-smoke

    /// Drives DriveStore directly (wayfinder #59) -- no CoreLocation fix injection exists on
    /// the sim, so the origin fix goes through PlanStore's real delegate method with a fresh
    /// CLLocationManager (it only reads locations off the array), and every drive fix goes
    /// straight to DriveStore.ingest. See this file's header comment for the full sequence.
    @MainActor
    private static func runDriveSmoke(store: PlanStore, tripStore: TripLogStore, driveStore: DriveStore) async {
        // Deterministic Trip Log capture (same idiom as triplog-smoke): a stubbed authorization
        // status and temperature fetch so step 13's (#62) capture never depends on the
        // simulator's real permission state or the network.
        tripStore.authorizationStatus = { .authorizedWhenInUse }
        tripStore.fetchTemperature = { _, _, _ in 15.0 }
        // So this smoke can delete only the log IT saves at the end, leaving any pre-existing
        // ones (e.g. from a prior triplog-smoke run) untouched.
        let preexistingLogs = Set(TripLogStorage.list())

        store.load()
        let ready = await waitWithTimeout(seconds: 120) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false) }

        var ok = true

        report("gate-no-plan", driveStore.canGo == false, "canGo=\(driveStore.canGo)")
        ok = ok && driveStore.canGo == false

        // Step 3: adopt an origin fix through PlanStore's real delegate path.
        store.locationManager(
            CLLocationManager(), didUpdateLocations: [CLLocation(latitude: 49.6116, longitude: 6.1319)]
        )
        report("origin-adopted", store.originIsCurrentLocation, "originIsCurrentLocation=\(store.originIsCurrentLocation)")
        ok = ok && store.originIsCurrentLocation

        // Step 4.
        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLanded = await waitWithTimeout(seconds: 30) { store.plan != nil }
        report("gate-ready", planLanded && driveStore.canGo, "planLanded=\(planLanded) canGo=\(driveStore.canGo)")
        ok = ok && planLanded && driveStore.canGo
        guard planLanded else { await finish(ok: false) }

        // Step 5 (wayfinder #62): Go now opens the start-SoC prompt instead of entering
        // directly.
        driveStore.go()
        let goPromptsOk = driveStore.phase == .idle && driveStore.pendingGo && tripStore.phase == .promptingStartSoc
        report(
            "go-prompts", goPromptsOk,
            "phase=\(driveStore.phase) pendingGo=\(driveStore.pendingGo) tripPhase=\(tripStore.phase)"
        )
        ok = ok && goPromptsOk

        // Cancelling that prompt cancels the Go outright -- no half-open capture.
        tripStore.cancelStartSoc()
        driveStore.resolvePendingGo()
        let goCancelledOk = driveStore.phase == .idle && !driveStore.pendingGo && tripStore.phase == .idle
        report(
            "go-cancelled", goCancelledOk,
            "phase=\(driveStore.phase) pendingGo=\(driveStore.pendingGo) tripPhase=\(tripStore.phase)"
        )
        ok = ok && goCancelledOk

        // Confirming it both enters the drive AND starts capture.
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        let enterOk = driveStore.phase == .driving && driveStore.cameraMode == .following
            && !store.mapView.showsUserLocation && tripStore.phase == .recording
        report(
            "enter", enterOk,
            "phase=\(driveStore.phase) cameraMode=\(driveStore.cameraMode) "
                + "showsUserLocation=\(store.mapView.showsUserLocation) tripPhase=\(tripStore.phase)"
        )
        ok = ok && enterOk

        guard let polyline = store.displayedPlan?.polyline, polyline.count >= 102 else {
            report("snap-on-route", false, "polyline too short for this smoke's fixed indices")
            await finish(ok: false)
        }

        // Step 6: three exact-vertex fixes -- on-route snap + strictly-increasing progress.
        let vertexIndices = [0, min(50, polyline.count - 1), min(100, polyline.count - 1)]
        var onRouteOks: [Bool] = []
        var snapCloseOks: [Bool] = []
        var progressValues: [Double] = []
        for idx in vertexIndices {
            let vertex = polyline[idx]
            let fixCoordinate = CLLocationCoordinate2D(latitude: vertex.lat, longitude: vertex.lon)
            let fix = CLLocation(
                coordinate: fixCoordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
                course: -1, speed: -1, timestamp: Date()
            )
            driveStore.ingest(fix)
            onRouteOks.append(driveStore.isOnRoute)
            if let snapped = driveStore.snappedCoordinate {
                let snapDistM = CLLocation(latitude: snapped.latitude, longitude: snapped.longitude)
                    .distance(from: CLLocation(latitude: fixCoordinate.latitude, longitude: fixCoordinate.longitude))
                snapCloseOks.append(snapDistM <= 1)
            } else {
                snapCloseOks.append(false)
            }
            progressValues.append(driveStore.distanceAlongRouteM)
        }
        let snapOnRouteOk = onRouteOks.allSatisfy { $0 } && snapCloseOks.allSatisfy { $0 }
        report("snap-on-route", snapOnRouteOk, "onRoute=\(onRouteOks) snapClose=\(snapCloseOks)")
        ok = ok && snapOnRouteOk

        let progressMonotonicOk = zip(progressValues, progressValues.dropFirst()).allSatisfy { $0 < $1 }
        report("progress-monotonic", progressMonotonicOk, "progress=\(progressValues)")
        ok = ok && progressMonotonicOk

        // Step 7: a small (~6 m) perpendicular offset from segment [~50]'s midpoint should
        // still snap on-route, pulled in 3-10 m from the raw fix.
        let segIdx = min(50, polyline.count - 2)
        let a = polyline[segIdx]
        let b = polyline[segIdx + 1]
        let midLat = (a.lat + b.lat) / 2
        let midLon = (a.lon + b.lon) / 2
        let smallOffsetCoordinate = CLLocationCoordinate2D(latitude: midLat + 0.000054, longitude: midLon)
        driveStore.ingest(CLLocation(
            coordinate: smallOffsetCoordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: -1, speed: -1, timestamp: Date()
        ))
        let smallOffsetSnapped = driveStore.snappedCoordinate
        let smallOffsetDistM = smallOffsetSnapped.map {
            CLLocation(latitude: $0.latitude, longitude: $0.longitude)
                .distance(from: CLLocation(latitude: smallOffsetCoordinate.latitude, longitude: smallOffsetCoordinate.longitude))
        } ?? -1
        let snapPullsInOk = driveStore.isOnRoute
            && smallOffsetSnapped.map { $0.latitude != smallOffsetCoordinate.latitude || $0.longitude != smallOffsetCoordinate.longitude } == true
            && (3.0...10.0).contains(smallOffsetDistM)
        report(
            "snap-pulls-in", snapPullsInOk,
            "isOnRoute=\(driveStore.isOnRoute) distFromFixM=\(smallOffsetDistM)"
        )
        ok = ok && snapPullsInOk

        // Step 8: a far (~300 m) offset from the same midpoint should stay raw, off-route.
        let farOffsetCoordinate = CLLocationCoordinate2D(latitude: midLat + 0.002695, longitude: midLon)
        driveStore.ingest(CLLocation(
            coordinate: farOffsetCoordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: -1, speed: -1, timestamp: Date()
        ))
        let farOffsetSnapped = driveStore.snappedCoordinate
        let farOffsetDistM = farOffsetSnapped.map {
            CLLocation(latitude: $0.latitude, longitude: $0.longitude)
                .distance(from: CLLocation(latitude: farOffsetCoordinate.latitude, longitude: farOffsetCoordinate.longitude))
        } ?? -1
        let rawBeyondSnapOk = !driveStore.isOnRoute && farOffsetDistM <= 1
        report("raw-beyond-snap", rawBeyondSnapOk, "isOnRoute=\(driveStore.isOnRoute) distFromFixM=\(farOffsetDistM)")
        ok = ok && rawBeyondSnapOk

        // Step 9: capped course smoothing -- course 0 then 180 at the same spot, capped at
        // +/-45 deg per fix.
        let courseFixCoordinate = CLLocationCoordinate2D(latitude: a.lat, longitude: a.lon)
        driveStore.ingest(CLLocation(
            coordinate: courseFixCoordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: 0, speed: 10, timestamp: Date()
        ))
        let courseBefore = driveStore.smoothedCourseDeg
        driveStore.ingest(CLLocation(
            coordinate: courseFixCoordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: 180, speed: 10, timestamp: Date()
        ))
        let courseAfter = driveStore.smoothedCourseDeg
        var arcDelta = (courseAfter - courseBefore).truncatingRemainder(dividingBy: 360)
        if arcDelta > 180 { arcDelta -= 360 }
        if arcDelta < -180 { arcDelta += 360 }
        let courseCappedOk = abs(arcDelta) <= 45 + 0.001
        report("course-capped", courseCappedOk, "before=\(courseBefore) after=\(courseAfter) delta=\(arcDelta)")
        ok = ok && courseCappedOk

        // Step 10: the four camera-mode transitions.
        driveStore.noteUserGesture()
        let freeLookOk = driveStore.cameraMode == .freeLook
        driveStore.recenter()
        let recenterOk = driveStore.cameraMode == .following
        driveStore.toggleOverview()
        let overviewOk = driveStore.cameraMode == .overview
        driveStore.toggleOverview()
        let backToFollowingOk = driveStore.cameraMode == .following
        let cameraModesOk = freeLookOk && recenterOk && overviewOk && backToFollowingOk
        report(
            "camera-modes", cameraModesOk,
            "freeLook=\(freeLookOk) recenter=\(recenterOk) overview=\(overviewOk) backToFollowing=\(backToFollowingOk)"
        )
        ok = ok && cameraModesOk

        // Step 11 (wayfinder #60): steps 6-10 above fed synthetic fixes all over the route
        // (small/far offsets, a fixed course-test coordinate) to exercise snap/course math, so
        // `distanceAlongRouteM` and the checked-stop/leg counters no longer reflect a
        // from-scratch drive. End and re-enter (canGo already proven true after "gate-ready")
        // so the HUD/stepper/arrival continuation below starts clean from distance 0 -- values
        // re-derived from the plan object itself, not hardcoded stop names, since the active
        // region on the sim may be corridor or eu-west and their goldens differ in stop sets.
        // (wayfinder #62: end() now flips tripStore to .promptingEndSoc, and go() re-opens the
        // start-SoC prompt -- both resolved inline here to keep this re-entry a single step.)
        driveStore.end()
        tripStore.confirmEndSoc(70)
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        guard let plan = store.displayedPlan else {
            report("hud-initial", false, "no displayed plan after re-entering drive")
            await finish(ok: false)
        }

        // "hud-initial": go()'s unthrottled initial computation.
        let initialHud = driveStore.hud
        let initialRemainingDistOk = initialHud.map { isClose($0.remainingDistM, plan.totalDistM, tol: 1) } ?? false
        let initialRemainingTimeOk = initialHud.map {
            isClose($0.remainingTimeS, plan.driveTimeS + plan.chargeTimeS, tol: 60)
        } ?? false
        let initialNextStopOk = plan.stops.isEmpty || initialHud?.nextLabel == plan.stops.first?.name
        let hudInitialOk = initialHud != nil && initialRemainingDistOk && initialRemainingTimeOk && initialNextStopOk
        report(
            "hud-initial", hudInitialOk,
            "hud=\(String(describing: initialHud)) totalDistM=\(plan.totalDistM) "
                + "driveTimeS+chargeTimeS=\(plan.driveTimeS + plan.chargeTimeS)"
        )
        ok = ok && hudInitialOk

        // Walk the polyline's cumulative distance to find vertices at/after given distances,
        // for the throttle and stop-check-off steps below.
        let driveCumulativeM = RouteSnap.cumulativeDistances(
            plan.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        )
        func firstVertexIndex(atOrAfterM targetM: Double) -> Int? {
            driveCumulativeM.firstIndex { $0 >= targetM }
        }

        // wayfinder #67: computed once, reused by every "banner-*" assert below. Empty on a v1
        // pack (every leg's `steps` is empty) -- each assert below degrades to "banner stays
        // nil" in that case and reports it as such.
        let expectedGuidance = StepTracker.guidanceSteps(
            legs: plan.legs, stops: ChargingStopVM.stops(from: plan),
            polyline: plan.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) },
            cumulativeM: driveCumulativeM
        )
        let stepsPresent = !expectedGuidance.isEmpty

        // "banner-initial": go()'s snapshotPlan already computed the banner at distance 0, same
        // atomic swap point as "hud-initial" above.
        let firstUpcomingIdx = StepTracker.upcomingIndex(steps: expectedGuidance, distanceAlongRouteM: 0)
        let bannerInitialOk: Bool
        if stepsPresent, let idx = firstUpcomingIdx {
            let expectedStep = expectedGuidance[idx]
            bannerInitialOk = driveStore.banner != nil && driveStore.banner?.primary == expectedStep.primary
                && isClose(driveStore.banner?.distanceM ?? -1, expectedStep.distAlongRouteM, tol: 1)
        } else {
            bannerInitialOk = driveStore.banner == nil
        }
        report(
            "banner-initial", bannerInitialOk,
            stepsPresent ? "banner=\(String(describing: driveStore.banner))" : "v1 pack, banner hidden"
        )
        ok = ok && bannerInitialOk

        let base = Date()
        func fix(atVertex idx: Int, at time: Date) -> CLLocation {
            let point = plan.polyline[idx]
            return CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: point.lat, longitude: point.lon),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 15, timestamp: time
            )
        }

        // Step: throttle. Fix A ~2 km in at T, fix B one vertex later at T+0.2s (hud unchanged),
        // fix C another vertex later at T+1.5s (hud updated).
        guard let vertexA = firstVertexIndex(atOrAfterM: 2000), vertexA + 2 < plan.polyline.count else {
            report("hud-throttled", false, "polyline too short for the throttle step")
            report("hud-updates", false, "polyline too short for the throttle step")
            await finish(ok: false)
        }
        driveStore.ingest(fix(atVertex: vertexA, at: base))
        let hudAfterA = driveStore.hud

        driveStore.ingest(fix(atVertex: vertexA + 1, at: base.addingTimeInterval(0.2)))
        let hudAfterB = driveStore.hud
        let throttledOk = hudAfterA == hudAfterB
        report("hud-throttled", throttledOk, "hudAfterA=\(String(describing: hudAfterA)) hudAfterB=\(String(describing: hudAfterB))")
        ok = ok && throttledOk

        driveStore.ingest(fix(atVertex: vertexA + 2, at: base.addingTimeInterval(1.5)))
        let hudAfterC = driveStore.hud
        let expectedSocAtC = manualInterpolatedSoc(plan.socCurve, at: driveStore.distanceAlongRouteM)
        let socCloseOk = hudAfterC.map { isClose($0.socAtPosition, expectedSocAtC, tol: 0.005) } ?? false
        let distDecreasedOk = (hudAfterC?.remainingDistM ?? .infinity) < (hudAfterB?.remainingDistM ?? -.infinity)
        let updatesOk = hudAfterC != hudAfterB && distDecreasedOk && socCloseOk
        report(
            "hud-updates", updatesOk,
            "hudAfterB=\(String(describing: hudAfterB)) hudAfterC=\(String(describing: hudAfterC)) "
                + "expectedSoc=\(expectedSocAtC)"
        )
        ok = ok && updatesOk

        // "banner-countdown"/"banner-advance" (wayfinder #67): pick the first guidance step past
        // 2500 m in (falling back to the last one) as an anchor, approach it with two fixes, then
        // pass it with a third.
        let anchorIndex: Int? = {
            if let k = expectedGuidance.firstIndex(where: { $0.distAlongRouteM > 2500 }) { return k }
            return expectedGuidance.isEmpty ? nil : expectedGuidance.count - 1
        }()
        let anchorM = anchorIndex.map { expectedGuidance[$0].distAlongRouteM } ?? 2500.0

        if let vertexNear1 = firstVertexIndex(atOrAfterM: max(0, anchorM - 1200)),
           let vertexNear2 = firstVertexIndex(atOrAfterM: max(0, anchorM - 400)) {
            driveStore.ingest(fix(atVertex: vertexNear1, at: base.addingTimeInterval(3)))
            let bannerNear1 = driveStore.banner
            let distAlongNear1 = driveStore.distanceAlongRouteM

            driveStore.ingest(fix(atVertex: vertexNear2, at: base.addingTimeInterval(4.5)))
            let bannerNear2 = driveStore.banner
            let distAlongNear2 = driveStore.distanceAlongRouteM

            let countdownOk: Bool
            if stepsPresent {
                countdownOk = bannerNear1 != nil && bannerNear2 != nil
                    && bannerNear2!.distanceM < bannerNear1!.distanceM
                    && isClose(bannerNear1!.distanceM, anchorM - distAlongNear1, tol: 1)
                    && isClose(bannerNear2!.distanceM, anchorM - distAlongNear2, tol: 1)
            } else {
                countdownOk = bannerNear1 == nil && bannerNear2 == nil
            }
            report(
                "banner-countdown", countdownOk,
                stepsPresent
                    ? "bannerNear1=\(String(describing: bannerNear1)) bannerNear2=\(String(describing: bannerNear2))"
                    : "v1 pack, banner hidden"
            )
            ok = ok && countdownOk
        } else {
            report("banner-countdown", false, "polyline too short for banner-countdown's vertices")
            ok = false
        }

        if let vertexPastAnchor = firstVertexIndex(atOrAfterM: anchorM + StepTracker.passedBufferM + 5) {
            driveStore.ingest(fix(atVertex: vertexPastAnchor, at: base.addingTimeInterval(6)))
            let advanceOk: Bool
            if stepsPresent, let k = anchorIndex {
                advanceOk = k + 1 < expectedGuidance.count
                    ? driveStore.banner?.primary == expectedGuidance[k + 1].primary
                    : driveStore.banner == nil
            } else {
                advanceOk = driveStore.banner == nil
            }
            report(
                "banner-advance", advanceOk,
                stepsPresent ? "banner=\(String(describing: driveStore.banner))" : "v1 pack, banner hidden"
            )
            ok = ok && advanceOk
        } else {
            report("banner-advance", false, "polyline too short for banner-advance's vertex")
            ok = false
        }

        // "banner-landmark" (wayfinder #67): pins the literal EN template table against a real
        // golden signage string -- the A6 on-ramp's "Toutes Directions" destination, ~3.9 km in
        // on the corridor pack's LU -> Amsterdam plan.
        if autotestRegion == "corridor" {
            if stepsPresent, let landmarkStep = expectedGuidance.first(where: { ($0.secondary ?? "").contains("Toutes Directions") }) {
                let landmarkOk = landmarkStep.primary.hasPrefix("Take the ramp") && landmarkStep.secondary == "toward Toutes Directions"
                report(
                    "banner-landmark", landmarkOk,
                    "primary=\(landmarkStep.primary) secondary=\(String(describing: landmarkStep.secondary))"
                )
                ok = ok && landmarkOk
            } else {
                report("banner-landmark", false, "no onRamp step with 'Toutes Directions' signage found on the corridor plan")
                ok = false
            }
        } else {
            report("banner-landmark", true, "not corridor, landmark skipped")
        }

        // "banner-then": any adjacent pair chained within `thenChainThresholdM` pins both the
        // precomputed field and (when a suitable approach vertex exists) the live banner.
        if let pairIdx = (0..<max(0, expectedGuidance.count - 1))
            .first(where: { expectedGuidance[$0 + 1].distAlongRouteM - expectedGuidance[$0].distAlongRouteM < StepTracker.thenChainThresholdM }) {
            let thenFieldOk = expectedGuidance[pairIdx].then == expectedGuidance[pairIdx + 1].primary
            var liveThenOk = true
            var liveDetail = "no vertex available for the live check"
            if let vertexBeforePair = firstVertexIndex(atOrAfterM: max(0, expectedGuidance[pairIdx].distAlongRouteM - 600)) {
                driveStore.ingest(fix(atVertex: vertexBeforePair, at: base.addingTimeInterval(6.5)))
                let landedOnPair = StepTracker.upcomingIndex(steps: expectedGuidance, distanceAlongRouteM: driveStore.distanceAlongRouteM) == pairIdx
                liveThenOk = !landedOnPair || driveStore.banner?.then == expectedGuidance[pairIdx + 1].primary
                liveDetail = "landedOnPair=\(landedOnPair) banner=\(String(describing: driveStore.banner))"
            }
            let bannerThenOk = thenFieldOk && liveThenOk
            report(
                "banner-then", bannerThenOk,
                "pairIdx=\(pairIdx) thenField=\(String(describing: expectedGuidance[pairIdx].then)) \(liveDetail)"
            )
            ok = ok && bannerThenOk
        } else {
            report("banner-then", true, "no close pair in plan")
        }

        // Step: stop check-off, only meaningful if the plan has stops. (Timestamp shifted to +8,
        // wayfinder #67, to stay strictly after the banner fixes above at +3..+6.5.)
        let stops = ChargingStopVM.stops(from: plan)
        if stops.isEmpty {
            report("stop-checked", true, "no stops in plan")
        } else if let firstStop = stops.first, let stopVertex = firstVertexIndex(atOrAfterM: firstStop.distFromStartM + 100) {
            driveStore.ingest(fix(atVertex: stopVertex, at: base.addingTimeInterval(8)))
            let checkedOk = driveStore.checkedStopCount == 1
            let nextOk = stops.count == 1
                ? driveStore.hud?.nextIsDestination == true
                : driveStore.hud?.nextLabel == stops[1].name
            let legAdvancedOk = driveStore.currentLegIndex >= 1
            let stopCheckedOk = checkedOk && nextOk && legAdvancedOk
            report(
                "stop-checked", stopCheckedOk,
                "checkedStopCount=\(driveStore.checkedStopCount) hud=\(String(describing: driveStore.hud)) "
                    + "currentLegIndex=\(driveStore.currentLegIndex)"
            )
            ok = ok && stopCheckedOk
        } else {
            report("stop-checked", false, "no polyline vertex found past the first stop's distance")
            ok = false
        }

        // Step: arrival -- feed the last polyline vertex. (Timestamp shifted to +10, wayfinder
        // #67, to stay strictly after the banner fixes above at +3..+6.5.)
        driveStore.ingest(fix(atVertex: plan.polyline.count - 1, at: base.addingTimeInterval(10)))
        let arrivalOk = driveStore.phase == .arrived && (driveStore.hud?.remainingDistM ?? .infinity) <= 40
        report(
            "arrival", arrivalOk,
            "phase=\(driveStore.phase) remainingDistM=\(String(describing: driveStore.hud?.remainingDistM))"
        )
        ok = ok && arrivalOk

        // "banner-arrival" (wayfinder #67): arrival clears the banner.
        let bannerArrivalOk = driveStore.banner == nil
        report("banner-arrival", bannerArrivalOk, "banner=\(String(describing: driveStore.banner))")
        ok = ok && bannerArrivalOk

        // Destination arrival also closes capture (wayfinder #62) with the end-SoC prompt.
        let arrivalStopsCaptureOk = tripStore.phase == .promptingEndSoc
        report("arrival-stops-capture", arrivalStopsCaptureOk, "tripPhase=\(tripStore.phase)")
        ok = ok && arrivalStopsCaptureOk

        // Step: end from arrived -- must NOT double-stop capture, already .promptingEndSoc.
        driveStore.end()
        let endOk = driveStore.phase == .idle && store.plan != nil && store.mapView.showsUserLocation
            && tripStore.phase == .promptingEndSoc
        report(
            "end", endOk,
            "phase=\(driveStore.phase) plan=\(store.plan != nil) showsUserLocation=\(store.mapView.showsUserLocation) "
                + "tripPhase=\(tripStore.phase)"
        )
        ok = ok && endOk
        tripStore.confirmEndSoc(60)

        // Step 12 (wayfinder #61): off-route detection + silent replan-from-position. Re-enter
        // drive (origin still adopted, plan still displayed from the steps above -- canGo
        // holds) and build a truly perpendicular 60 m offset off the segment at ~2 km along, in
        // the same local equirectangular plane RouteSnap.snap projects into.
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        guard let plan61 = store.displayedPlan else {
            report("offroute-noise", false, "no displayed plan after re-entering drive for step 12")
            await finish(ok: false)
        }
        let cumulative61 = RouteSnap.cumulativeDistances(
            plan61.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        )
        guard let vertexIdx61 = cumulative61.firstIndex(where: { $0 >= 2000 }), vertexIdx61 + 1 < plan61.polyline.count else {
            report("offroute-noise", false, "polyline too short for step 12's offset")
            await finish(ok: false)
        }
        let a61 = plan61.polyline[vertexIdx61]
        let b61 = plan61.polyline[vertexIdx61 + 1]
        let midLat61 = (a61.lat + b61.lat) / 2
        let midLon61 = (a61.lon + b61.lon) / 2
        let cosMidLat61 = cos(midLat61 * .pi / 180)
        let vx61 = b61.lon * cosMidLat61 - a61.lon * cosMidLat61
        let vy61 = b61.lat - a61.lat
        let vLen61 = (vx61 * vx61 + vy61 * vy61).squareRoot()
        let nx61 = -vy61 / vLen61
        let ny61 = vx61 / vLen61
        let offsetCoordinate61 = CLLocationCoordinate2D(
            latitude: midLat61 + (60.0 / 111_320.0) * ny61,
            longitude: midLon61 + (60.0 / 111_320.0) * nx61 / cosMidLat61
        )
        let midCoordinate61 = CLLocationCoordinate2D(latitude: midLat61, longitude: midLon61)
        let base61 = Date()
        func offsetFix61(at time: Date) -> CLLocation {
            CLLocation(
                coordinate: offsetCoordinate61, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
                course: -1, speed: 15, timestamp: time
            )
        }

        // "offroute-noise": a single GPS-noise excursion, back on-route before the 5 s sustain
        // bar, must not fire a replan.
        let planVersionBefore61 = store.planVersion
        driveStore.ingest(offsetFix61(at: base61))
        driveStore.ingest(CLLocation(
            coordinate: midCoordinate61, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: -1, speed: 15, timestamp: base61.addingTimeInterval(1)
        ))
        let noiseOk = driveStore.routeUpdatedVersion == 0 && store.planVersion == planVersionBefore61
            && driveStore.phase == .driving
        report(
            "offroute-noise", noiseOk,
            "routeUpdatedVersion=\(driveStore.routeUpdatedVersion) planVersion=\(store.planVersion) phase=\(driveStore.phase)"
        )
        ok = ok && noiseOk

        // "offroute-replan": the SAME offset sustained across the 5 s bar (window opens at
        // +2s, +7.5s crosses it) must fire exactly one silent replan.
        driveStore.ingest(offsetFix61(at: base61.addingTimeInterval(2)))
        driveStore.ingest(offsetFix61(at: base61.addingTimeInterval(4)))
        driveStore.ingest(offsetFix61(at: base61.addingTimeInterval(6)))
        driveStore.ingest(offsetFix61(at: base61.addingTimeInterval(7.5)))

        // "banner-muted" (wayfinder #67): guidance mutes during a sustained off-route excursion
        // in progress -- holds trivially on a v1 pack too, since the banner is already nil there.
        let bannerMutedOk = driveStore.banner == nil
        report("banner-muted", bannerMutedOk, "banner=\(String(describing: driveStore.banner))")
        ok = ok && bannerMutedOk

        let expectedSoc61 = manualInterpolatedSoc(plan61.socCurve, at: driveStore.distanceAlongRouteM)

        let replanLanded61 = await waitWithTimeout(seconds: 30) { driveStore.routeUpdatedVersion == 1 }
        let replanOk = replanLanded61 && store.planVersion > planVersionBefore61 && driveStore.phase == .driving
        report(
            "offroute-replan", replanOk,
            "routeUpdatedVersion=\(driveStore.routeUpdatedVersion) planVersion=\(store.planVersion) "
                + "planVersionBefore=\(planVersionBefore61) phase=\(driveStore.phase)"
        )
        ok = ok && replanOk

        // "offroute-origin-soc": the replan departs from the deviated position at the model's
        // own predicted SoC, not the settings slider.
        guard let newPlan61 = store.displayedPlan else {
            report("offroute-origin-soc", false, "no displayed plan after off-route replan")
            await finish(ok: false)
        }
        let newOriginDistM61 = newPlan61.polyline.first.map {
            CLLocation(latitude: $0.lat, longitude: $0.lon)
                .distance(from: CLLocation(latitude: offsetCoordinate61.latitude, longitude: offsetCoordinate61.longitude))
        } ?? .infinity
        let originSocOk = newOriginDistM61 <= 300
            && (newPlan61.socCurve.first.map { isClose($0.soc, expectedSoc61, tol: 0.02) } ?? false)
        report(
            "offroute-origin-soc", originSocOk,
            "newOriginDistM=\(newOriginDistM61) newSoc=\(String(describing: newPlan61.socCurve.first?.soc)) "
                + "expectedSoc=\(expectedSoc61)"
        )
        ok = ok && originSocOk

        // "offroute-survives": the drive itself keeps running through the swap (HUD present,
        // stepper reset for the new geometry), and the snap engine runs against the NEW
        // polyline for the very next fix.
        let survivesInitialOk = driveStore.hud != nil && driveStore.checkedStopCount == 0 && driveStore.distanceAlongRouteM == 0
        let newGeometryVertex61 = newPlan61.polyline[min(10, newPlan61.polyline.count - 1)]
        driveStore.ingest(CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: newGeometryVertex61.lat, longitude: newGeometryVertex61.lon),
            altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 15,
            timestamp: base61.addingTimeInterval(9)
        ))
        let survivesOk = survivesInitialOk && driveStore.isOnRoute && driveStore.distanceAlongRouteM > 0
        report(
            "offroute-survives", survivesOk,
            "hud=\(driveStore.hud != nil) checkedStopCount=\(driveStore.checkedStopCount) "
                + "isOnRoute=\(driveStore.isOnRoute) distanceAlongRouteM=\(driveStore.distanceAlongRouteM)"
        )
        ok = ok && survivesOk

        // "banner-after-replan" (wayfinder #67): the banner re-derives against the NEW plan's
        // own guidance steps, recomputed independently here rather than reusing `expectedGuidance`
        // (that one is still keyed off the pre-replan `plan`).
        let newPolyline61 = newPlan61.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        let newExpectedGuidance61 = StepTracker.guidanceSteps(
            legs: newPlan61.legs, stops: ChargingStopVM.stops(from: newPlan61),
            polyline: newPolyline61, cumulativeM: RouteSnap.cumulativeDistances(newPolyline61)
        )
        let newStepsPresent61 = !newExpectedGuidance61.isEmpty
        let bannerAfterReplanOk: Bool
        if newStepsPresent61 {
            let newUpcomingIdx61 = StepTracker.upcomingIndex(steps: newExpectedGuidance61, distanceAlongRouteM: driveStore.distanceAlongRouteM)
            bannerAfterReplanOk = driveStore.banner != nil
                && newUpcomingIdx61.map { driveStore.banner?.primary == newExpectedGuidance61[$0].primary } == true
        } else {
            bannerAfterReplanOk = driveStore.banner == nil
        }
        report(
            "banner-after-replan", bannerAfterReplanOk,
            "banner=\(String(describing: driveStore.banner)) newStepsPresent=\(newStepsPresent61)"
        )
        ok = ok && bannerAfterReplanOk

        driveStore.end()
        tripStore.confirmEndSoc(55)

        // Step 13 (wayfinder #62, ADR 0012 point 7): a STANDALONE capture (started via the
        // record button, not Go) is ADOPTED as the drive's capture outright -- one capture,
        // never two. Origin is still current-location here (the off-route replan above
        // re-adopted it), so canGo still holds.
        tripStore.startTapped()
        tripStore.confirmStartSoc(75)
        driveStore.go()
        let goSharesCaptureOk = driveStore.phase == .driving && !driveStore.pendingGo && tripStore.phase == .recording
        report(
            "go-shares-capture", goSharesCaptureOk,
            "phase=\(driveStore.phase) pendingGo=\(driveStore.pendingGo) tripPhase=\(tripStore.phase)"
        )
        ok = ok && goSharesCaptureOk

        guard let captureTripStartDate = tripStore.tripStartDate else {
            report("drive-log-saved", false, "no tripStartDate after adopting the standalone capture")
            await finish(ok: false)
        }
        for secondOffset in 1...3 {
            tripStore.ingest(CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 15,
                timestamp: captureTripStartDate.addingTimeInterval(Double(secondOffset))
            ))
        }

        // End (not arrival) closes the shared capture too, same end-SoC prompt.
        driveStore.end()
        let endPromptsEndSocOk = tripStore.phase == .promptingEndSoc && driveStore.phase == .idle
        report(
            "end-prompts-endsoc", endPromptsEndSocOk,
            "tripPhase=\(tripStore.phase) drivePhase=\(driveStore.phase)"
        )
        ok = ok && endPromptsEndSocOk

        // The drive-closed log is shape-identical to a button-started one because it IS the
        // same producer.
        let savedBefore = tripStore.lastSavedURL
        tripStore.confirmEndSoc(65)
        let driveLogSaved = await waitWithTimeout(seconds: 15) { tripStore.lastSavedURL != savedBefore }
        if driveLogSaved, let url = tripStore.lastSavedURL {
            do {
                let data = try Data(contentsOf: url)
                let log = try JSONDecoder().decode(TripLog.self, from: data)
                let driveLogSavedOk = log.format == "tlog-1" && log.startSocPct == 75 && log.endSocPct == 65
                    && log.samples.count == 3
                report(
                    "drive-log-saved", driveLogSavedOk,
                    "format=\(log.format) start=\(log.startSocPct) end=\(log.endSocPct) samples=\(log.samples.count)"
                )
                ok = ok && driveLogSavedOk
            } catch {
                report("drive-log-saved", false, "\(error)")
                ok = false
            }
        } else {
            report("drive-log-saved", false, "lastSavedURL never changed")
            ok = false
        }

        // Step 14 (wayfinder #63): manual mid-drive dash-SoC correction. Re-enter drive (origin is
        // still current-location -- the drive replans above re-adopt it), land one on-route fix so
        // `snappedCoordinate` is set, then correct to 55%: exactly one replan from the snapped
        // position with the ENTERED SoC (not the curve's) as the departure anchor, and the HUD/curve
        // re-anchored to it.
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        guard let plan63 = store.displayedPlan else {
            report("soc-correct-replan", false, "no displayed plan after re-entering drive for step 14")
            await finish(ok: false)
        }
        let polyline63 = plan63.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        let cumulative63 = RouteSnap.cumulativeDistances(polyline63)
        guard let vertexIdx63 = cumulative63.firstIndex(where: { $0 >= 2000 }) else {
            report("soc-correct-replan", false, "polyline too short for step 14's fix")
            await finish(ok: false)
        }
        driveStore.ingest(CLLocation(
            coordinate: polyline63[vertexIdx63], altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
            course: -1, speed: 15, timestamp: Date()
        ))
        guard let snapped63 = driveStore.snappedCoordinate, driveStore.isOnRoute else {
            report("soc-correct-replan", false, "no on-route snapped position before correcting")
            await finish(ok: false)
        }
        let routeVersionBefore63 = driveStore.routeUpdatedVersion
        let planVersionBefore63 = store.planVersion
        driveStore.correctSoc(55)
        let correctionLanded = await waitWithTimeout(seconds: 30) { driveStore.routeUpdatedVersion == routeVersionBefore63 + 1 }
        let correctReplanOk = correctionLanded && store.planVersion > planVersionBefore63 && driveStore.phase == .driving
        report(
            "soc-correct-replan", correctReplanOk,
            "routeUpdatedVersion=\(driveStore.routeUpdatedVersion) planVersion=\(store.planVersion) phase=\(driveStore.phase)"
        )
        ok = ok && correctReplanOk

        // "soc-correct-anchor": the new plan departs from the snapped position at EXACTLY the
        // entered dash SoC -- the whole point of the ticket; the model's curve estimate loses.
        guard let newPlan63 = store.displayedPlan else {
            report("soc-correct-anchor", false, "no displayed plan after the correction replan")
            await finish(ok: false)
        }
        let newOriginDistM63 = newPlan63.polyline.first.map {
            CLLocation(latitude: $0.lat, longitude: $0.lon)
                .distance(from: CLLocation(latitude: snapped63.latitude, longitude: snapped63.longitude))
        } ?? .infinity
        let anchorOk = newOriginDistM63 <= 300
            && (newPlan63.socCurve.first.map { isClose($0.soc, 0.55, tol: 0.001) } ?? false)
        report(
            "soc-correct-anchor", anchorOk,
            "newOriginDistM=\(newOriginDistM63) newSoc=\(String(describing: newPlan63.socCurve.first?.soc))"
        )
        ok = ok && anchorOk

        // "soc-correct-hud": the displayed values re-anchor -- distanceAlong reset to 0 by the
        // snapshot swap, HUD SoC now reading the corrected curve.
        let hudAnchorOk = driveStore.distanceAlongRouteM == 0
            && (driveStore.hud.map { isClose($0.socAtPosition, 0.55, tol: 0.001) } ?? false)
        report(
            "soc-correct-hud", hudAnchorOk,
            "distanceAlongRouteM=\(driveStore.distanceAlongRouteM) socAtPosition=\(String(describing: driveStore.hud?.socAtPosition))"
        )
        ok = ok && hudAnchorOk

        // "soc-correct-gated": once the drive is over, correctSoc must be a no-op.
        driveStore.end()
        tripStore.confirmEndSoc(60)
        let routeVersionAfterEnd63 = driveStore.routeUpdatedVersion
        driveStore.correctSoc(40)
        try? await Task.sleep(nanoseconds: 500_000_000)
        let gatedOk = driveStore.routeUpdatedVersion == routeVersionAfterEnd63 && driveStore.phase == .idle
        report(
            "soc-correct-gated", gatedOk,
            "routeUpdatedVersion=\(driveStore.routeUpdatedVersion) phase=\(driveStore.phase)"
        )
        ok = ok && gatedOk

        // Step: re-entry guard -- a long-press origin override closes the gate again. Note the
        // off-route replan above reset `originOverridden` to false (it adopts the deviated fix
        // like any other current-location origin), so this override still flips it back to
        // true -- unchanged semantics from before #61.
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.7, longitude: 6.2))
        report("gate-overridden", driveStore.canGo == false, "canGo=\(driveStore.canGo)")
        ok = ok && driveStore.canGo == false

        // Clean up only the log(s) this smoke itself saved -- `preexistingLogs` above.
        for url in TripLogStorage.list() where !preexistingLogs.contains(url) {
            try? TripLogStorage.delete(url: url)
        }

        await finish(ok: ok)
    }

    /// Polls `condition` every 100ms until it's true or `seconds` elapses.
    @MainActor
    private static func waitWithTimeout(seconds: Double, until condition: () -> Bool) async -> Bool {
        let deadline = DispatchTime.now() + seconds
        while !condition() {
            if DispatchTime.now() >= deadline { return condition() }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return true
    }
}
