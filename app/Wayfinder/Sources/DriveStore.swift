// Drive Mode core (wayfinder #59, ADR 0012 points 1-3 and 8): a Go/drive/End state machine
// entered from the result card's Go button, driving a heading-up following camera and a
// route-snapped puck off real CLLocationManager fixes. No HUD data surface, no off-route
// replan, no Trip Log wiring -- those are later tickets (#60/#61/#67 consume
// `distanceAlongRouteM`, which is produced here for exactly that reason). Owns its OWN
// CLLocationManager, same reasoning as TripLogStore.swift's header: a continuous drive trace
// is a different concern from PlanStore's route-editor origin adoption, and PlanStore's
// accuracy/background settings must not change for this.
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

    /// ADR 0012 point 2's Go gate: origin provenance (adopted from a location fix, not
    /// overridden since) and a plan actually on screen.
    var canGo: Bool {
        phase == .idle && planStore.displayedPlan != nil && planStore.originIsCurrentLocation
    }

    private let planStore: PlanStore
    private let locationManager = CLLocationManager()
    private var routePolyline: [CLLocationCoordinate2D] = []
    private var routeCumulativeM: [Double] = []
    private var lastSegmentIndex: Int?
    private var puckAnnotation: MLNPointAnnotation?

    init(planStore: PlanStore) {
        self.planStore = planStore
        super.init()
        locationManager.delegate = self
        planStore.onUserMapGesture = { [weak self] in self?.noteUserGesture() }
    }

    // MARK: Lifecycle

    /// Snapshots the displayed Plan's polyline at entry. A replan landing mid-drive can't
    /// diverge the drive in this slice -- the planning UI is hidden while driving, and
    /// off-route replans are ticket #61 -- so a stale snapshot is fine here.
    func go() {
        guard canGo, let plan = planStore.displayedPlan else { return }

        routePolyline = plan.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        routeCumulativeM = RouteSnap.cumulativeDistances(routePolyline)
        lastSegmentIndex = nil
        snappedCoordinate = nil
        isOnRoute = false
        distanceAlongRouteM = 0
        smoothedCourseDeg = 0

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

    /// The Plan and its map layers stay intact -- planning UI just returns, via RootView's
    /// phase switch.
    func end() {
        guard phase == .driving else { return }
        locationManager.stopUpdatingLocation()
        if let puckAnnotation { planStore.mapView.removeAnnotation(puckAnnotation) }
        puckAnnotation = nil
        planStore.mapView.showsUserLocation = true
        phase = .idle
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

        if result.distanceFromRouteM <= 10 {
            snappedCoordinate = result.coordinate
            isOnRoute = true
        } else {
            snappedCoordinate = location.coordinate
            isOnRoute = false
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
