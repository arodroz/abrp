// Shared planner/map plumbing: owns the single MLNMapView instance, runs Planner in the
// background, parses plan_json responses into Plan, and drives route/stop map layers.
// Kept from the original benchmark app; the CADisplayLink/FPS/flyover instrumentation was
// removed since this is now an interactive prototype, not a perf benchmark.

import CoreLocation
import Foundation
import MapLibre
import Planner
import SwiftUI

// MARK: - Documents-directory runtime data (unchanged from the benchmark app)

func documentsURL() -> URL {
    FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
}

let requiredRelativePaths = [
    "pack/graph.bin",
    "pack/nodes.bin",
    "pack/edges_meta.bin",
    "pack/geometry.bin",
    "pack/chargers.bin",
    "pack/chargers.geojson",
    "corridor-z14.pmtiles",
    "style.json",
]

func findMissingPaths() -> [String] {
    let docs = documentsURL()
    return requiredRelativePaths.filter {
        !FileManager.default.fileExists(atPath: docs.appendingPathComponent($0).path)
    }
}

/// Reads Documents/style.json, replaces the pmtiles placeholder with a file:// path to
/// corridor-z14.pmtiles, writes the patched copy to tmp, returns its URL.
func patchedStyleURL() -> URL? {
    let docs = documentsURL()
    guard var text = try? String(contentsOf: docs.appendingPathComponent("style.json"), encoding: .utf8) else {
        return nil
    }
    let pmtilesPath = docs.appendingPathComponent("corridor-z14.pmtiles").path
    // pmtiles:// wraps a full URL, not a bare path — a path here dies in CFNetwork as "unsupported URL".
    text = text.replacingOccurrences(of: "pmtiles://PMTILES_URL_PLACEHOLDER", with: "pmtiles://file://" + pmtilesPath)
    let dst = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("slice-proto-style.json")
    guard (try? text.write(to: dst, atomically: true, encoding: .utf8)) != nil else { return nil }
    return dst
}

// MARK: - PlanStore: owns the map view, the planner, the Plan, and all request inputs.

/// A failed/snapping-failed plan response (see plan_json's "error" field in lib.rs). Carries
/// a fresh id per occurrence so SwiftUI's onChange fires even for repeated identical messages.
struct PlanErrorEvent: Identifiable, Equatable {
    let id = UUID()
    let message: String
}

/// Roughly lat 49.4-53.6, lon 2.5-7.3 (LU+BE+NL pack corridor). A location fix outside this
/// box (device far from the corridor) is ignored for origin purposes -- the LU fallback stays.
private let corridorLatRange = 49.4...53.6
private let corridorLonRange = 2.5...7.3

@MainActor
final class PlanStore: NSObject, ObservableObject, MLNMapViewDelegate, CLLocationManagerDelegate {
    let mapView: MLNMapView
    private let locationManager = CLLocationManager()

    @Published var missingPaths: [String] = []
    @Published var isPlanning = false
    @Published var plan: Plan?
    /// Incremented on every successful plan; Variant D observes this to fit the camera to
    /// the new route (see CONTEXT.md "Plan").
    @Published var planVersion = 0
    /// Set when plan_json returns an "error" (e.g. destination outside the pack). `plan` and
    /// the map layers are left untouched so the previous state stays on screen.
    @Published var planError: PlanErrorEvent?
    /// Latest CoreLocation fix, for the "locate me" button. Independent of `originCoordinate`.
    @Published var userLocation: CLLocationCoordinate2D?

    // Origin/destination for the plan request. Default to LU -> Amsterdam (same as the
    // original benchmark); Variant D's search and long-press-to-set-origin override these.
    // If a location fix lands inside the pack corridor before any long-press override, it
    // replaces this default once (see locationManager(_:didUpdateLocations:)).
    @Published var originCoordinate = CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319)
    @Published var destinationCoordinate = CLLocationCoordinate2D(latitude: 52.3676, longitude: 4.9041)
    private var originOverridden = false
    private var hasSetOriginFromLocation = false

    // Plan request inputs. Only departSoc re-runs planJson (debounced); the rest are
    // display-only stubs that take effect the next time departSoc changes.
    @Published var departSoc: Double = 0.9 {
        didSet { scheduleReplan() }
    }
    @Published var destinationArrivalSoc: Double = 0.1
    @Published var chargerArrivalSoc: Double = 0.1
    @Published var chargerMaxSoc: Double = 0.8
    @Published var stopsBias: StopsBias = .quickest
    @Published var maxSpeedKmh: Double = 130
    @Published var temperatureC: Double = 20
    @Published var extraWeightKg: Double = 0
    @Published var referenceConsumptionWhKm: Double = 180

    /// Per-stop "charge to %" override, keyed by ChargingStopVM.id. Display-only: naively
    /// rescales the shown charge duration linearly by delta-SoC ratio, no re-plan.
    @Published var stopOverrides: [Int: Double] = [:]

    /// Distance (m) selected by dragging on the SoC chart. Moves a marker on the map.
    @Published var selectedDistanceM: Double? {
        didSet { updateScrubMarker() }
    }

    private var cachedPlanner: Planner?
    private var replanWorkItem: DispatchWorkItem?
    private var scrubAnnotation: MLNPointAnnotation?
    private var originAnnotation: MLNPointAnnotation?

    override init() {
        mapView = MLNMapView(frame: .zero)
        super.init()
        mapView.delegate = self
        mapView.showsUserLocation = true
        locationManager.delegate = self
        locationManager.desiredAccuracy = kCLLocationAccuracyHundredMeters

        let missing = findMissingPaths()
        missingPaths = missing
        for p in missing {
            print("PROTO ERROR missing \(p)")
        }
        if missing.isEmpty {
            if let url = patchedStyleURL() {
                mapView.styleURL = url
            } else {
                missingPaths = ["style.json"]
                print("PROTO ERROR missing style.json")
            }
        }
    }

    // MARK: Current location (Variant D: blue dot + default origin + locate-me button)

    /// Called from Variant D's `onAppear`. Requesting again once already authorized/denied
    /// is a no-op.
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
        userLocation = coordinate

        guard !originOverridden, !hasSetOriginFromLocation,
              corridorLatRange.contains(coordinate.latitude),
              corridorLonRange.contains(coordinate.longitude)
        else { return }
        hasSetOriginFromLocation = true
        originCoordinate = coordinate
        runPlan()
    }

    /// Google-style "locate me" button: just centers the camera, doesn't touch the origin.
    func centerOnUser() {
        guard let userLocation else { return }
        panMap(to: userLocation)
    }

    // MARK: MLNMapViewDelegate

    func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
        addChargersLayers(to: style)
        runPlan()
    }

    private func addChargersLayers(to style: MLNStyle) {
        let geojsonURL = documentsURL().appendingPathComponent("pack/chargers.geojson")
        guard FileManager.default.fileExists(atPath: geojsonURL.path) else { return }

        let options: [MLNShapeSourceOption: Any] = [.clustered: true, .clusterRadius: 50]
        let source = MLNShapeSource(identifier: "chargers", url: geojsonURL, options: options)
        style.addSource(source)

        // Subdued gray-green, only from zoom ~8, so the Plan's own Charging Stops layer
        // (bright red, added in addRouteAndStopsLayers) stays visually dominant.
        let mutedColor = UIColor(red: 0.56, green: 0.64, blue: 0.56, alpha: 1.0)

        let clusterCircles = MLNCircleStyleLayer(identifier: "chargers-clusters", source: source)
        clusterCircles.predicate = NSPredicate(format: "cluster == YES")
        clusterCircles.circleRadius = NSExpression(forConstantValue: 11.0)
        clusterCircles.circleColor = NSExpression(forConstantValue: mutedColor)
        clusterCircles.circleOpacity = NSExpression(forConstantValue: 0.55)
        clusterCircles.minimumZoomLevel = 8
        style.addLayer(clusterCircles)

        let clusterCount = MLNSymbolStyleLayer(identifier: "chargers-cluster-count", source: source)
        clusterCount.predicate = NSPredicate(format: "cluster == YES")
        clusterCount.text = NSExpression(format: "CAST(point_count, 'NSString')")
        clusterCount.textColor = NSExpression(forConstantValue: UIColor.white)
        clusterCount.textFontSize = NSExpression(forConstantValue: 10.0)
        clusterCount.minimumZoomLevel = 8
        style.addLayer(clusterCount)

        let radiusStops: NSDictionary = [0: 2.5, 50: 3.0, 150: 3.75, 350: 4.5]
        let unclustered = MLNCircleStyleLayer(identifier: "chargers-points", source: source)
        unclustered.predicate = NSPredicate(format: "cluster != YES")
        unclustered.circleRadius = NSExpression(
            format: "mgl_interpolate:withCurveType:parameters:stops:(power_kw, 'linear', nil, %@)",
            radiusStops)
        unclustered.circleColor = NSExpression(forConstantValue: mutedColor)
        unclustered.circleOpacity = NSExpression(forConstantValue: 0.6)
        unclustered.circleStrokeWidth = NSExpression(forConstantValue: 0.5)
        unclustered.circleStrokeColor = NSExpression(forConstantValue: UIColor.white.withAlphaComponent(0.6))
        unclustered.minimumZoomLevel = 8
        style.addLayer(unclustered)
    }

    // MARK: Debounced replan (Departure SoC only)

    private func scheduleReplan() {
        replanWorkItem?.cancel()
        let item = DispatchWorkItem { [weak self] in self?.runPlan() }
        replanWorkItem = item
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5, execute: item)
    }

    // MARK: Planning

    /// Runs plan_json with a snapshot of the current request inputs. Called on launch,
    /// after the debounced Departure SoC change, and directly by the "Plan" button in
    /// VariantB (which needs an immediate, non-debounced run).
    func runPlan() {
        guard missingPaths.isEmpty else { return }
        isPlanning = true

        let request: [String: Any] = [
            "origin": [originCoordinate.latitude, originCoordinate.longitude],
            "dest": [destinationCoordinate.latitude, destinationCoordinate.longitude],
            "depart_soc": departSoc,
            "arrival_min_soc": destinationArrivalSoc,
            "charger_arrival_min_soc": chargerArrivalSoc,
            "charger_max_soc": chargerMaxSoc,
            "stops_bias": stopsBias.requestValue,
        ]

        Task.detached { [weak self] in
            guard let self else { return }
            let packDir = documentsURL().appendingPathComponent("pack").path

            let plannerInstance: Planner
            if let existing = await self.cachedPlanner {
                plannerInstance = existing
            } else {
                plannerInstance = Planner(packDir: packDir)
                await self.setCachedPlanner(plannerInstance)
            }

            let requestData = try? JSONSerialization.data(withJSONObject: request)
            let requestJson = requestData.flatMap { String(data: $0, encoding: .utf8) } ?? "{}"
            let responseJson = plannerInstance.planJson(requestJson: requestJson)

            await self.handlePlanResponse(responseJson)
        }
    }

    private func setCachedPlanner(_ p: Planner) {
        cachedPlanner = p
    }

    /// Sets a new destination and immediately re-plans (Variant D's search flow).
    func planTo(destination: CLLocationCoordinate2D) {
        destinationCoordinate = destination
        runPlan()
    }

    /// Sets a new origin (Variant D's long-press-on-map) and immediately re-plans, dropping
    /// or moving a pin annotation at the new origin.
    func setOrigin(_ coordinate: CLLocationCoordinate2D) {
        originOverridden = true
        originCoordinate = coordinate
        if let a = originAnnotation {
            a.coordinate = coordinate
        } else {
            let a = MLNPointAnnotation()
            a.coordinate = coordinate
            a.title = "Origin"
            mapView.addAnnotation(a)
            originAnnotation = a
        }
        runPlan()
    }

    private func handlePlanResponse(_ json: String) {
        isPlanning = false
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            print("PROTO ERROR bad plan response")
            return
        }

        // plan_json returns a 200-shaped-but-empty response with an "error" field on failure
        // (e.g. NO_ROUTE, destination outside the pack). Leave `plan` and the map layers
        // untouched so the previous state stays on screen; PlanErrorEvent lets a variant
        // (Variant D) surface a toast.
        if let errorMsg = obj["error"] as? String {
            print("PROTO plan error: \(errorMsg)")
            planError = PlanErrorEvent(message: errorMsg)
            return
        }

        let stopsRaw = obj["stops"] as? [[String: Any]] ?? []
        let stops: [ChargingStopVM] = stopsRaw.enumerated().map { idx, s in
            ChargingStopVM(
                id: idx,
                name: s["name"] as? String ?? "Charger",
                lat: s["lat"] as? Double ?? 0,
                lon: s["lon"] as? Double ?? 0,
                powerKw: s["power_kw"] as? Double ?? 0,
                arrivalSoc: s["arrival_soc"] as? Double ?? 0,
                departSoc: s["depart_soc"] as? Double ?? 0,
                chargeS: s["charge_s"] as? Double ?? 0,
                distFromStartM: s["dist_from_start_m"] as? Double ?? 0
            )
        }

        let socCurveRaw = obj["soc_curve"] as? [[Double]] ?? []
        let socCurve: [(distM: Double, soc: Double)] = socCurveRaw.compactMap {
            $0.count >= 2 ? ($0[0], $0[1]) : nil
        }

        var routeCoords: [CLLocationCoordinate2D] = []
        var routeGeojson: [String: Any]?
        if let rg = obj["route_geojson"] as? [String: Any] {
            routeGeojson = rg
            if let coords = rg["coordinates"] as? [[Double]] {
                routeCoords = coords.compactMap {
                    guard $0.count >= 2 else { return nil }
                    return CLLocationCoordinate2D(latitude: $0[1], longitude: $0[0])
                }
            }
        }

        let totalDistM = obj["total_dist_m"] as? Double ?? 0
        let arrivalSoc = socCurve.last?.soc ?? departSoc

        plan = Plan(
            totalTimeS: obj["total_time_s"] as? Double ?? 0,
            driveTimeS: obj["drive_time_s"] as? Double ?? 0,
            chargeTimeS: obj["charge_time_s"] as? Double ?? 0,
            totalDistM: totalDistM,
            stops: stops,
            socCurve: socCurve,
            routeCoordinates: routeCoords,
            arrivalSoc: arrivalSoc
        )
        planVersion += 1
        stopOverrides = [:]
        selectedDistanceM = nil

        if let routeGeojson {
            addRouteAndStopsLayers(routeGeojson: routeGeojson, stops: stopsRaw)
        }
    }

    private func addRouteAndStopsLayers(routeGeojson: [String: Any], stops: [[String: Any]]) {
        guard let style = mapView.style else { return }

        for id in ["route-line", "route-line-top", "stops-circles", "stops-labels"] {
            if let layer = style.layer(withIdentifier: id) { style.removeLayer(layer) }
        }
        for id in ["route", "stops"] {
            if let source = style.source(withIdentifier: id) { style.removeSource(source) }
        }

        guard let routeData = try? JSONSerialization.data(withJSONObject: routeGeojson),
              let routeShape = try? MLNShape(data: routeData, encoding: String.Encoding.utf8.rawValue)
        else { return }

        let routeSource = MLNShapeSource(identifier: "route", shape: routeShape, options: nil)
        style.addSource(routeSource)

        let routeLine = MLNLineStyleLayer(identifier: "route-line", source: routeSource)
        routeLine.lineWidth = NSExpression(forConstantValue: 6.0)
        routeLine.lineColor = NSExpression(forConstantValue: UIColor.systemBlue)
        routeLine.lineJoin = NSExpression(forConstantValue: "round")
        routeLine.lineCap = NSExpression(forConstantValue: "round")
        style.addLayer(routeLine)

        // Thin brighter line on top, for looks.
        let routeLineTop = MLNLineStyleLayer(identifier: "route-line-top", source: routeSource)
        routeLineTop.lineWidth = NSExpression(forConstantValue: 2.0)
        routeLineTop.lineColor = NSExpression(forConstantValue: UIColor.cyan)
        routeLineTop.lineJoin = NSExpression(forConstantValue: "round")
        routeLineTop.lineCap = NSExpression(forConstantValue: "round")
        style.addLayer(routeLineTop)

        let stopFeatures: [[String: Any]] = stops.map { s in
            [
                "type": "Feature",
                "geometry": ["type": "Point", "coordinates": [s["lon"] ?? 0, s["lat"] ?? 0]],
                "properties": [
                    "name": s["name"] ?? "", "power_kw": s["power_kw"] ?? 0,
                    // Precomputed label -- MapLibre iOS NSExpression does NOT support CONCAT.
                    "label": "\(s["name"] as? String ?? "") \(Int((s["power_kw"] as? Double) ?? 0))kW",
                ],
            ]
        }
        let stopsFC: [String: Any] = ["type": "FeatureCollection", "features": stopFeatures]
        if let stopsData = try? JSONSerialization.data(withJSONObject: stopsFC),
           let stopsShape = try? MLNShape(data: stopsData, encoding: String.Encoding.utf8.rawValue) {
            let stopsSource = MLNShapeSource(identifier: "stops", shape: stopsShape, options: nil)
            style.addSource(stopsSource)

            let stopsCircles = MLNCircleStyleLayer(identifier: "stops-circles", source: stopsSource)
            stopsCircles.circleRadius = NSExpression(forConstantValue: 10.0)
            stopsCircles.circleColor = NSExpression(forConstantValue: UIColor.systemRed)
            stopsCircles.circleStrokeWidth = NSExpression(forConstantValue: 2.0)
            stopsCircles.circleStrokeColor = NSExpression(forConstantValue: UIColor.white)
            style.addLayer(stopsCircles)

            let stopsLabels = MLNSymbolStyleLayer(identifier: "stops-labels", source: stopsSource)
            stopsLabels.text = NSExpression(forKeyPath: "label")
            stopsLabels.textColor = NSExpression(forConstantValue: UIColor.white)
            stopsLabels.textHaloColor = NSExpression(forConstantValue: UIColor.black)
            stopsLabels.textHaloWidth = NSExpression(forConstantValue: 1.0)
            stopsLabels.textFontSize = NSExpression(forConstantValue: 11.0)
            stopsLabels.textAnchor = NSExpression(forConstantValue: "top")
            style.addLayer(stopsLabels)
        }
    }

    // MARK: Per-stop override display math (no re-plan; linear rescale of charge duration)

    func displayedChargeS(for stop: ChargingStopVM, index: Int) -> Double {
        guard let overrideSoc = stopOverrides[index] else { return stop.chargeS }
        let originalDelta = stop.departSoc - stop.arrivalSoc
        guard originalDelta > 0.0001 else { return stop.chargeS }
        let newDelta = max(0, overrideSoc - stop.arrivalSoc)
        return stop.chargeS * (newDelta / originalDelta)
    }

    // MARK: Map camera + chart scrub marker

    func panMap(to coordinate: CLLocationCoordinate2D) {
        mapView.setCenter(coordinate, animated: true)
    }

    /// Fits the camera to the current plan's route (Variant D, called after a replan).
    func fitToRoute() {
        guard let plan, !plan.routeCoordinates.isEmpty else { return }
        var minLat = plan.routeCoordinates[0].latitude
        var maxLat = minLat
        var minLon = plan.routeCoordinates[0].longitude
        var maxLon = minLon
        for c in plan.routeCoordinates {
            minLat = min(minLat, c.latitude)
            maxLat = max(maxLat, c.latitude)
            minLon = min(minLon, c.longitude)
            maxLon = max(maxLon, c.longitude)
        }
        let bounds = MLNCoordinateBoundsMake(
            CLLocationCoordinate2D(latitude: minLat, longitude: minLon),
            CLLocationCoordinate2D(latitude: maxLat, longitude: maxLon))
        mapView.setVisibleCoordinateBounds(
            bounds,
            edgePadding: UIEdgeInsets(top: 110, left: 40, bottom: 260, right: 40),
            animated: true,
            completionHandler: nil)
    }

    /// nearest soc_curve sample by distance -> nearest route point by fraction of total
    /// distance. Approximate, matches the SoC curve's own 2km sampling resolution.
    private func updateScrubMarker() {
        guard let plan, let distM = selectedDistanceM,
              !plan.socCurve.isEmpty, !plan.routeCoordinates.isEmpty, plan.totalDistM > 0
        else {
            if let a = scrubAnnotation { mapView.removeAnnotation(a) }
            scrubAnnotation = nil
            return
        }
        let nearest = plan.socCurve.min { abs($0.distM - distM) < abs($1.distM - distM) }!
        let fraction = min(max(nearest.distM / plan.totalDistM, 0), 1)
        let idx = min(plan.routeCoordinates.count - 1, max(0, Int(fraction * Double(plan.routeCoordinates.count - 1))))
        let coord = plan.routeCoordinates[idx]

        if let a = scrubAnnotation {
            a.coordinate = coord
        } else {
            let a = MLNPointAnnotation()
            a.coordinate = coord
            mapView.addAnnotation(a)
            scrubAnnotation = a
        }
        mapView.setCenter(coord, animated: true)
    }
}
