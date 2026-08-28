// Owns the MLNMapView instance, the PlannerClient, and the pack/planner load state --
// salvaged from prototype/planner-ui's PlanStore (map plumbing: style loading with the
// PMTILES placeholder patch, the Chargers layer, route/stops layers, light/dark style swap,
// camera fit) plus, for this ticket (search + route editor #40), the route-editor state
// (origin/destination/Waypoints), the generation-guarded replan, and the CoreLocation
// bootstrap. Skipped for later tickets: the SoC-scrub marker sync (arrival-card ticket).
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

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

/// Roughly lat 49.4-53.6, lon 2.5-7.3 (Benelux corridor) -- a location fix outside this box
/// is ignored for origin purposes. SearchModel.swift's `corridorRegion` covers the same
/// area for MKLocalSearchCompleter's suggestion bias.
private let corridorLatRange = 49.4...53.6
private let corridorLonRange = 2.5...7.3

@MainActor
@Observable
final class PlanStore: NSObject, MLNMapViewDelegate, CLLocationManagerDelegate {
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
    /// Bumped on every replan error, even a repeated identical one, so RootView's
    /// onChange(planErrorVersion) fires every time (a plain onChange(planErrorMessage) would
    /// miss back-to-back identical errors).
    private(set) var planErrorMessage: String?
    private(set) var planErrorVersion = 0
    /// Bumped on every landed plan; RootView uses it to fit the camera.
    private(set) var planVersion = 0

    // Fixed plan request defaults for this ticket (later tickets bind these to UI).
    var departSoc = 0.90
    var arrivalMinSoc = 0.10
    var chargerArrivalMinSoc = 0.10
    var chargerMaxSoc = 0.80
    var stopsBias = 1.0
    var tempC = 20.0
    var headwindMs = 0.0
    var batteryWarmth = 1.0
    var offerStopFreeAlternative = false
    var vehicle: FfiVehicle = .ioniq5Lr2wd
    var referenceConsumptionWhPerKm: Double?

    private var generation = 0
    private var originOverridden = false
    private var hasSetOriginFromLocationFix = false
    private var originAnnotation: MLNPointAnnotation?
    private let locationManager = CLLocationManager()

    private var client: PlannerClient?
    private var located: Packs.Located?
    private var chargersForMap: [CPack1Charger]?
    private var isDarkAppearance = false
    private var hasStartedLoad = false
    private var hasSetInitialCamera = false

    private let region = "corridor"

    override init() {
        super.init()
        mapView.delegate = self
        mapView.showsUserLocation = true
        locationManager.delegate = self
    }

    func load() {
        guard !hasStartedLoad else { return }
        hasStartedLoad = true

        guard let located = Packs.locate(region: region) else {
            packStatus = .missing
            return
        }
        self.located = located
        packStatus = .loaded(region: region)
        plannerStatus = .loading
        applyStyle(located: located)

        let rpackPath = located.rpackURL.path
        let chargersURL = located.chargersURL
        Task.detached { [weak self] in
            do {
                let client = try PlannerClient(regionPackPath: rpackPath)
                let bytes = try Data(contentsOf: chargersURL)
                try client.loadChargers(bytes: bytes, format: "cpack-1")
                let chargers = try CPack1.parseChargers(data: bytes)
                await self?.didLoad(client: client, chargers: chargers)
            } catch {
                await self?.didFail(error: error)
            }
        }
    }

    private func didLoad(client: PlannerClient, chargers: [CPack1Charger]) {
        self.client = client
        chargersForMap = chargers
        chargerCount = chargers.count
        plannerStatus = .ready
        addChargersLayerIfPossible()
    }

    private func didFail(error: Error) {
        plannerStatus = .failed(String(describing: error))
    }

    // MARK: Appearance (light/dark style)

    /// Called from RootView on appear and whenever the SwiftUI color scheme changes. Swaps
    /// the map style only when the appearance actually changed; `didFinishLoading` re-adds
    /// the chargers/route/stops layers once the new style finishes loading.
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

    // MARK: MLNMapViewDelegate

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

    func moveWaypoints(fromOffsets: IndexSet, toOffset: Int) {
        waypoints.move(fromOffsets: fromOffsets, toOffset: toOffset)
        replan()
    }

    /// Runs on a fix inside the corridor bounds, once, unless the origin was already
    /// overridden by a long-press -- adopts it as the origin and replans if a destination is
    /// already set. editor-smoke pins the origin via setOrigin before loading, so adoption
    /// never affects it; on a plain launch inside the corridor the fix is adopted as
    /// intended.
    private func adoptLocationFixAsOriginIfEligible(_ coordinate: CLLocationCoordinate2D) {
        guard !originOverridden, !hasSetOriginFromLocationFix,
              corridorLatRange.contains(coordinate.latitude),
              corridorLonRange.contains(coordinate.longitude)
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
    /// cancels the previous in-flight plan and lands only if it's still the latest.
    ///
    /// IMPORTANT: `client.cancel()` is a latency optimisation only, not a correctness
    /// mechanism. `Planner::plan` resets the cancel flag at entry (core/ffi/src/planner.rs),
    /// so a superseded call can still COMPLETE normally if it never observes the flag before
    /// the next call resets it -- including racing back in as a stale success after a newer
    /// call has already landed. Correctness rests entirely on the generation guard below: any
    /// result or error arriving with `gen != generation` is dropped, including a superseded
    /// call's own `Cancelled` error.
    private func replan() {
        guard let destination, plannerStatus == .ready, let client else { return }

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
            departSoc: departSoc,
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

        Task {
            do {
                let result = try await client.plan(request)
                guard gen == self.generation else { return }
                self.plan = result
                if let style = mapView.style {
                    RouteLayer.addLayers(to: style, plan: result)
                }
                planVersion += 1
                isPlanning = false
            } catch {
                guard gen == self.generation else { return }
                // Leave the previous plan and its layers on screen.
                planErrorMessage = Self.userMessage(for: error)
                planErrorVersion += 1
                isPlanning = false
            }
        }
    }

    private static func userMessage(for error: Error) -> String {
        guard let plannerError = error as? PlannerError else { return "Planning failed" }
        switch plannerError {
        case .NoRouteFound: return "No route — outside pack region?"
        case .PackMissing: return "Planner not ready"
        case .InvalidRequest: return "Invalid route request"
        case .Cancelled: return "Cancelled"
        case .Unimplemented: return "Not available yet"
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
