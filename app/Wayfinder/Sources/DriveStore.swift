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
// `phase == .idle`), just a brief "Route updated" toast off `routeUpdatedVersion`. Manual mid-drive
// SoC correction (wayfinder #63) reuses this same replan path, anchored on the driver's entered
// dash % instead of the model's own curve estimate.
//
// Trip Log coupling (wayfinder #62, ADR 0012 point 7): Go takes the dash SoC and starts capture,
// reusing TripLogStore's own start/stop phases rather than a second capture path -- `go()` opens
// the start-SoC alert (via `pendingGo`) unless a standalone capture is already `.recording`, in
// which case it's adopted as the drive's capture outright. End or destination arrival stops
// capture with the end-SoC prompt; cancelling the start prompt cancels the Go, no half-open
// capture.
//
// Maneuver banners (wayfinder #67, ADR 0012 point 3's HUD extended to turn-by-turn guidance):
// `banner` is StepTracker's upcoming-step lookup resolved into display values, recomputed on
// EVERY fix while driving (unthrottled -- the countdown is the point) and muted while off-route-
// but-unreplanned or mid-replan, same as the HUD stays live but the banner shouldn't imply a
// route that may no longer be current. `guidanceSteps` is rebuilt at every `snapshotPlan` call,
// so a replan swap can never leave a banner pointing at stale step indices.
//
// Voice guidance (wayfinder #68, the banner's audio twin): `SpeechController` speaks tiered
// prompts computed by `VoicePromptScheduler` from the same `guidanceSteps`/`distanceAlongRouteM`
// the banner reads -- muted by the IDENTICAL shared gate (banner nil <=> voice frozen), and
// logged to `voiceEventLog` as a test seam that's appended to ALWAYS, mute or not, so muting
// only silences `SpeechController`, never the assertable log.
//
// Live SoC (wayfinder #79) / telemetry auto-capture (wayfinder #80): the 12V-safety gate's only
// writers are `go()` (opens, at intent-to-drive -- moved here from `enterDrive` so the link is
// usually connected by the time the driver answers the start-SoC prompt) and three closers:
// `resolvePendingGo()` (the Go was cancelled -- start-SoC prompt Cancel, or its denied-auth
// refusal -- before ever entering the drive), `end()`, and the arrival branch in `ingest`. The
// invariant this preserves: the gate is open exactly during [go() ... drive end/arrival/cancel],
// never longer. Closing on end/arrival always happens AFTER trip capture closes -- see the
// ordering comment in `end()`; that ordering exists FOR the end-of-trip telemetry snapshot
// (wayfinder #80, `TripLogStore.stopTapped`), which must still be able to read `latestReadings`.
//
// SoC chart overhaul (wayfinder #83): `socTrail` records the ACTUAL SoC over distance (Tesla-
// style overlay on the predicted curve) whenever a fresh live OBD reading lands during `ingest`,
// thinned via `SoCChartModel.shouldAppendTrailPoint`; reset on every `enterDrive`, same lifecycle
// as `voiceEventLog`. Everything else the chart needs (callouts, margin coloring, interpolation)
// is pure and lives in SoCChartModel.swift instead, so this file's own state stays exactly what
// it was: the trail is its one addition.
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
    /// The maneuver banner's current display values (wayfinder #67), nil whenever there's no
    /// upcoming step (v1 pack, guidance exhausted) or guidance is muted (off-route-but-
    /// unreplanned, mid-replan) -- see `computeBanner`.
    private(set) var banner: ManeuverBanner?
    /// Voice guidance test seam (wayfinder #68): every prompt/stop event, appended REGARDLESS of
    /// `voiceMuted` -- the store-side assertable truth for what would have spoken, independent of
    /// whether `SpeechController` was actually allowed to speak it. Reset on each `enterDrive`.
    private(set) var voiceEventLog: [VoiceEvent] = []
    /// Charging Stops passed -- an index into `stopVMs` (and `plan.stops`), not a count of
    /// arbitrary events: ADR 0012 point 3's only stepper.
    private(set) var checkedStopCount = 0
    /// Leg boundaries passed -- an index into `plan.legs`/`legEndsM`, clamped to `legs.count - 1`.
    private(set) var currentLegIndex = 0
    /// Whether the drive card (wayfinder #60) shows its expanded SoC chart. Plain var, like
    /// `PlanStore.cardExpanded` -- UI + drive-smoke drive it directly.
    var driveCardExpanded = false
    /// The actual-SoC trail (wayfinder #83, design point 5's Tesla-style overlay): thinned via
    /// `SoCChartModel.shouldAppendTrailPoint`, appended to in `ingest` only when the live OBD
    /// reading is fresh -- empty (and the chart identical to the predicted-only curve) with no
    /// dongle connected. Reset on every `enterDrive`.
    private(set) var socTrail: [SoCChartModel.SoCTrailPoint] = []
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

    /// One resolved maneuver banner (wayfinder #67): `distanceM` is the live countdown
    /// (`GuidanceStep.distAlongRouteM - distanceAlongRouteM`), everything else copied straight
    /// off the upcoming `StepTracker.GuidanceStep`.
    struct ManeuverBanner: Equatable {
        let iconSystemName: String
        let distanceM: Double
        let primary: String
        let secondary: String?
        let then: String?
    }

    /// Voice guidance test seam (wayfinder #68) -- see `voiceEventLog`.
    enum VoiceEvent: Equatable {
        case spoke(String)
        case stopped
    }

    /// ADR 0012 point 2's Go gate: origin provenance (adopted from a location fix, not
    /// overridden since) and a plan actually on screen.
    var canGo: Bool {
        phase == .idle && planStore.displayedPlan != nil && planStore.originIsCurrentLocation
    }

    /// Voice guidance mute (wayfinder #68), for the drive-controls mute button and drive-smoke.
    /// A STORED observable mirror of `SpeechController`'s UserDefaults-persisted flag, not a
    /// computed forward: @Observable tracks stored properties only, so a computed getter reading
    /// UserDefaults would never invalidate the mute button's view (its label would freeze --
    /// the first gate run failed exactly there). `didSet` keeps SpeechController authoritative
    /// for persistence and the hard-stop side effect.
    var voiceMuted: Bool = UserDefaults.standard.bool(forKey: "voiceMuted") {
        didSet { speech.muted = voiceMuted }
    }

    private let planStore: PlanStore
    private let tripStore: TripLogStore
    /// The 12V-safety gate this drive opens/closes (wayfinder #79): `nil` only in contexts that
    /// don't wire telemetry up at all. See `enterDrive`/`end`.
    private let telemetryStore: TelemetryLinkStore?
    private let locationManager = CLLocationManager()
    private var routePolyline: [CLLocationCoordinate2D] = []
    private var routeCumulativeM: [Double] = []
    private var lastSegmentIndex: Int?
    private var puckAnnotation: MLNPointAnnotation?
    /// Snapshotted at `go()` for the HUD engine (wayfinder #60) -- see that method's comment.
    private var drivePlan: FfiPlan?
    private var stopVMs: [ChargingStopVM] = []
    /// The current Plan's banner-eligible steps (wayfinder #67), rebuilt on every `snapshotPlan`
    /// call -- see that method and `computeBanner`.
    private var guidanceSteps: [StepTracker.GuidanceStep] = []
    /// Voice guidance (wayfinder #68): the AVSpeechSynthesizer wrapper and the tier-scheduling
    /// bookkeeping, reset on every `snapshotPlan` swap -- see that method's comment.
    private let speech = SpeechController()
    private var voiceState = VoicePromptScheduler.State()
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

    init(planStore: PlanStore, tripStore: TripLogStore, telemetryStore: TelemetryLinkStore?) {
        self.planStore = planStore
        self.tripStore = tripStore
        self.telemetryStore = telemetryStore
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
        // 12V-safety gate (wayfinder #80): opened HERE, at intent-to-drive, not at actual entry
        // -- so the link is usually already connected by the time the driver confirms the
        // start-SoC prompt, giving that prompt's auto-fill (and the lazy start snapshot) a fresh
        // reading. Every path below that doesn't reach `enterDrive` must close it again.
        telemetryStore?.gateOpen = true
        if tripStore.phase == .recording {
            enterDrive()
        } else if tripStore.phase == .idle {
            pendingGo = true
            tripStore.startTapped()
        } else {
            // Unreachable via the UI (alerts are modal) -- closed defensively so the gate
            // invariant holds even here.
            telemetryStore?.gateOpen = false
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
        } else {
            // The Go was cancelled before ever entering the drive (wayfinder #80): the gate
            // `go()` opened optimistically must close again.
            telemetryStore?.gateOpen = false
        }
    }

    /// Snapshots the displayed Plan's polyline and HUD engine state at entry via `snapshotPlan`
    /// (shared with the off-route replan swap, wayfinder #61 -- see that method's adopt comment
    /// for what it deliberately leaves untouched), plus this method's own phase-entry work.
    private func enterDrive() {
        guard canGo, let plan = planStore.displayedPlan else { return }

        // 12V-safety gate (wayfinder #79/#80): already open -- `go()` opens it at
        // intent-to-drive, before this method ever runs.

        // Flags reset BEFORE snapshotPlan (wayfinder #67): snapshotPlan now computes the initial
        // banner, and computeBanner mutes while `replanInFlight` -- a drive entered right after a
        // previous drive ended mid-replan would otherwise start with a wrongly muted banner.
        offRouteSinceFixTimestamp = nil
        replanInFlight = false
        driveSession += 1
        // wayfinder #68: the voice log's story starts fresh each drive, same reasoning as the
        // flags above -- a re-entered drive must not carry over the previous drive's spoken log.
        voiceEventLog = []
        // wayfinder #83: the actual-SoC trail is drive-scoped too -- a re-entered drive starts a
        // fresh trail, never carrying over the previous drive's.
        socTrail = []
        snapshotPlan(plan)
        snappedCoordinate = nil
        isOnRoute = false
        smoothedCourseDeg = 0
        driveCardExpanded = false

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

        // e2e drive seam (wayfinder #67), mirror of PlanStore's `-simulatedLocationFix` seam:
        // XCUITest cannot inject CoreLocation fixes, so `-simulatedDriveDistancesM` (a
        // ";"-separated list of along-route meters, launch-argument domain -- only the UI test
        // passes it) drives synthetic fixes into `ingest` on a timer, to exercise the maneuver
        // banner countdown in the real UI. Guarded by phase/`driveSession` so a stale seam from a
        // previous drive can never ingest into a later one. The 2.5 s cadence is deliberate slack:
        // the test reads the countdown label only after several UI waits, so the fix train must
        // outlast them for a label CHANGE to still be observable -- a fast burst would be fully
        // consumed before the first read.
        if let raw = UserDefaults.standard.string(forKey: "simulatedDriveDistancesM") {
            let distancesM = raw.split(separator: ";").compactMap { Double($0) }
            let session = driveSession
            Task {
                for distanceM in distancesM {
                    guard phase == .driving, driveSession == session else { return }
                    if let coordinate = coordinate(atRouteDistanceM: distanceM) {
                        ingest(CLLocation(
                            coordinate: coordinate, altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5,
                            course: -1, speed: 15, timestamp: Date()
                        ))
                    }
                    try? await Task.sleep(nanoseconds: 2_500_000_000)
                }
            }
        }
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

        // wayfinder #67: rebuilt here, the same atomic swap point `hud` uses -- a banner index
        // can never survive a snapshot swap by construction.
        guidanceSteps = StepTracker.guidanceSteps(legs: plan.legs, stops: stopVMs, polyline: routePolyline, cumulativeM: routeCumulativeM)
        // wayfinder #68: mirrors the comment above -- voice bookkeeping is keyed to a step index
        // too, so it can't survive a snapshot swap either.
        voiceState = VoicePromptScheduler.State()
        banner = computeBanner()
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
        // wayfinder #68: no log entry -- the drive is over, the voiceEventLog's story ends at
        // the last fix.
        speech.stop()
        if let puckAnnotation { planStore.mapView.removeAnnotation(puckAnnotation) }
        puckAnnotation = nil
        planStore.mapView.showsUserLocation = true
        driveCardExpanded = false
        banner = nil
        phase = .idle
        if tripStore.phase == .recording { tripStore.stopTapped() }
        // 12V-safety gate (wayfinder #79): closed the moment the drive ends -- but AFTER capture
        // closes, because closing the gate wipes `latestReadings` and the end-of-trip telemetry
        // snapshot (wayfinder #80) must still be able to read them inside `stopTapped`.
        telemetryStore?.gateOpen = false
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

        // Actual SoC trail (wayfinder #83, design point 5): recorded only when the live OBD
        // reading is fresh -- same wall-clock freshness gate RootView's SoC-prompt auto-fill uses
        // (TelemetryLinkStore.snapshotFreshnessS) -- and thinned via
        // SoCChartModel.shouldAppendTrailPoint so a ~1 Hz fix stream doesn't flood the chart.
        if let telemetryStore, let liveSoc = telemetryStore.liveDisplaySoc, let lastReadingAt = telemetryStore.lastReadingAt,
           Date().timeIntervalSince(lastReadingAt) <= TelemetryLinkStore.snapshotFreshnessS {
            let candidate = SoCChartModel.SoCTrailPoint(distM: distanceAlongRouteM, socPct: liveSoc)
            if SoCChartModel.shouldAppendTrailPoint(lastKept: socTrail.last, candidate: candidate) {
                socTrail.append(candidate)
            }
        }

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
            banner = nil
            lastHudFixTimestamp = location.timestamp
            locationManager.stopUpdatingLocation()
            if tripStore.phase == .recording { tripStore.stopTapped() }
            // 12V-safety gate (wayfinder #79): arrival is as much an end as tapping End -- a
            // parked car must not keep the gate open through an indefinite `.arrived` dwell
            // (backoff retries would poke the sleeping car's adapter forever). After capture
            // closes, same ordering constraint as `end()`.
            telemetryStore?.gateOpen = false
            // Voice guidance (wayfinder #68): the arrive step's own `.now` tier at <=40 m races
            // this ~40 m arrival cutover and loses -- this branch returns before the scheduler
            // below ever runs -- so arrival speaks explicitly instead.
            voiceEventLog.append(.spoke("You have arrived"))
            speech.speak("You have arrived")
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

        // Maneuver banner (wayfinder #67): recomputed on EVERY fix, unthrottled -- the countdown
        // is the point. Placed after the off-route bookkeeping above so the muted state
        // (off-route-but-unreplanned) is current-fix-accurate.
        banner = computeBanner()

        // Voice guidance (wayfinder #68): shares the banner's mute gate exactly -- `banner` nil
        // means guidance is frozen (v1 pack, off-route-but-unreplanned, mid-replan, none left),
        // and voice must never speak into that state either.
        if banner != nil, let upcomingIdx = StepTracker.upcomingIndex(steps: guidanceSteps, distanceAlongRouteM: distanceAlongRouteM) {
            // Negative speed is invalid, same convention `location.course` uses above; fall back
            // to the current leg's average, and further to 15 m/s if that's zero (a zero-duration
            // leg).
            let legSpeedMPerS = legAvgSpeedMPerS.indices.contains(currentLegIndex) ? legAvgSpeedMPerS[currentLegIndex] : 0
            let effectiveSpeedMPerS = location.speed > 0 ? location.speed : (legSpeedMPerS > 0 ? legSpeedMPerS : 15)
            if let prompt = VoicePromptScheduler.nextPrompt(
                state: &voiceState, steps: guidanceSteps, upcomingIndex: upcomingIdx,
                distanceAlongRouteM: distanceAlongRouteM, speedMPerS: effectiveSpeedMPerS
            ) {
                voiceEventLog.append(.spoke(prompt.text))
                speech.speak(prompt.text)
            }
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

    // MARK: Drive replans (wayfinder #61 off-route, #63 manual SoC correction)

    /// Fires once a deviation has been sustained past `offRouteSustainedS`. Computes the
    /// departure SoC from the Plan's own predicted curve at the current position (model-driven,
    /// not the settings slider); the deviated fix is the replan origin.
    private func triggerOffRouteReplan(from location: CLLocation) {
        guard let drivePlan else { return }
        let departSoc = Self.interpolatedSoc(drivePlan.socCurve, at: distanceAlongRouteM)
        triggerDriveReplan(from: location.coordinate, departSoc: departSoc)
    }

    /// Manual mid-drive dash-SoC correction (wayfinder #63, ADR 0012 point 5's named follow-up):
    /// the driver's entered dash % replaces the model's estimate as the departure anchor -- the
    /// app's only truth anchor without car telemetry. Replans from `snappedCoordinate` (the puck:
    /// snapped while on-route, the raw fix beyond the snap tolerance); a no-op before the first
    /// fix lands or while another drive replan is in flight.
    func correctSoc(_ pct: Int) {
        guard phase == .driving, !replanInFlight, let coordinate = snappedCoordinate else { return }
        triggerDriveReplan(from: coordinate, departSoc: Double(min(max(pct, 0), 100)) / 100.0)
    }

    /// Shared replan-from-position body (off-route + manual correction): waypoints still ahead
    /// are kept -- snapped against the OLD `routePolyline` (the new one doesn't exist yet); a
    /// waypoint that fails to snap is kept rather than silently dropped. Adopts the landed plan
    /// on completion only if this drive session is still current and still driving -- see
    /// `driveSession`'s comment.
    private func triggerDriveReplan(from coordinate: CLLocationCoordinate2D, departSoc: Double) {
        // Voice guidance (wayfinder #68): hard-stop in-flight speech before the reroute -- the
        // research doc's mandated behavior. Logged unconditionally, like every voiceEventLog
        // entry.
        voiceEventLog.append(.stopped)
        speech.stop()

        replanInFlight = true
        offRouteSinceFixTimestamp = nil

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
                from: coordinate, keepingWaypointIds: keepingWaypointIds, departSoc: departSoc
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

    // MARK: Maneuver banner (wayfinder #67, ADR 0012 point 3)

    /// `nil` when `guidanceSteps` is empty (v1 pack, or none left), when guidance is muted
    /// (a sustained off-route excursion not yet replanned, or a replan in flight -- the
    /// displayed route may no longer be current), or when no upcoming step remains. Otherwise
    /// built from the upcoming `StepTracker.GuidanceStep`, with the live countdown distance.
    private func computeBanner() -> ManeuverBanner? {
        guard !guidanceSteps.isEmpty, offRouteSinceFixTimestamp == nil, !replanInFlight,
              let idx = StepTracker.upcomingIndex(steps: guidanceSteps, distanceAlongRouteM: distanceAlongRouteM)
        else { return nil }
        let step = guidanceSteps[idx]
        return ManeuverBanner(
            iconSystemName: step.iconSystemName, distanceM: max(0, step.distAlongRouteM - distanceAlongRouteM),
            primary: step.primary, secondary: step.secondary, then: step.then
        )
    }

    /// The polyline point at `targetM` along-route (wayfinder #67's e2e seam), interpolating
    /// within whichever segment `targetM` falls in. `nil` only when the polyline has fewer than
    /// 2 points (never true once a Plan is snapshotted).
    private func coordinate(atRouteDistanceM targetM: Double) -> CLLocationCoordinate2D? {
        guard routePolyline.count >= 2, let routeLenM = routeCumulativeM.last else { return nil }
        let clampedM = max(0, min(targetM, routeLenM))
        for i in 0..<(routePolyline.count - 1) {
            let segEndM = routeCumulativeM[i + 1]
            guard clampedM <= segEndM || i == routePolyline.count - 2 else { continue }
            let segStartM = routeCumulativeM[i]
            let segLenM = segEndM - segStartM
            let t = segLenM > 0 ? max(0, min(1, (clampedM - segStartM) / segLenM)) : 0
            let a = routePolyline[i]
            let b = routePolyline[i + 1]
            return CLLocationCoordinate2D(
                latitude: a.latitude + t * (b.latitude - a.latitude), longitude: a.longitude + t * (b.longitude - a.longitude)
            )
        }
        return routePolyline.last
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
