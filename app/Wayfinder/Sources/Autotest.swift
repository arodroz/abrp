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
// the sheet's Slider would trigger.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

enum Autotest {
    static func runIfRequested(store: PlanStore) {
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

    private static func runPlanGolden() async {
        guard let located = Packs.locate(region: "corridor") else {
            report("pack-present", false, "Documents/corridor.rpack or corridor-chargers.json missing")
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

        let chargersReady = await waitWithTimeout(seconds: 15) { store.chargerCount > 0 }
        let chargersOK = chargersReady && store.chargerCount == 1549
        report("chargers-count", chargersOK, "expected 1549, got \(store.chargerCount)")

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
    /// Antwerp golden at the 0.90 default, the Capellen golden (with its exact 100 km/h Speed
    /// Cap) after `departSoc` drops to 0.30, a genuinely different plan after a temperature
    /// change (no pinned golden exists at -5 °C), and the appearance override actually
    /// swapping the map style.
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

        let lastLegSpeedCapKmh = store.plan?.legs.last?.speedCapKmh
        let speedCapOk = lastLegSpeedCapKmh == 100.0
        report("speed-cap-100", speedCapOk, "expected 100.0, got \(String(describing: lastLegSpeedCapKmh))")
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

    /// Pinned observed values, LU -> Antwerp, depart 0.30 (matches
    /// core/optimiser/tests/golden_corridor.rs's `golden_speed_cap_exercised`).
    private static func assertDepart30Golden(_ plan: FfiPlan) -> (Bool, String) {
        var failures: [String] = []
        if plan.stops.count != 1 { failures.append("stops.count=\(plan.stops.count), want 1") }
        let stopName = plan.stops.first?.name ?? "<none>"
        let expectedName = "SuperChargy - Aire de Capellen direction Arlon"
        if stopName != expectedName { failures.append("stop name=\"\(stopName)\"") }
        let stopArrivalSoc = plan.stops.first?.arrivalSoc ?? -1
        if !isClose(stopArrivalSoc, 0.247, tol: 0.005) { failures.append("stop arrivalSoc=\(stopArrivalSoc)") }
        let stopDepartSoc = plan.stops.first?.departSoc ?? -1
        if !isClose(stopDepartSoc, 0.800, tol: 0.005) { failures.append("stop departSoc=\(stopDepartSoc)") }
        let chargeS = plan.stops.first?.chargeS ?? -1
        if !isClosePct(chargeS, 993, pct: 0.10) { failures.append("chargeS=\(chargeS)") }
        let lastLegArrivalSoc = plan.legs.last?.arrivalSoc ?? -1
        if !isClose(lastLegArrivalSoc, 0.111, tol: 0.005) { failures.append("last leg arrivalSoc=\(lastLegArrivalSoc)") }
        return (failures.isEmpty, failures.joined(separator: "; "))
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
