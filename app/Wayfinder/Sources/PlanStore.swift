// Owns the MLNMapView instance, the PlannerClient, and the pack/planner load state --
// salvaged from prototype/planner-ui's PlanStore (map plumbing: style loading with the
// PMTILES placeholder patch, the Chargers layer, route/stops layers, light/dark style swap,
// camera fit) plus, for this ticket (search + route editor #40), the route-editor state
// (origin/destination/Waypoints), the generation-guarded replan, and the CoreLocation
// bootstrap, and, for the arrival-card ticket (#43), the stop-free alternative toggle and the
// SoC-scrub marker sync (ported from prototype/planner-ui's PlanStore.updateScrubMarker()), and,
// for the settings sheet (#44), didSet-triggered replans on every planner-affecting request
// field plus the appearance override (persisted to UserDefaults), and,
// for the pack installer (#47), `region` becoming a persisted `activeRegion` with a
// `setActiveRegion(_:)` that resets route state and reloads, and the corridor-only origin gate
// generalizing to RegionBounds.swift's per-region table, and, for the refit acceptance UX
// (#53), `referenceConsumptionWhPerKm` becoming a third persisted setting and new calibration
// state (`calibrationResult`/`calibrationErrorMessage`/`calibrationDismissed`) recomputed via
// `PlannerClient.calibrate` whenever the settings sheet's Trip Logs list changes, and, for
// Drive Mode core (#59), `originIsCurrentLocation` (the Go gate's origin-provenance check) and
// `onUserMapGesture` (DriveStore's free-look trigger, fired from a new
// `shouldChangeFrom:to:reason:` delegate method that only trips on real gesture reasons) plus
// `mapView(_:imageFor:)` supplying the drive puck's custom annotation image.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit
import UIKit

/// One stop in the route editor: the destination or an ordered via-point (ADR 0010 point 4).
/// `CLLocationCoordinate2D` isn't `Equatable`, so this compares lat/lon manually.
struct EditorWaypoint: Identifiable, Equatable {
    let id = UUID()
    var name: String
    var coordinate: CLLocationCoordinate2D

    static func == (lhs: EditorWaypoint, rhs: EditorWaypoint) -> Bool {
        lhs.id == rhs.id && lhs.name == rhs.name
            && lhs.coordinate.latitude == rhs.coordinate.latitude
            && lhs.coordinate.longitude == rhs.coordinate.longitude
    }
}

// Swift 6 strict concurrency (wayfinder #47/#51 audit remediation, M-05 --
// docs/codebase-audit-2026-08-29.md): both delegate protocols declare nonisolated requirements,
// but this class is @MainActor. `@preconcurrency` conformance is sound here because delivery is
// genuinely on the main thread for both: MLNMapView calls its delegate on main, and
// CLLocationManager delivers callbacks on the runloop of the thread that started it -- main,
// since `locationManager` is a stored property initialized from this @MainActor class's `init`.
@MainActor
@Observable
final class PlanStore: NSObject, @preconcurrency MLNMapViewDelegate, @preconcurrency CLLocationManagerDelegate {
    enum PackStatus: Equatable {
        case missing
        case loaded(region: String)
    }

    enum PlannerStatus: Equatable {
        case idle
        case loading
        case ready
        case failed(String)
    }

    enum PlanStoreError: Error {
        case plannerNotReady
    }

    let mapView = MLNMapView(frame: .zero)

    private(set) var packStatus: PackStatus = .missing
    private(set) var plannerStatus: PlannerStatus = .idle
    private(set) var chargerCount = 0
    private(set) var isStyleLoaded = false
    private(set) var plan: FfiPlan?

    // MARK: Route editor state (wayfinder #40)

    private(set) var originCoordinate = CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319)
    private(set) var originName = "Luxembourg"
    private(set) var destination: EditorWaypoint?
    private(set) var waypoints: [EditorWaypoint] = []
    private(set) var isPlanning = false
    /// ADR 0012 point 2's Drive Mode Go gate (wayfinder #59): the origin was adopted from a
    /// location fix and hasn't since been overridden by a long-press -- provenance, not just
    /// "does the origin currently match a fix".
    var originIsCurrentLocation: Bool { hasSetOriginFromLocationFix && !originOverridden }
    /// DriveStore's free-look trigger (wayfinder #59): fired from `shouldChangeFrom:to:reason:`
    /// below on any real map gesture while driving. Set by DriveStore's init, nil otherwise.
    var onUserMapGesture: (() -> Void)?
    /// Bumped on every replan error, even a repeated identical one, so RootView's
    /// onChange(planErrorVersion) fires every time (a plain onChange(planErrorMessage) would
    /// miss back-to-back identical errors).
    private(set) var planErrorMessage: String?
    private(set) var planErrorVersion = 0
    /// Bumped on every landed plan; RootView uses it to fit the camera.
    private(set) var planVersion = 0

    // Plan request fields, bound to the settings sheet (wayfinder #44). Each planner-affecting
    // field replans on change via didSet, guarded against no-op sets so a continuous slider
    // drag landing back on its own value doesn't fire a redundant replan.
    var departSoc = 0.90 {
        didSet { guard oldValue != departSoc else { return }; replan() }
    }
    var arrivalMinSoc = 0.10 {
        didSet { guard oldValue != arrivalMinSoc else { return }; replan() }
    }
    var chargerArrivalMinSoc = 0.10 {
        didSet { guard oldValue != chargerArrivalMinSoc else { return }; replan() }
    }
    var chargerMaxSoc = 0.80 {
        didSet { guard oldValue != chargerMaxSoc else { return }; replan() }
    }
    var stopsBias = 1.0 {
        didSet { guard oldValue != stopsBias else { return }; replan() }
    }
    var tempC = 20.0 {
        didSet { guard oldValue != tempC else { return }; replan() }
    }
    var headwindMs = 0.0 {
        didSet { guard oldValue != headwindMs else { return }; replan() }
    }
    var batteryWarmth = 1.0
    var offerStopFreeAlternative = true
    var vehicle: FfiVehicle = .ioniq5Lr2wd
    /// nil = vehicle default reference consumption; the settings sheet's Toggle sets/clears
    /// this, and so does `acceptCalibration()` (wayfinder #53). Persisted to UserDefaults like
    /// `appearanceOverride`/`activeRegion` below: read once at init, written here on every change.
    var referenceConsumptionWhPerKm: Double? {
        didSet {
            guard oldValue != referenceConsumptionWhPerKm else { return }
            if let referenceConsumptionWhPerKm {
                UserDefaults.standard.set(referenceConsumptionWhPerKm, forKey: Self.referenceConsumptionWhPerKmKey)
            } else {
                UserDefaults.standard.removeObject(forKey: Self.referenceConsumptionWhPerKmKey)
            }
            replan()
        }
    }
    private static let referenceConsumptionWhPerKmKey = "referenceConsumptionWhPerKm"

    // MARK: Settings sheet state (wayfinder #44)

    /// Whether the settings sheet is presented. Lives here, not as View @State, so
    /// settings-smoke can drive it directly, like `cardExpanded`.
    var showingSettings = false

    /// "system" / "light" / "dark", persisted to UserDefaults (read once at init) so it
    /// survives relaunch -- the only setting in this ticket that persists (others have joined it
    /// since; see PlanStore.swift header). `updateAppearance(systemDark:)` combines it with the
    /// live system scheme.
    var appearanceOverride: String {
        didSet {
            guard oldValue != appearanceOverride else { return }
            UserDefaults.standard.set(appearanceOverride, forKey: Self.appearanceOverrideKey)
            setAppearance(dark: effectiveDark)
        }
    }
    private static let appearanceOverrideKey = "appearanceOverride"
    /// The system color scheme as last reported by RootView's onChange(of: colorScheme).
    private var systemIsDark = false
    private var effectiveDark: Bool {
        appearanceOverride == "dark" || (appearanceOverride == "system" && systemIsDark)
    }

    // MARK: Result card state (wayfinder #43)

    /// Whether the result card shows the itinerary + SoC chart. Lives here, not as View
    /// @State, so the card-smoke autotest can drive it directly.
    var cardExpanded = false
    /// Whether the stop-free alternative (ADR 0010 point 5) is displayed in place of the main
    /// plan -- reset to false whenever a new plan lands. Toggle via `toggleAlternative()`.
    private(set) var showingAlternative = false
    /// Distance (m) selected by dragging on the SoC chart; moves `scrubAnnotation` on the map.
    /// Cleared whenever a new plan lands or the alternative is toggled.
    var selectedDistanceM: Double? {
        didSet { updateScrubMarker() }
    }

    /// The plan the card, chart, scrub marker, and map route/stop layers all render: the main
    /// plan, or, when `showingAlternative`, an `FfiPlan` built from `FfiPlanAlt`'s fields
    /// (which are the same as `FfiPlan` minus `alternative`).
    var displayedPlan: FfiPlan? {
        guard let plan else { return nil }
        guard showingAlternative, let alt = plan.alternative else { return plan }
        return FfiPlan(
            legs: alt.legs, stops: alt.stops, driveTimeS: alt.driveTimeS, chargeTimeS: alt.chargeTimeS,
            totalTimeS: alt.totalTimeS, totalDistM: alt.totalDistM, flags: alt.flags,
            socCurve: alt.socCurve, polyline: alt.polyline, alternative: nil
        )
    }

    private var generation = 0
    /// Bumped in `setActiveRegion` and captured in `load()` before its detached Task starts
    /// (wayfinder #47 audit remediation, H-02 -- docs/codebase-audit-2026-08-29.md): if region
    /// A's load is still in flight when the user switches to region B, A's Task.detached keeps
    /// running (there's no cancellation plumbing here, only this guard) and can complete after
    /// B's own load has already landed. Correctness rests entirely on `didLoad`/`didFail`
    /// dropping any result whose captured `gen` no longer matches `loadGeneration` -- same
    /// pattern as `replan()`'s `generation` guard below, just for pack loads instead of plan
    /// requests; the two are deliberately separate counters since a route edit shouldn't
    /// invalidate an in-flight pack load or vice versa.
    private var loadGeneration = 0
    private var originOverridden = false
    private var hasSetOriginFromLocationFix = false
    private var originAnnotation: MLNPointAnnotation?
    private var scrubAnnotation: MLNPointAnnotation?
    private let locationManager = CLLocationManager()

    private var client: PlannerClient?
    private var located: Packs.Located?
    private var chargersForMap: [CPack1Charger]?
    // Read externally by settings-smoke (wayfinder #44) to check the effective appearance
    // actually applied to the map, as distinct from the override setting alone.
    private(set) var isDarkAppearance = false
    private var hasStartedLoad = false
    private var hasSetInitialCamera = false

    /// The persisted active region (wayfinder #47), UserDefaults key `activeRegion`, default
    /// "corridor" -- `setActiveRegion(_:)` is the only way to change it.
    private(set) var activeRegion: String
    private static let activeRegionKey = "activeRegion"

    override init() {
        appearanceOverride = UserDefaults.standard.string(forKey: Self.appearanceOverrideKey) ?? "system"
        activeRegion = UserDefaults.standard.string(forKey: Self.activeRegionKey) ?? "corridor"
        referenceConsumptionWhPerKm = UserDefaults.standard.object(forKey: Self.referenceConsumptionWhPerKmKey) as? Double
        super.init()
        mapView.delegate = self
        mapView.showsUserLocation = true
        locationManager.delegate = self
    }

    /// Persists the new active region, resets route state (a plan/stops belong to the previous
    /// region's pack), and re-runs `load()` against it.
    func setActiveRegion(_ region: String) {
        guard region != activeRegion else { return }
        activeRegion = region
        UserDefaults.standard.set(region, forKey: Self.activeRegionKey)

        destination = nil
        waypoints = []
        plan = nil
        showingAlternative = false
        selectedDistanceM = nil
        generation += 1
        loadGeneration += 1

        hasStartedLoad = false
        located = nil
        client = nil
        chargersForMap = nil
        chargerCount = 0
        plannerStatus = .idle
        packStatus = .missing

        load()
    }

    func load() {
        guard !hasStartedLoad else { return }
        hasStartedLoad = true

        // UI-e2e seam (wayfinder #59): `-simulatedLocationFix "49.6116,6.1319"` launch argument.
        // XCUITest cannot inject CoreLocation fixes, and the suite must be self-contained -- the
        // real adoption path (via the CLLocationManagerDelegate callback below) is covered by
        // drive-smoke and by the M4-gate device drives instead.
        if let fixString = UserDefaults.standard.string(forKey: "simulatedLocationFix") {
            let parts = fixString.split(separator: ",")
            if parts.count == 2, let lat = Double(parts[0]), let lon = Double(parts[1]) {
                adoptLocationFixAsOriginIfEligible(CLLocationCoordinate2D(latitude: lat, longitude: lon))
            }
        }

        guard let located = Packs.locate(region: activeRegion) else {
            packStatus = .missing
            return
        }
        self.located = located
        packStatus = .loaded(region: activeRegion)
        plannerStatus = .loading
        applyStyle(located: located)

        let rpackPath = located.rpackURL.path
        let chargersURL = located.chargersURL
        let gen = loadGeneration
        Task.detached { [weak self] in
            do {
                let client = try PlannerClient(regionPackPath: rpackPath)
                let bytes = try Data(contentsOf: chargersURL)
                try client.loadChargers(bytes: bytes, format: "cpack-1")
                let chargers = try CPack1.parseChargers(data: bytes)
                await self?.didLoad(client: client, chargers: chargers, gen: gen)
            } catch {
                await self?.didFail(error: error, gen: gen)
            }
        }
    }

    private func didLoad(client: PlannerClient, chargers: [CPack1Charger], gen: Int) {
        guard gen == loadGeneration else { return }
        self.client = client
        chargersForMap = chargers
        chargerCount = chargers.count
        plannerStatus = .ready
        addChargersLayerIfPossible()
    }

    private func didFail(error: Error, gen: Int) {
        guard gen == loadGeneration else { return }
        plannerStatus = .failed(String(describing: error))
    }

    // MARK: Appearance (light/dark style)

    /// Called from RootView on appear and whenever the SwiftUI color scheme changes -- RootView
    /// keeps reporting the system scheme; combined here with `appearanceOverride` (wayfinder
    /// #44) into the effective appearance, applied through `setAppearance(dark:)`'s existing
    /// no-op guard.
    func updateAppearance(systemDark: Bool) {
        systemIsDark = systemDark
        setAppearance(dark: effectiveDark)
    }

    /// Swaps the map style only when the appearance actually changed; `didFinishLoading`
    /// re-adds the chargers/route/stops layers once the new style finishes loading.
    func setAppearance(dark: Bool) {
        guard dark != isDarkAppearance else { return }
        isDarkAppearance = dark
        guard let located else { return } // load() hasn't run yet -- it applies isDarkAppearance itself
        applyStyle(located: located)
    }

    private func applyStyle(located: Packs.Located) {
        let styleFile = isDarkAppearance ? located.styleDarkURL : located.styleLightURL
        guard let styleURL = MapStyle.patchedStyleURL(pmtilesURL: located.pmtilesURL, styleURL: styleFile) else {
            return
        }
        // Reset so callers (settings-smoke) can wait on the false -> true transition to know
        // the swapped style, not the previous one, has finished loading.
        isStyleLoaded = false
        mapView.styleURL = styleURL
    }

    // MARK: Planning

    /// Runs a plan request and draws the route + Charging Stop layers, bypassing the route
    /// editor state entirely. RootView drives plans through `replan()` instead -- only the
    /// map-smoke autotest calls this directly, to prove the map layers against the golden
    /// LU -> Amsterdam plan.
    @discardableResult
    func runPlan(_ request: FfiPlanRequest) async throws -> FfiPlan {
        guard let client else { throw PlanStoreError.plannerNotReady }
        let plan = try await client.plan(request)
        self.plan = plan
        if let style = mapView.style {
            RouteLayer.addLayers(to: style, plan: plan)
            RouteLayer.fitToRoute(mapView: mapView, plan: plan)
        }
        return plan
    }

    // MARK: Result card mutations (wayfinder #43)

    /// Flips between the main plan and its stop-free alternative (ADR 0010 point 5). The
    /// scrub selection doesn't carry over -- the two plans' SoC curves differ -- and the route
    /// layers are redrawn from `displayedPlan` with no camera refit (Google Maps doesn't
    /// re-fit for this either).
    func toggleAlternative() {
        showingAlternative.toggle()
        selectedDistanceM = nil
        if let style = mapView.style, let displayedPlan {
            RouteLayer.addLayers(to: style, plan: displayedPlan)
        }
    }

    /// Moves (or creates) the scrub marker: nearest socCurve sample by distance -> nearest
    /// polyline point by walked (haversine) distance. Adapted from prototype/planner-ui's
    /// PlanStore.updateScrubMarker(), which found the polyline point by fraction of total
    /// distance instead -- that assumes the polyline's vertices are evenly spaced by distance,
    /// which doesn't hold here: contraction-hierarchy shortcuts (wayfinder #31) unpack to
    /// sparse geometry on highway stretches and dense geometry through cities, so a
    /// uniform-fraction guess can land several km off the route. Walking real distance instead
    /// keeps the approximation to the SoC curve's own ~2km sampling resolution, as intended.
    private func updateScrubMarker() {
        guard let plan = displayedPlan, let distM = selectedDistanceM,
              !plan.socCurve.isEmpty, plan.polyline.count > 1, plan.totalDistM > 0
        else {
            if let scrubAnnotation { mapView.removeAnnotation(scrubAnnotation) }
            scrubAnnotation = nil
            return
        }
        let nearest = plan.socCurve.min { abs($0.distM - distM) < abs($1.distM - distM) }!

        var accumulatedM = 0.0
        var coordinate = CLLocationCoordinate2D(latitude: plan.polyline[0].lat, longitude: plan.polyline[0].lon)
        for i in 1..<plan.polyline.count {
            let previous = plan.polyline[i - 1]
            let current = plan.polyline[i]
            coordinate = CLLocationCoordinate2D(latitude: current.lat, longitude: current.lon)
            accumulatedM += CLLocation(latitude: previous.lat, longitude: previous.lon)
                .distance(from: CLLocation(latitude: current.lat, longitude: current.lon))
            if accumulatedM >= nearest.distM { break }
        }

        if let scrubAnnotation {
            scrubAnnotation.coordinate = coordinate
        } else {
            let annotation = MLNPointAnnotation()
            annotation.coordinate = coordinate
            annotation.title = "Scrub"
            mapView.addAnnotation(annotation)
            scrubAnnotation = annotation
        }
        mapView.setCenter(coordinate, animated: true)
    }

    // MARK: Calibration (wayfinder #53)

    /// The refit-and-accept flow's result of the last `PlannerClient.calibrate` call, or nil
    /// before the first one lands (or after an empty Trip Log set clears it).
    private(set) var calibrationResult: FfiCalibrationResult?
    /// Set on a failed `calibrate` call (e.g. a corrupt log -- `PlannerError.InvalidRequest`
    /// names the offending log index) and cleared on the next success. Corrupt logs failing
    /// loudly rather than being silently skipped is deliberate (ADR 0009 audit ethos).
    private(set) var calibrationErrorMessage: String?
    /// Hides the proposal row in SettingsForm's Calibration section without discarding
    /// `calibrationResult` itself -- set by `acceptCalibration()`/`dismissCalibration()`, reset
    /// whenever the Trip Log set changes.
    private(set) var calibrationDismissed = false
    private var calibrationGeneration = 0
    /// The (sorted) log URL set `calibrationResult`/`calibrationDismissed` belong to -- lets
    /// `refreshCalibration` tell a genuinely new log set (which should reset the dismissal)
    /// apart from a redundant re-run over the same one.
    private var calibrationLogURLs: [URL] = []

    /// Recomputes calibration for `logURLs` (SettingsForm's Trip Logs list) via
    /// `PlannerClient.calibrate` -- ADR 0009's refit-and-accept flow. Called from
    /// SettingsForm.onAppear and after every Trip Log add/delete. Generation-guarded like
    /// `replan()`: a superseded call's result or error is dropped.
    func refreshCalibration(logURLs: [URL]) {
        let sortedURLs = logURLs.sorted { $0.path < $1.path }
        if sortedURLs != calibrationLogURLs {
            calibrationDismissed = false
            calibrationLogURLs = sortedURLs
        }

        guard !sortedURLs.isEmpty else {
            calibrationResult = nil
            calibrationErrorMessage = nil
            return
        }
        guard plannerStatus == .ready, let client else { return }

        calibrationGeneration += 1
        let gen = calibrationGeneration
        let vehicle = vehicle
        let referenceConsumptionWhPerKm = referenceConsumptionWhPerKm

        Task {
            do {
                let result = try await Self.readLogsAndCalibrate(
                    client: client, urls: sortedURLs, vehicle: vehicle,
                    referenceConsumptionWhPerKm: referenceConsumptionWhPerKm
                )
                guard gen == self.calibrationGeneration else { return }
                calibrationResult = result
                calibrationErrorMessage = nil
            } catch {
                guard gen == self.calibrationGeneration else { return }
                calibrationErrorMessage = "Calibration failed: \(error)"
                calibrationResult = nil
            }
        }
    }

    /// Reads each log file's JSON text and calls `PlannerClient.calibrate` -- `nonisolated` so
    /// `await`ing it from `refreshCalibration` hops off the main actor for the (small but
    /// synchronous) file reads too, not just for `calibrate`'s own already-detached Rust call.
    nonisolated private static func readLogsAndCalibrate(
        client: PlannerClient, urls: [URL], vehicle: FfiVehicle, referenceConsumptionWhPerKm: Double?
    ) async throws -> FfiCalibrationResult {
        let logs = try urls.map { try String(contentsOf: $0, encoding: .utf8) }
        return try await client.calibrate(logs: logs, vehicle: vehicle, referenceConsumptionWhPerKm: referenceConsumptionWhPerKm)
    }

    /// Applies the proposed refit as the new Reference Consumption override, clamped to the
    /// settings sheet's Stepper range (120...260 Wh/km -- SettingsForm.swift) since a refit
    /// outside that range couldn't be represented there anyway. `referenceConsumptionWhPerKm`'s
    /// existing didSet persists it and replans.
    func acceptCalibration() {
        guard let result = calibrationResult else { return }
        referenceConsumptionWhPerKm = min(max(result.referenceConsumptionWhPerKm, 120), 260)
        calibrationDismissed = true
    }

    /// Hides the proposal row without discarding the metric display.
    func dismissCalibration() {
        calibrationDismissed = true
    }

    // MARK: MLNMapViewDelegate

    /// DriveStore's free-look trigger (wayfinder #59, ADR 0012 point 3): always allows the
    /// change, but calls `onUserMapGesture` when `reason` is a real user gesture -- never for
    /// `.programmatic`, which is what DriveStore's own following/overview camera moves use, so
    /// they can't cancel following back out of itself.
    func mapView(
        _ mapView: MLNMapView, shouldChangeFrom oldCamera: MLNMapCamera, to newCamera: MLNMapCamera,
        reason: MLNCameraChangeReason
    ) -> Bool {
        let gestureReasons: MLNCameraChangeReason = [
            .gesturePan, .gesturePinch, .gestureRotate, .gestureZoomIn, .gestureZoomOut,
            .gestureOneFingerZoom, .gestureTilt,
        ]
        if !reason.isDisjoint(with: gestureReasons) {
            onUserMapGesture?()
        }
        return true
    }

    /// Custom puck image for Drive Mode's "drive-puck" annotation (wayfinder #59); every other
    /// annotation (origin pin, scrub marker) keeps the default pin by returning nil.
    func mapView(_ mapView: MLNMapView, imageFor annotation: MLNAnnotation) -> MLNAnnotationImage? {
        guard annotation.title ?? "" == "drive-puck" else { return nil }
        return Self.drivePuckImage
    }

    private static let drivePuckImage: MLNAnnotationImage = {
        let size = CGSize(width: 30, height: 30)
        let renderer = UIGraphicsImageRenderer(size: size)
        let image = renderer.image { context in
            let rect = CGRect(origin: .zero, size: size).insetBy(dx: 2, dy: 2)
            let circle = UIBezierPath(ovalIn: rect)
            UIColor.systemBlue.setFill()
            circle.fill()
            UIColor.white.setStroke()
            circle.lineWidth = 2
            circle.stroke()

            let triangle = UIBezierPath()
            triangle.move(to: CGPoint(x: size.width / 2, y: 7))
            triangle.addLine(to: CGPoint(x: size.width - 9, y: size.height - 9))
            triangle.addLine(to: CGPoint(x: 9, y: size.height - 9))
            triangle.close()
            UIColor.white.setFill()
            triangle.fill()
        }
        return MLNAnnotationImage(image: image, reuseIdentifier: "drive-puck")
    }()

    func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
        isStyleLoaded = true
        addChargersLayerIfPossible()
        if let plan {
            RouteLayer.addLayers(to: style, plan: plan)
        }
        setInitialCameraIfNeeded()
    }

    /// On plain launch there's no destination yet, so no route to fit the camera to --
    /// center on the pack's Luxembourg corridor at a zoom that shows charger dots
    /// (ChargersLayer's minimumZoomLevel is 8) instead of leaving the default world view.
    private func setInitialCameraIfNeeded() {
        guard !hasSetInitialCamera else { return }
        hasSetInitialCamera = true
        let luxembourg = CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319)
        mapView.setCenter(luxembourg, zoomLevel: 10, animated: false)
    }

    private func addChargersLayerIfPossible() {
        guard let style = mapView.style, let chargersForMap else { return }
        guard style.source(withIdentifier: ChargersLayer.sourceId) == nil else { return }
        guard let source = ChargersLayer.makeSource(chargers: chargersForMap) else { return }
        ChargersLayer.addLayers(to: style, source: source)
    }

    // MARK: Route editor mutations (wayfinder #40)

    /// Sets the destination from a search selection and replans. `destination` starts nil,
    /// so no plan runs until this is called at least once.
    func setDestination(name: String, coordinate: CLLocationCoordinate2D) {
        destination = EditorWaypoint(name: name, coordinate: coordinate)
        replan()
    }

    /// Sets the origin from a long-press on the map, dropping or moving a pin annotation
    /// there, and replans.
    func setOrigin(_ coordinate: CLLocationCoordinate2D) {
        originOverridden = true
        originCoordinate = coordinate
        originName = "Dropped pin"
        if let originAnnotation {
            originAnnotation.coordinate = coordinate
        } else {
            let annotation = MLNPointAnnotation()
            annotation.coordinate = coordinate
            annotation.title = "Origin"
            mapView.addAnnotation(annotation)
            originAnnotation = annotation
        }
        replan()
    }

    /// Appends an ordered via-point (ADR 0010 point 4) and replans.
    func addWaypoint(name: String, coordinate: CLLocationCoordinate2D) {
        waypoints.append(EditorWaypoint(name: name, coordinate: coordinate))
        replan()
    }

    func removeWaypoint(id: EditorWaypoint.ID) {
        waypoints.removeAll { $0.id == id }
        replan()
    }

    /// Offset-based removal for the route editor's native edit-mode delete controls
    /// (`.onDelete`); editor-smoke keeps driving the id-based mutation above directly.
    func removeWaypoints(atOffsets offsets: IndexSet) {
        waypoints.remove(atOffsets: offsets)
        replan()
    }

    func moveWaypoints(fromOffsets: IndexSet, toOffset: Int) {
        waypoints.move(fromOffsets: fromOffsets, toOffset: toOffset)
        replan()
    }

    /// Runs on a fix inside the active region's bounds (RegionBounds.swift), once, unless the
    /// origin was already overridden by a long-press -- adopts it as the origin and replans if
    /// a destination is already set. editor-smoke pins the origin via setOrigin before loading,
    /// so adoption never affects it; on a plain launch inside the region the fix is adopted as
    /// intended.
    private func adoptLocationFixAsOriginIfEligible(_ coordinate: CLLocationCoordinate2D) {
        let box = RegionBounds.box(for: activeRegion)
        guard !originOverridden, !hasSetOriginFromLocationFix,
              box.latRange.contains(coordinate.latitude),
              box.lonRange.contains(coordinate.longitude)
        else { return }
        hasSetOriginFromLocationFix = true
        originCoordinate = coordinate
        originName = "Your location"
        if destination != nil { replan() }
    }

    /// Google-style "locate me": only pans the camera, doesn't touch the origin.
    func centerOnUser() {
        guard let coordinate = mapView.userLocation?.location?.coordinate else { return }
        mapView.setCenter(coordinate, animated: true)
    }

    /// Replans from the current route editor state. Every route edit calls this; each call
    /// cancels the previous in-flight plan and lands only if it's still the latest. Thin wrapper
    /// over `runGuardedReplan` -- fire-and-forget, since no editor call site awaits a plan.
    private func replan() {
        Task { _ = await runGuardedReplan() }
    }

    /// ADR 0012 point 6's replan-from-position entry (wayfinder #61): called by DriveStore when
    /// a sustained off-route deviation calls for a silent replan. `coordinate` (the deviated
    /// fix) becomes the new origin, adopted the same way a location fix normally is --
    /// `originOverridden` is reset to false, not set true, because the deviated position IS the
    /// current location, so the Go gate (`originIsCurrentLocation`) stays truthful after End.
    /// `waypoints` is filtered down to `keepingWaypointIds` (stops not yet reached); the
    /// destination is untouched. `departSoc` is passed through as `runGuardedReplan`'s override,
    /// deliberately NOT written to the settings-bound `departSoc` property -- that slider must
    /// keep reflecting the user's configured start-of-trip SoC, not the model's live estimate.
    func replanForDrive(
        from coordinate: CLLocationCoordinate2D, keepingWaypointIds: Set<EditorWaypoint.ID>, departSoc: Double
    ) async -> FfiPlan? {
        originCoordinate = coordinate
        originName = "Your location"
        hasSetOriginFromLocationFix = true
        originOverridden = false
        waypoints = waypoints.filter { keepingWaypointIds.contains($0.id) }
        return await runGuardedReplan(departSocOverride: departSoc)
    }

    /// IMPORTANT: `client.cancel()` is a latency optimisation only, not a correctness
    /// mechanism. `Planner::plan` resets the cancel flag at entry (core/ffi/src/planner.rs),
    /// so a superseded call can still COMPLETE normally if it never observes the flag before
    /// the next call resets it -- including racing back in as a stale success after a newer
    /// call has already landed. Correctness rests entirely on the generation guard below: any
    /// result or error arriving with `gen != generation` is dropped, including a superseded
    /// call's own `Cancelled` error. Returns the landed `FfiPlan` on success, `nil` on failure or
    /// when superseded before landing.
    private func runGuardedReplan(departSocOverride: Double? = nil) async -> FfiPlan? {
        guard let destination, plannerStatus == .ready, let client else { return nil }

        generation += 1
        let gen = generation
        client.cancel()
        isPlanning = true

        let request = FfiPlanRequest(
            originLat: originCoordinate.latitude, originLon: originCoordinate.longitude,
            destLat: destination.coordinate.latitude, destLon: destination.coordinate.longitude,
            waypoints: waypoints.map {
                FfiWaypoint(lat: $0.coordinate.latitude, lon: $0.coordinate.longitude, departSocOverride: nil)
            },
            departSoc: departSocOverride ?? departSoc,
            arrivalMinSoc: arrivalMinSoc,
            chargerArrivalMinSoc: chargerArrivalMinSoc,
            chargerMaxSoc: chargerMaxSoc,
            stopsBias: stopsBias,
            tempC: tempC,
            headwindMs: headwindMs,
            batteryWarmth: batteryWarmth,
            offerStopFreeAlternative: offerStopFreeAlternative,
            vehicle: vehicle,
            referenceConsumptionWhPerKm: referenceConsumptionWhPerKm
        )

        do {
            let result = try await client.plan(request)
            guard gen == self.generation else { return nil }
            self.plan = result
            showingAlternative = false
            selectedDistanceM = nil
            if let style = mapView.style {
                RouteLayer.addLayers(to: style, plan: result)
            }
            planVersion += 1
            isPlanning = false
            return result
        } catch {
            guard gen == self.generation else { return nil }
            // Leave the previous plan and its layers on screen.
            planErrorMessage = Self.userMessage(for: error)
            planErrorVersion += 1
            isPlanning = false
            return nil
        }
    }

    private static func userMessage(for error: Error) -> String {
        guard let plannerError = error as? PlannerError else { return "Planning failed" }
        switch plannerError {
        case .NoRouteFound: return "No route — outside pack region?"
        case .PackMissing: return "Planner not ready"
        case .InvalidRequest: return "Invalid route request"
        case .Cancelled: return "Cancelled"
        }
    }

    // MARK: CoreLocation (wayfinder #40)

    /// Called from RootView.onAppear. Requesting again once already authorized/denied is a
    /// no-op.
    func requestLocationPermission() {
        locationManager.requestWhenInUseAuthorization()
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        switch manager.authorizationStatus {
        case .authorizedWhenInUse, .authorizedAlways:
            manager.startUpdatingLocation()
        default:
            break
        }
    }

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        guard let coordinate = locations.last?.coordinate else { return }
        adoptLocationFixAsOriginIfEligible(coordinate)
    }
}
