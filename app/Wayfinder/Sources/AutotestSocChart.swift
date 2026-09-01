// `--autotest soc-chart-smoke` (wayfinder #83): asserts SoCChartModel's pure functions directly
// -- no UI, no screenshots. Callouts/margin-color/interpolation need no store at all (see
// SoCChartModel.swift's own docs: they take minimal per-stop/curve inputs, not a whole FfiPlan).
// The trail's exact >=500m/>=0.5pt thinning math is proven purely too, against synthetic
// SoCTrailPoint tuples; the DriveStore-level check that follows only proves the SEAM -- that a
// fresh reading during a real `ingest` actually lands in `socTrail`, and that `enterDrive` resets
// it -- via `TelemetryLinkStore.setSyntheticDisplaySoc` rather than a full scripted engine
// dialogue (AutotestLiveSoc.swift's 34-exchange script), which this ticket's own instructions say
// not to rebuild just for this.
//
// This file also hosts the two REVIEWER-FACING demo modes (wayfinder #83): `chart-demo-plan` and
// `chart-demo-drive` stage the overhauled chart in the result card / drive HUD and print a READY
// line instead of calling `finish` on the happy path -- they never terminate the app themselves,
// so the 14-mode sweep excludes them; a reviewer screenshots the running app after the READY line.
//
// A SEPARATE FILE, same reasoning as AutotestObdSmoke.swift's header: this ticket's new code
// reproducibly breaks the Swift 6 strict-concurrency checker once appended to the ~2500-line
// Autotest.swift.
import CoreLocation
import Foundation
import PlannerKit

extension Autotest {
    @MainActor
    static func runSocChartSmoke() async {
        var ok = true

        // 1: arrival-SoC callouts -- two stops + destination, minimal ChargingStopVM inputs (no
        // FfiPlan constructed at all, per this ticket's own guidance).
        let stops = [
            ChargingStopVM(
                id: "c1", name: "Stop A", lat: 0, lon: 0, powerKw: 150,
                arrivalSoc: 0.15, departSoc: 0.80, chargeS: 1200, distFromStartM: 50_000
            ),
            ChargingStopVM(
                id: "c2", name: "Stop B", lat: 0, lon: 0, powerKw: 50,
                arrivalSoc: 0.08, departSoc: 0.60, chargeS: 900, distFromStartM: 120_000
            ),
        ]
        let callouts = SoCChartModel.arrivalCallouts(stops: stops, destinationDistM: 200_000, destinationSocFraction: 0.42)
        let calloutsOk = callouts.count == 3
            && callouts[0].distM == 50_000 && callouts[0].label == "15%" && callouts[0].color == .orange
            && callouts[1].distM == 120_000 && callouts[1].label == "8%" && callouts[1].color == .red
            && callouts[2].distM == 200_000 && callouts[2].label == "42%" && callouts[2].color == .green
        report("arrival-callouts", calloutsOk, "\(callouts)")
        ok = ok && calloutsOk

        // 2: socMarginColor boundaries -- 9.9 red, 10/20 amber (both inclusive), 20.1/50 green.
        let marginOk = SoCChartModel.socMarginColor(9.9) == .red
            && SoCChartModel.socMarginColor(10) == .orange
            && SoCChartModel.socMarginColor(20) == .orange
            && SoCChartModel.socMarginColor(20.1) == .green
            && SoCChartModel.socMarginColor(50) == .green
        report("margin-color-boundaries", marginOk)
        ok = ok && marginOk

        // 3: curve interpolation -- mid-segment, before-first, after-last clamping.
        let curve = [
            FfiSocPoint(distM: 0, soc: 0.8),
            FfiSocPoint(distM: 100_000, soc: 0.5),
            FfiSocPoint(distM: 200_000, soc: 0.2),
        ]
        let midValue = SoCChartModel.interpolatedSocFraction(curve, at: 50_000)
        let midOk = abs(midValue - 0.65) <= 0.0001
        let beforeFirstOk = SoCChartModel.interpolatedSocFraction(curve, at: -10) == 0.8
        let afterLastOk = SoCChartModel.interpolatedSocFraction(curve, at: 300_000) == 0.2
        report("interpolation-mid", midOk, "value=\(midValue)")
        report("interpolation-before-first", beforeFirstOk)
        report("interpolation-after-last", afterLastOk)
        ok = ok && midOk && beforeFirstOk && afterLastOk

        // 4: trail thinning -- the exact >=500m/>=0.5pt rule, against synthetic tuples.
        let base = SoCChartModel.SoCTrailPoint(distM: 0, socPct: 50)
        let firstAlwaysKeptOk = SoCChartModel.shouldAppendTrailPoint(lastKept: nil, candidate: base)
        let tooCloseDroppedOk = !SoCChartModel.shouldAppendTrailPoint(
            lastKept: base, candidate: SoCChartModel.SoCTrailPoint(distM: 400, socPct: 50)
        )
        let farEnoughKeptOk = SoCChartModel.shouldAppendTrailPoint(
            lastKept: base, candidate: SoCChartModel.SoCTrailPoint(distM: 500, socPct: 50)
        )
        let socJumpKeptOk = SoCChartModel.shouldAppendTrailPoint(
            lastKept: base, candidate: SoCChartModel.SoCTrailPoint(distM: 100, socPct: 50.6)
        )
        let smallSocDroppedOk = !SoCChartModel.shouldAppendTrailPoint(
            lastKept: base, candidate: SoCChartModel.SoCTrailPoint(distM: 100, socPct: 50.4)
        )
        let thinningOk = firstAlwaysKeptOk && tooCloseDroppedOk && farEnoughKeptOk && socJumpKeptOk && smallSocDroppedOk
        report(
            "trail-thinning", thinningOk,
            "first=\(firstAlwaysKeptOk) tooClose=\(tooCloseDroppedOk) farEnough=\(farEnoughKeptOk) "
                + "socJump=\(socJumpKeptOk) smallSoc=\(smallSocDroppedOk)"
        )
        ok = ok && thinningOk

        // 5-7: the store-level seam -- a real DriveStore over a plan+go dance (crib of
        // live-soc-smoke's), proving `ingest` appends to `socTrail` only once a fresh reading
        // exists, and `enterDrive` resets it on re-entry. Local stores throughout, like
        // live-soc-smoke's own, not the shared ones drive-smoke uses.
        let planStore = PlanStore()
        let tripStore = TripLogStore()
        tripStore.authorizationStatus = { .authorizedWhenInUse }
        tripStore.fetchTemperature = { _, _, _ in 15.0 }
        let telemetryStore = TelemetryLinkStore(link: StubTelemetryLink())
        let driveStore = DriveStore(planStore: planStore, tripStore: tripStore, telemetryStore: telemetryStore)

        planStore.load()
        let plannerReadyOk = await waitWithTimeout(seconds: 120) { planStore.plannerStatus == .ready }
        report("planner-ready", plannerReadyOk)
        ok = ok && plannerReadyOk
        guard plannerReadyOk else { await finish(ok: false) }

        planStore.locationManager(
            CLLocationManager(), didUpdateLocations: [CLLocation(latitude: 49.6116, longitude: 6.1319)]
        )
        planStore.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLandedOk = await waitWithTimeout(seconds: 30) { planStore.plan != nil }
        report("plan-landed", planLandedOk)
        ok = ok && planLandedOk
        guard planLandedOk, let polyline = planStore.displayedPlan?.polyline, polyline.count >= 51 else {
            report("trail-seam", false, "polyline too short for this smoke's fixed indices")
            await finish(ok: false)
        }

        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        let enteredOk = driveStore.phase == .driving
        report("entered", enteredOk, "phase=\(driveStore.phase)")
        ok = ok && enteredOk

        func fix(at index: Int) -> CLLocation {
            let vertex = polyline[index]
            return CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: vertex.lat, longitude: vertex.lon),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: -1, timestamp: Date()
            )
        }

        // No fresh reading yet -- ingest must not append.
        driveStore.ingest(fix(at: 0))
        let noReadingNoAppendOk = driveStore.socTrail.isEmpty
        report("no-reading-no-append", noReadingNoAppendOk, "socTrail=\(driveStore.socTrail)")
        ok = ok && noReadingNoAppendOk

        // A fresh reading lands with the very next fix.
        telemetryStore.setSyntheticDisplaySoc(75)
        driveStore.ingest(fix(at: 50))
        let appendedOk = driveStore.socTrail.count == 1 && driveStore.socTrail.first?.socPct == 75
        report("fresh-reading-appends", appendedOk, "socTrail=\(driveStore.socTrail)")
        ok = ok && appendedOk

        // Re-entering the drive resets the trail.
        driveStore.end()
        tripStore.confirmEndSoc(70)
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        let resetOk = driveStore.socTrail.isEmpty
        report("enter-drive-resets-trail", resetOk, "socTrail=\(driveStore.socTrail)")
        ok = ok && resetOk

        await finish(ok: ok)
    }

    // MARK: chart-demo-plan / chart-demo-drive (wayfinder #83, visual verification -- see header)

    /// Stages the result card, expanded, over the Luxembourg -> Amsterdam plan -- no drive, the
    /// planning-mode chart (scrub gesture, no live dot/trail).
    @MainActor
    static func runChartDemoPlan(store: PlanStore) async {
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.vehicle = .ioniq5Lr2wd
        store.load()
        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        report("plan-landed", planLanded)

        store.cardExpanded = true
        print("AUTOTEST chart-demo-plan READY")
    }

    /// Same plan, entered as a drive to ~40% along the route, with a scripted fake live-SoC trail
    /// fed through the store's normal `ingest` recording path (via `setSyntheticDisplaySoc`, this
    /// ticket's own sanctioned demo-only seam -- there's no real BLE hardware on the simulator to
    /// produce one otherwise) -- the drive HUD's expanded chart (live dot + trail overlay).
    @MainActor
    static func runChartDemoDrive(
        store: PlanStore, tripStore: TripLogStore, driveStore: DriveStore, telemetryStore: TelemetryLinkStore
    ) async {
        tripStore.authorizationStatus = { .authorizedWhenInUse }
        tripStore.fetchTemperature = { _, _, _ in 15.0 }

        // Origin adopted via the location-fix delegate path, NOT `setOrigin` (which marks it
        // overridden and would leave `canGo` permanently false) -- same as drive-smoke/
        // live-soc-smoke's own dance.
        store.vehicle = .ioniq5Lr2wd
        store.load()
        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        store.locationManager(
            CLLocationManager(), didUpdateLocations: [CLLocation(latitude: 49.6116, longitude: 6.1319)]
        )
        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLanded = await waitWithTimeout(seconds: 30) { store.plan != nil }
        report("plan-landed", planLanded)
        guard planLanded, let polyline = store.displayedPlan?.polyline, polyline.count >= 10 else {
            await finish(ok: false)
        }

        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        let entered = driveStore.phase == .driving
        report("entered", entered, "phase=\(driveStore.phase) canGo-was=\(driveStore.canGo)")
        guard entered else { await finish(ok: false) }

        // Sweeps synthetic fixes to ~40% of the route, feeding a plausible descending fake trail
        // through the SAME normal recording path `ingest` uses.
        let targetIndex = max(1, Int(Double(polyline.count - 1) * 0.4))
        let stepIndex = max(1, targetIndex / 20)
        var soc = 82.0
        for index in stride(from: 0, through: targetIndex, by: stepIndex) {
            let vertex = polyline[index]
            telemetryStore.setSyntheticDisplaySoc(soc)
            driveStore.ingest(CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: vertex.lat, longitude: vertex.lon),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 20, timestamp: Date()
            ))
            soc -= 1.5
        }

        driveStore.driveCardExpanded = true
        print("AUTOTEST chart-demo-drive READY")
    }
}
