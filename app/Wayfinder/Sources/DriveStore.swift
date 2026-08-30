// Drive Mode core (wayfinder #59, ADR 0012 points 1-3 and 8): a Go/drive/End state machine
// entered from the result card's Go button, driving a heading-up following camera and a
// route-snapped puck off real CLLocationManager fixes. Owns its OWN CLLocationManager, same
// reasoning as TripLogStore.swift's header: a continuous drive trace is a different concern
// from PlanStore's route-editor origin adoption, and PlanStore's accuracy/background settings
// must not change for this.
//
// The drive HUD (wayfinder #60, ADR 0012 points 3-5) extends this with `hud`: the compact
// drive card's ETA/remaining-distance/remaining-time/next-stop-SoC values (throttled to >=1 s
// fix-timestamp deltas), plus `checkedStopCount`/`currentLegIndex`, the position-driven stepper
// advancing the current Leg as stops/leg boundaries are passed, and a `.arrived` phase entered
// on ~40 m destination arrival.
//
// Off-route detection + silent replan (wayfinder #61, ADR 0012 point 6) extends this further: a
// sustained (>=5 s) 50+ m deviation from the displayed route triggers `PlanStore.replanForDrive`
// from the deviated position, and a landed result is swapped in via the same HUD/geometry
// snapshot `go()` uses -- automatic, silent, no camera yank (RootView gates its fit-to-route on
// `phase == .idle`), just a brief "Route updated" toast off `routeUpdatedVersion`.
//
// Trip Log coupling (wayfinder #62, ADR 0012 point 7): Go takes the dash SoC and starts capture,
// reusing TripLogStore's own start/stop phases rather than a second capture path -- `go()` opens
// the start-SoC alert (via `pendingGo`) unless a standalone capture is already `.recording`, in
// which case it's adopted as the drive's capture outright. End or destination arrival stops
// capture with the end-SoC prompt; cancelling the start prompt cancels the Go, no half-open
// capture. Per-sample Trip Log wiring beyond start/stop (#67) is still a later ticket, also keyed
// off `distanceAlongRouteM`.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

// Swift 6 strict concurrency (same justification as PlanStore.swift/TripLogStore.swift):
// `@preconcurrency` conformance is sound here because CLLocationManager delivers callbacks on
// the runloop of the thread that started it -- main, since `locationManager` is a stored
// property initialized from this @MainActor class's `init`.
@MainActor
@Observable
final class DriveStore: NSObject, @preconcurrency CLLocationManagerDelegate {
    enum Phase: Equatable {
        case idle
        case driving
        case arrived
    }

    /// Meaningful only while `phase == .driving`.
    enum CameraMode: Equatable {
        case following
        case freeLook
        case overview
    }

    private(set) var phase: Phase = .idle
    private(set) var cameraMode: CameraMode = .following
    /// Where the puck is: the snapped point while on-route, the raw fix beyond the 10 m snap
    /// tolerance.
    private(set) var snappedCoordinate: CLLocationCoordinate2D?
    private(set) var isOnRoute = false
    /// Progress along the Plan's polyline, in meters. Nothing displays this yet in this slice --
    /// it's the one value the drive HUD (#60), off-route detection (#61), and the step tracker
    /// (#67) will all consume, so it's produced now rather than bolted on later.
    private(set) var distanceAlongRouteM: Double = 0
    private(set) var smoothedCourseDeg: Double = 0

    /// The drive HUD's throttled display values (wayfinder #60, ADR 0012 points 3-4): nil until
    /// `go()` computes the first (unthrottled) value. See `computeHud` and `ingest`'s throttle
    /// comment for the update policy.
    private(set) var hud: DriveHud?
    /// Charging Stops passed -- an index into `stopVMs` (and `plan.stops`), not a count of
    /// arbitrary events: ADR 0012 point 3's only stepper.
    private(set) var checkedStopCount = 0
    /// Leg boundaries passed -- an index into `plan.legs`/`legEndsM`, clamped to `legs.count - 1`.
    private(set) var currentLegIndex = 0
    /// Whether the drive card (wayfinder #60) shows its expanded SoC chart. Plain var, like
    /// `PlanStore.cardExpanded` -- UI + drive-smoke drive it directly.
    var driveCardExpanded = false
    /// Bumped when an off-route replan (wayfinder #61) lands; RootView's toast trigger.
    private(set) var routeUpdatedVersion = 0
    /// A Go awaiting the start-SoC prompt's outcome (wayfinder #62); the drive hasn't entered
    /// yet -- see `go()`/`resolvePendingGo()`.
    private(set) var pendingGo = false

    struct DriveHud: Equatable {
        let etaDate: Date
        let remainingDistM: Double
        let remainingTimeS: Double
        let socAtPosition: Double
        /// The next unchecked Charging Stop's name + arrival SoC, or the destination's
        /// (name = drive destination label, soc = the curve's final value) when none remain.
        let nextLabel: String
        let nextArrivalSoc: Double
        let nextIsDestination: Bool
    }

    /// ADR 0012 point 2's Go gate: origin provenance (adopted from a location fix, not
    /// overridden since) and a plan actually on screen.
    var canGo: Bool {
        phase == .idle && planStore.displayedPlan != nil && planStore.originIsCurrentLocation
    }

    private let planStore: PlanStore
    private let tripStore: TripLogStore
    private let locationManager = CLLocationManager()
    private var routePolyline: [CLLocationCoordinate2D] = []
    private var routeCumulativeM: [Double] = []
    private var lastSegmentIndex: Int?
    private var puckAnnotation: MLNPointAnnotation?
    /// Snapshotted at `go()` for the HUD engine (wayfinder #60) -- see that method's comment.
    private var drivePlan: FfiPlan?
    private var stopVMs: [ChargingStopVM] = []
    /// Cumulative leg-end distances, one entry per `drivePlan.legs`; `legEndsM.last == totalDistM`.
    private var legEndsM: [Double] = []
    /// Per-leg average speed (`leg.distM / leg.driveS`), 0 for a zero-duration leg.
    private var legAvgSpeedMPerS: [Double] = []
    /// The FIX timestamp (not wall clock) `hud` was last recomputed at -- see `ingest`'s
    /// throttle comment for why.
    private var lastHudFixTimestamp: Date?
    /// The FIX timestamp (not wall clock, same reasoning as the HUD throttle) the current
    /// off-route excursion started at; `nil` while on-route or between excursions.
    private var offRouteSinceFixTimestamp: Date?
    /// Set for the duration of an in-flight off-route replan so a second sustained excursion
    /// (or a lingering one) can't fire a redundant `replanForDrive` call.
    private var replanInFlight = false
    /// Incremented in `go()`. An in-flight replan's landed result is adopted only if the
    /// session it was triggered in is still current AND `phase == .driving` -- insurance
    /// against End-then-Go racing an in-flight replan from the previous drive.
    private var driveSession = 0

    /// ADR 0012 point 6's off-route calibration: a lateral deviation past this, sustained for
    /// `offRouteSustainedS`, triggers a silent replan-from-position.
    private static let offRouteThresholdM = 50.0
    private static let offRouteSustainedS = 5.0

    init(planStore: PlanStore, tripStore: TripLogStore) {
        self.planStore = planStore
        self.tripStore = tripStore
        super.init()
        locationManager.delegate = self
        planStore.onUserMapGesture = { [weak self] in self?.noteUserGesture() }
    }

    // MARK: Lifecycle

    /// ADR 0012 point 7 (wayfinder #62): Go and the standalone record button share ONE Trip Log
    /// capture, never two. If a capture is already `.recording` (started via the record button),
    /// it's ADOPTED as the drive's capture and the drive enters immediately; if capture is
    /// `.idle`, this opens the existing start-SoC alert (`TripLogStore.startTapped`) and defers
    /// entry to `resolvePendingGo()`, called once that alert resolves. The `else` branch (a SoC
    /// prompt already up) is unreachable via the UI -- alerts are modal.
    func go() {
        guard canGo, !pendingGo else { return }
        if tripStore.phase == .recording {
            enterDrive()
        } else if tripStore.phase == .idle {
            pendingGo = true
            tripStore.startTapped()
        }
    }

    /// The single resolution point for a pending Go's start-SoC prompt -- called from RootView
    /// when the "Trip start SoC" alert dismisses, OK or Cancel. Entering only on `.recording`
    /// means both an explicit Cancel and `confirmStartSoc`'s denied-authorization refusal cancel
    /// the Go -- no half-open capture, no drive without its log.
    func resolvePendingGo() {
        guard pendingGo else { return }
        pendingGo = false
        if tripStore.phase == .recording {
            enterDrive()
        }
    }

    /// Snapshots the displayed Plan's polyline and HUD engine state at entry via `snapshotPlan`
    /// (shared with the off-route replan swap, wayfinder #61 -- see that method's adopt comment
    /// for what it deliberately leaves untouched), plus this method's own phase-entry work.
    private func enterDrive() {
        guard canGo, let plan = planStore.displayedPlan else { return }

        snapshotPlan(plan)
        snappedCoordinate = nil
        isOnRoute = false
        smoothedCourseDeg = 0
        driveCardExpanded = false
        offRouteSinceFixTimestamp = nil
        replanInFlight = false
        driveSession += 1

        phase = .driving
        cameraMode = .following
        planStore.mapView.showsUserLocation = false
        addPuckIfNeeded()

        // Best-effort: the Go gate already implies location worked once (that's how the origin
        // got adopted), so there's no error surface here in v1.
        locationManager.desiredAccuracy = kCLLocationAccuracyBest
        locationManager.distanceFilter = kCLDistanceFilterNone
        locationManager.activityType = .automotiveNavigation
        locationManager.requestWhenInUseAuthorization()
        locationManager.startUpdatingLocation()
    }

    /// HUD/geometry snapshot shared by `go()` (full entry) and the off-route replan swap
    /// (`triggerOffRouteReplan`'s completion, wayfinder #61): the Plan struct itself (for its
    /// legs/socCurve), each Charging Stop's along-route distance (ChargingStopVM.stops(from:),
    /// same walk ResultCard/SoCChartView already use), and per-leg average speed for the
    /// remaining-time estimate -- see `computeHud`. Deliberately does NOT touch
    /// phase/cameraMode/puck/smoothedCourseDeg -- those survive an off-route swap; the next fix
    /// re-snaps against the new geometry.
    private func snapshotPlan(_ plan: FfiPlan) {
        routePolyline = plan.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        routeCumulativeM = RouteSnap.cumulativeDistances(routePolyline)
        lastSegmentIndex = nil
        distanceAlongRouteM = 0

        drivePlan = plan
        stopVMs = ChargingStopVM.stops(from: plan)
        var cumulativeLegDistM = 0.0
        legEndsM = plan.legs.map { leg in
            cumulativeLegDistM += leg.distM
            return cumulativeLegDistM
        }
        legAvgSpeedMPerS = plan.legs.map { $0.driveS > 0 ? $0.distM / $0.driveS : 0 }
        checkedStopCount = 0
        currentLegIndex = 0
        lastHudFixTimestamp = nil
        hud = computeHud(distanceAlongM: 0)
    }

    /// The Plan and its map layers stay intact -- planning UI just returns, via RootView's
    /// phase switch. Also closes capture (wayfinder #62) with the end-SoC prompt; guard-idempotent
    /// with the arrival path in `ingest` below (already `.promptingEndSoc` by then when arrival
    /// got there first). Cancelling THAT end prompt rides TripLogStore's existing data-loss guard
    /// -- capture resumes as a standalone recording (the drive is already over), stoppable via
    /// the record button.
    func end() {
        guard phase != .idle else { return }
        locationManager.stopUpdatingLocation()
        if let puckAnnotation { planStore.mapView.removeAnnotation(puckAnnotation) }
        puckAnnotation = nil
        planStore.mapView.showsUserLocation = true
        driveCardExpanded = false
        phase = .idle
        if tripStore.phase == .recording { tripStore.stopTapped() }
    }

    // MARK: Fix ingestion

    /// Internal (not private) so drive-smoke can feed synthetic fixes directly -- same seam as
    /// TripLogStore.ingest, since there's no way to inject CoreLocation fixes on the simulator.
    /// No ~1 Hz thinning here (unlike TripLogStore): every fix should move the puck/camera.
    func ingest(_ location: CLLocation) {
        guard phase == .driving else { return }
        guard let result = RouteSnap.snap(
            fix: location.coordinate, polyline: routePolyline, cumulativeM: routeCumulativeM, hintSegment: lastSegmentIndex
        ) else { return }
        lastSegmentIndex = result.segmentIndex
        distanceAlongRouteM = result.distanceAlongRouteM

        // Position-driven stepper (ADR 0012 points 3/5, wayfinder #60): the drive's ONLY
        // stepper. A `while`, not an `if`, in case one fix jumps past more than one stop/leg
        // boundary (a sparse-fix gap, or a synthetic drive-smoke fix landing far along-route).
        var advancedThisFix = false
        while checkedStopCount < stopVMs.count, distanceAlongRouteM >= stopVMs[checkedStopCount].distFromStartM - 40 {
            checkedStopCount += 1
            advancedThisFix = true
        }
        while currentLegIndex < legEndsM.count - 1, distanceAlongRouteM >= legEndsM[currentLegIndex] - 40 {
            currentLegIndex += 1
            advancedThisFix = true
        }

        // Destination arrival (~40 m along-route, ADR 0012 point 3) ends the drive into an
        // arrival state: location updates stop and the puck/camera are left exactly where they
        // last were -- `end()` is what finally tears them down. Also closes capture (wayfinder
        // #62, ADR 0012 point 7) with the end-SoC prompt -- arrival is as much an end as tapping
        // End is.
        if distanceAlongRouteM >= (drivePlan?.totalDistM ?? 0) - 40 {
            phase = .arrived
            hud = computeHud(distanceAlongM: distanceAlongRouteM)
            lastHudFixTimestamp = location.timestamp
            locationManager.stopUpdatingLocation()
            if tripStore.phase == .recording { tripStore.stopTapped() }
            return
        }

        if result.distanceFromRouteM <= 10 {
            snappedCoordinate = result.coordinate
            isOnRoute = true
        } else {
            snappedCoordinate = location.coordinate
            isOnRoute = false
        }

        // Off-route detection (ADR 0012 point 6, wayfinder #61): a sustained deviation triggers
        // a silent replan-from-position. Keyed on FIX timestamps, not wall-clock time -- same
        // determinism reasoning as the HUD throttle below.
        if result.distanceFromRouteM >= Self.offRouteThresholdM, !replanInFlight {
            if let since = offRouteSinceFixTimestamp {
                if location.timestamp.timeIntervalSince(since) >= Self.offRouteSustainedS {
                    triggerOffRouteReplan(from: location)
                }
            } else {
                offRouteSinceFixTimestamp = location.timestamp
            }
        }
        if result.distanceFromRouteM < Self.offRouteThresholdM {
            offRouteSinceFixTimestamp = nil
        }

        // Capped course smoothing (ADR 0012 point 3): move toward the target heading along the
        // shortest arc, capped at +/-45 deg per fix, so a noisy/absent course reading never
        // snaps the camera instantly. `location.course` is negative when invalid, in which case
        // the snap result's own segment bearing is the best available heading.
        let targetCourseDeg = location.course >= 0 ? location.course : result.segmentBearingDeg
        smoothedCourseDeg = Self.stepCourse(current: smoothedCourseDeg, target: targetCourseDeg, maxStepDeg: 45)

        if let puckAnnotation, let snappedCoordinate {
            puckAnnotation.coordinate = snappedCoordinate
        }
        if cameraMode == .following {
            applyFollowingCamera()
        }

        // HUD throttle (ADR 0012 point 3, >=1 s UI deltas): keyed on the FIX's own timestamp,
        // not wall-clock time, so this is deterministic for drive-smoke's synthetic fixes -- a
        // stop/leg advance always updates immediately regardless of the throttle window.
        let dueForHudUpdate = advancedThisFix || lastHudFixTimestamp == nil
            || location.timestamp.timeIntervalSince(lastHudFixTimestamp!) >= 1.0
        if dueForHudUpdate {
            hud = computeHud(distanceAlongM: distanceAlongRouteM)
            lastHudFixTimestamp = location.timestamp
        }
    }

    // MARK: Off-route replan (wayfinder #61, ADR 0012 point 6)

    /// Fires once a deviation has been sustained past `offRouteSustainedS`. Computes the
    /// departure SoC from the Plan's own predicted curve at the current position (model-driven,
    /// not the settings slider) and the set of waypoints still ahead -- snapped against the OLD
    /// `routePolyline` (the new one doesn't exist yet); a waypoint that fails to snap is kept
    /// rather than silently dropped. Adopts the landed plan on completion only if this drive
    /// session is still current and still driving -- see `driveSession`'s comment.
    private func triggerOffRouteReplan(from location: CLLocation) {
        guard let drivePlan else { return }
        replanInFlight = true
        offRouteSinceFixTimestamp = nil

        let departSoc = Self.interpolatedSoc(drivePlan.socCurve, at: distanceAlongRouteM)
        let currentDistanceAlongRouteM = distanceAlongRouteM
        let keepingWaypointIds = Set(planStore.waypoints.filter { waypoint in
            guard let snap = RouteSnap.snap(
                fix: waypoint.coordinate, polyline: routePolyline, cumulativeM: routeCumulativeM, hintSegment: nil
            ) else { return true }
            return snap.distanceAlongRouteM > currentDistanceAlongRouteM
        }.map(\.id))
        let session = driveSession

        Task {
            let result = await planStore.replanForDrive(
                from: location.coordinate, keepingWaypointIds: keepingWaypointIds, departSoc: departSoc
            )
            if let result, phase == .driving, session == driveSession {
                snapshotPlan(result)
                routeUpdatedVersion += 1
            }
            replanInFlight = false
        }
    }

    // MARK: HUD (wayfinder #60, ADR 0012 points 3-4)

    /// Recomputes the drive HUD at `distanceAlongM`: remaining distance/time from the remaining
    /// Leg geometry, SoC read off the Plan's own predicted curve (model-driven, ADR 0012 point
    /// 5), and the next unchecked Charging Stop (or the destination, once none remain). Also
    /// called once from `go()` at distanceAlong=0 for the drive's initial HUD values.
    private func computeHud(distanceAlongM: Double) -> DriveHud {
        guard let drivePlan else {
            return DriveHud(
                etaDate: Date(), remainingDistM: 0, remainingTimeS: 0, socAtPosition: 0,
                nextLabel: "", nextArrivalSoc: 0, nextIsDestination: true
            )
        }

        let totalDistM = drivePlan.totalDistM
        let remainingDistM = max(0, totalDistM - distanceAlongM)

        let legEndM = legEndsM.indices.contains(currentLegIndex) ? legEndsM[currentLegIndex] : totalDistM
        let avgSpeedMPerS = legAvgSpeedMPerS.indices.contains(currentLegIndex) ? legAvgSpeedMPerS[currentLegIndex] : 0
        let remainingInLegM = max(0, legEndM - distanceAlongM)
        let remainingInLegTimeS = avgSpeedMPerS > 0 ? remainingInLegM / avgSpeedMPerS : 0

        let futureLegsTimeS = drivePlan.legs.count > currentLegIndex + 1
            ? drivePlan.legs[(currentLegIndex + 1)...].reduce(0) { $0 + $1.driveS }
            : 0
        let uncheckedStopsChargeS = stopVMs.count > checkedStopCount
            ? stopVMs[checkedStopCount...].reduce(0) { $0 + $1.chargeS }
            : 0
        let remainingTimeS = remainingInLegTimeS + futureLegsTimeS + uncheckedStopsChargeS

        let socAtPosition = Self.interpolatedSoc(drivePlan.socCurve, at: distanceAlongM)

        let nextLabel: String
        let nextArrivalSoc: Double
        let nextIsDestination: Bool
        if checkedStopCount < stopVMs.count {
            let next = stopVMs[checkedStopCount]
            nextLabel = next.name
            nextArrivalSoc = next.arrivalSoc
            nextIsDestination = false
        } else {
            nextLabel = drivePlan.legs.last?.toLabel ?? "Destination"
            nextArrivalSoc = drivePlan.socCurve.last?.soc ?? 0
            nextIsDestination = true
        }

        return DriveHud(
            etaDate: Date().addingTimeInterval(remainingTimeS), remainingDistM: remainingDistM,
            remainingTimeS: remainingTimeS, socAtPosition: socAtPosition, nextLabel: nextLabel,
            nextArrivalSoc: nextArrivalSoc, nextIsDestination: nextIsDestination
        )
    }

    /// Linear interpolation of `curve` (sorted ascending by `distM`) at `distanceM`, clamped to
    /// the curve's ends. At a charge-jump distance where two samples share `distM`, this forward
    /// scan lands on the pre-charge sample -- fine either way, per this ticket's spec.
    private static func interpolatedSoc(_ curve: [FfiSocPoint], at distanceM: Double) -> Double {
        guard let first = curve.first else { return 0 }
        guard distanceM > first.distM else { return first.soc }
        var previous = first
        for point in curve.dropFirst() {
            if distanceM <= point.distM {
                let span = point.distM - previous.distM
                guard span > 0 else { return point.soc }
                let t = (distanceM - previous.distM) / span
                return previous.soc + t * (point.soc - previous.soc)
            }
            previous = point
        }
        return previous.soc
    }

    private static func stepCourse(current: Double, target: Double, maxStepDeg: Double) -> Double {
        var delta = (target - current).truncatingRemainder(dividingBy: 360)
        if delta > 180 { delta -= 360 }
        if delta < -180 { delta += 360 }
        let clamped = max(-maxStepDeg, min(maxStepDeg, delta))
        var result = (current + clamped).truncatingRemainder(dividingBy: 360)
        if result < 0 { result += 360 }
        return result
    }

    // MARK: Camera

    /// Free-look on ANY map gesture (ADR 0012 point 3), from either following or overview --
    /// wired to PlanStore.onUserMapGesture, which only fires for real gesture reasons, never
    /// `.programmatic` (see PlanStore's MLNCameraChangeReason check), so this following camera's
    /// own `setCamera` calls below can never cancel themselves out.
    func noteUserGesture() {
        guard phase == .driving, cameraMode != .freeLook else { return }
        cameraMode = .freeLook
    }

    func recenter() {
        guard phase == .driving else { return }
        cameraMode = .following
        applyFollowingCamera()
    }

    /// Frames the REMAINING route (from the last-known segment to the end; the whole polyline
    /// before any fix has landed) when entering overview; recentres back to following when
    /// leaving it.
    func toggleOverview() {
        guard phase == .driving else { return }
        guard cameraMode != .overview else {
            recenter()
            return
        }
        cameraMode = .overview

        let remaining = lastSegmentIndex.map { Array(routePolyline[$0...]) } ?? routePolyline
        guard remaining.count >= 2 else { return }
        var minLat = remaining[0].latitude, maxLat = minLat
        var minLon = remaining[0].longitude, maxLon = minLon
        for c in remaining {
            minLat = min(minLat, c.latitude); maxLat = max(maxLat, c.latitude)
            minLon = min(minLon, c.longitude); maxLon = max(maxLon, c.longitude)
        }
        let bounds = MLNCoordinateBoundsMake(
            CLLocationCoordinate2D(latitude: minLat, longitude: minLon),
            CLLocationCoordinate2D(latitude: maxLat, longitude: maxLon)
        )
        let camera = planStore.mapView.cameraThatFitsCoordinateBounds(
            bounds, edgePadding: UIEdgeInsets(top: 40, left: 40, bottom: 40, right: 40)
        )
        camera.heading = 0
        camera.pitch = 0
        planStore.mapView.setCamera(camera, withDuration: 0.8, animationTimingFunction: CAMediaTimingFunction(name: .linear))
    }

    /// `MLNMapCamera(lookingAtCenter:altitude:pitch:heading:)`, applied with a linear 0.8s
    /// transition. IMPORTANT: this is a PROGRAMMATIC camera move, which fires PlanStore's
    /// `shouldChangeFrom:to:reason:` delegate with reason `.programmatic` -- `onUserMapGesture`
    /// only trips on real gesture reasons, so this can't cancel itself back out of following.
    private func applyFollowingCamera() {
        guard let snappedCoordinate else { return }
        let camera = MLNMapCamera(
            lookingAtCenter: snappedCoordinate, altitude: 800, pitch: 45, heading: smoothedCourseDeg
        )
        planStore.mapView.setCamera(camera, withDuration: 0.8, animationTimingFunction: CAMediaTimingFunction(name: .linear))
    }

    // MARK: Puck

    /// PlanStore's `mapView(_:imageFor:)` supplies the image for this annotation's "drive-puck"
    /// title. Known simplification: the image is a static up-pointing arrow, correct while the
    /// camera is heading-up (following), approximate in free-look/overview -- the drive-HUD
    /// ticket can refine this.
    private func addPuckIfNeeded() {
        guard puckAnnotation == nil else { return }
        let annotation = MLNPointAnnotation()
        annotation.title = "drive-puck"
        annotation.coordinate = routePolyline.first ?? CLLocationCoordinate2D()
        planStore.mapView.addAnnotation(annotation)
        puckAnnotation = annotation
    }

    // MARK: CLLocationManagerDelegate

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        for location in locations {
            ingest(location)
        }
    }
}
