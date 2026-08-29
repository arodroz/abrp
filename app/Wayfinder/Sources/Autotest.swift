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
// network-dependent one.
// `--autotest triplog-smoke` (wayfinder #51) drives TripLogStore's capture
// lifecycle directly -- there's no location-fix injection on the sim, so
// synthetic CLLocations are fed straight to `ingest` -- and verifies the
// saved tlog-1 JSON against the schema #52's Rust `calibrate()` will parse.
import CoreLocation
import CryptoKit
import Darwin
import Foundation
import MapLibre
import PlannerKit

enum Autotest {
    static func runIfRequested(store: PlanStore, installer: PackInstaller, tripStore: TripLogStore) {
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
    /// installed rpack and parses its Charger Pack (lu-dev's known 17 chargers), then deletes
    /// the region and checks the artifact files + record are gone while the shared style files
    /// remain.
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

    /// Drives TripLogStore's capture lifecycle directly. Proves: the start/end SoC prompts and
    /// their phase transitions, the cancel-end-SoC data-loss guard (cancelling resumes
    /// recording rather than truncating the trace), and that the saved tlog-1 JSON matches the
    /// schema on the fields #52's Rust `calibrate()` will parse -- including a byte-level
    /// spot-check that the on-disk keys are really snake_case, independent of the Swift model.
    @MainActor
    private static func runTriplogSmoke(tripStore: TripLogStore) async {
        var ok = true

        tripStore.fetchTemperature = { _, _, _ in 14.5 }

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
