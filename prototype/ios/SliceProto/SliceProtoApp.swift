// Throwaway performance-prototype app. One file on purpose (see prototype/slice/README.md
// for the Rust side this drives). No tests, no polish beyond the metrics overlay.

import SwiftUI
import MapLibre
import Planner
import CoreLocation
import QuartzCore
import Darwin

// MARK: - Memory footprint (task_vm_info phys_footprint, sampled 1 Hz)

func physFootprintMB() -> Double {
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

// MARK: - Documents-directory runtime data

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
    text = text.replacingOccurrences(of: "pmtiles://PMTILES_URL_PLACEHOLDER", with: "pmtiles://" + pmtilesPath)
    let dst = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("slice-proto-style.json")
    guard (try? text.write(to: dst, atomically: true, encoding: .utf8)) != nil else { return nil }
    return dst
}

func fmt(_ v: Double) -> String { String(format: "%.2f", v) }

/// Initial bearing in degrees from `a` to `b`.
func bearingDegrees(from a: CLLocationCoordinate2D, to b: CLLocationCoordinate2D) -> Double {
    let lat1 = a.latitude * .pi / 180
    let lat2 = b.latitude * .pi / 180
    let dLon = (b.longitude - a.longitude) * .pi / 180
    let y = sin(dLon) * cos(lat2)
    let x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dLon)
    let brng = atan2(y, x) * 180 / .pi
    return (brng + 360).truncatingRemainder(dividingBy: 360)
}

// MARK: - Engine: owns the map view, the planner, and all metrics

@MainActor
final class Engine: NSObject, ObservableObject, MLNMapViewDelegate {
    let mapView: MLNMapView

    @Published var missingPaths: [String] = []
    @Published var fpsCurrent: Double = 0
    @Published var fpsAvgOverall: Double = 0
    @Published var fpsMinOverall: Double = .infinity
    @Published var memCurrentMB: Double = 0
    @Published var memPeakMB: Double = 0
    @Published var plannerInitMs: Double = 0
    @Published var routeMs: Double = 0
    @Published var optimiserMs: Double = 0
    @Published var totalMs: Double = 0
    @Published var wallMs: Double = 0
    @Published var stopsCount: Int = 0
    @Published var totalTimeS: Double = 0
    @Published var distKm: Double = 0

    private var displayLink: CADisplayLink?
    private var frameCountThisSecond = 0
    private var secondAnchor: CFTimeInterval = 0
    private var overallSecondsElapsed = 0
    private var overallFpsSum: Double = 0

    private var flyoverActive = false
    private var flyoverSecondIndex = 0
    private var flyoverFpsSamples: [Double] = []

    private var cachedPlanner: Planner?
    private var routeCoordinates: [CLLocationCoordinate2D] = []

    override init() {
        mapView = MLNMapView(frame: .zero)
        super.init()
        mapView.delegate = self

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

        startDisplayLink()
    }

    private func startDisplayLink() {
        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 80, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        displayLink = link
    }

    @objc private func tick(_ link: CADisplayLink) {
        frameCountThisSecond += 1
        if secondAnchor == 0 { secondAnchor = link.timestamp }
        let elapsed = link.timestamp - secondAnchor
        guard elapsed >= 1.0 else { return }

        let fps = Double(frameCountThisSecond) / elapsed
        frameCountThisSecond = 0
        secondAnchor = link.timestamp

        fpsCurrent = fps
        overallSecondsElapsed += 1
        overallFpsSum += fps
        fpsAvgOverall = overallFpsSum / Double(overallSecondsElapsed)
        fpsMinOverall = min(fpsMinOverall, fps)

        let mem = physFootprintMB()
        memCurrentMB = mem
        memPeakMB = max(memPeakMB, mem)

        if flyoverActive {
            flyoverSecondIndex += 1
            flyoverFpsSamples.append(fps)
            print("PROTO t=\(flyoverSecondIndex) fps=\(fmt(fps)) mem_mb=\(fmt(mem))")
        }
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

        let clusterCircles = MLNCircleStyleLayer(identifier: "chargers-clusters", source: source)
        clusterCircles.predicate = NSPredicate(format: "cluster == YES")
        clusterCircles.circleRadius = NSExpression(forConstantValue: 16.0)
        clusterCircles.circleColor = NSExpression(forConstantValue: UIColor.systemOrange)
        clusterCircles.circleOpacity = NSExpression(forConstantValue: 0.85)
        style.addLayer(clusterCircles)

        let clusterCount = MLNSymbolStyleLayer(identifier: "chargers-cluster-count", source: source)
        clusterCount.predicate = NSPredicate(format: "cluster == YES")
        clusterCount.text = NSExpression(format: "CAST(point_count, 'NSString')")
        clusterCount.textColor = NSExpression(forConstantValue: UIColor.white)
        clusterCount.textFontSize = NSExpression(forConstantValue: 11.0)
        style.addLayer(clusterCount)

        let radiusStops: NSDictionary = [0: 4.0, 50: 5.0, 150: 6.5, 350: 8.0]
        let unclustered = MLNCircleStyleLayer(identifier: "chargers-points", source: source)
        unclustered.predicate = NSPredicate(format: "cluster != YES")
        unclustered.circleRadius = NSExpression(
            format: "mgl_interpolate:withCurveType:parameters:stops:(power_kw, 'linear', nil, %@)",
            radiusStops)
        unclustered.circleColor = NSExpression(forConstantValue: UIColor.systemGreen)
        unclustered.circleStrokeWidth = NSExpression(forConstantValue: 1.0)
        unclustered.circleStrokeColor = NSExpression(forConstantValue: UIColor.white)
        style.addLayer(unclustered)
    }

    // MARK: Planning

    func runPlan() {
        guard missingPaths.isEmpty else { return }
        Task.detached { [weak self] in
            guard let self else { return }
            let packDir = documentsURL().appendingPathComponent("pack").path

            let plannerInstance: Planner
            var initMs: Double = 0
            if let existing = await self.cachedPlanner {
                plannerInstance = existing
            } else {
                let t0 = Date()
                plannerInstance = Planner(packDir: packDir)
                initMs = Date().timeIntervalSince(t0) * 1000
                await self.setCachedPlanner(plannerInstance)
                print("PROTO planner_init_ms=\(fmt(initMs))")
            }

            let request: [String: Any] = [
                "origin": [49.6116, 6.1319],
                "dest": [52.3676, 4.9041],
                "depart_soc": 0.9,
            ]
            let requestData = try? JSONSerialization.data(withJSONObject: request)
            let requestJson = requestData.flatMap { String(data: $0, encoding: .utf8) } ?? "{}"

            let wallStart = Date()
            let responseJson = plannerInstance.planJson(requestJson: requestJson)
            let wallMs = Date().timeIntervalSince(wallStart) * 1000

            await self.handlePlanResponse(responseJson, wallMs: wallMs, initMs: initMs)
        }
    }

    private func setCachedPlanner(_ p: Planner) {
        cachedPlanner = p
    }

    private func handlePlanResponse(_ json: String, wallMs: Double, initMs: Double) {
        guard let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else {
            print("PROTO ERROR bad plan response")
            return
        }

        if initMs > 0 { plannerInitMs = initMs }
        self.wallMs = wallMs

        let timings = obj["timings"] as? [String: Any] ?? [:]
        let routeMsV = (timings["route_ms"] as? Double) ?? 0
        let optimiserMsV = (timings["optimiser_ms"] as? Double) ?? 0
        let totalMsV = (timings["total_ms"] as? Double) ?? 0
        let stops = obj["stops"] as? [[String: Any]] ?? []
        let totalTimeSV = (obj["total_time_s"] as? Double) ?? 0
        let totalDistM = (obj["total_dist_m"] as? Double) ?? 0
        let socCurve = obj["soc_curve"] as? [[Double]] ?? []

        routeMs = routeMsV
        optimiserMs = optimiserMsV
        totalMs = totalMsV
        stopsCount = stops.count
        totalTimeS = totalTimeSV
        distKm = totalDistM / 1000

        print("PROTO plan route_ms=\(fmt(routeMsV)) optimiser_ms=\(fmt(optimiserMsV)) total_ms=\(fmt(totalMsV)) " +
              "wall_ms=\(fmt(wallMs)) stops=\(stops.count) total_time_s=\(fmt(totalTimeSV)) dist_km=\(fmt(totalDistM / 1000))")

        if !socCurve.isEmpty {
            let first = socCurve.prefix(3).map { "[\(fmt($0[0])),\(fmt($0[1]))]" }.joined(separator: " ")
            let last = socCurve.suffix(3).map { "[\(fmt($0[0])),\(fmt($0[1]))]" }.joined(separator: " ")
            print("PROTO soc_curve first=\(first) last=\(last)")
        }

        if let routeGeojson = obj["route_geojson"] as? [String: Any] {
            addRouteAndStops(routeGeojson: routeGeojson, stops: stops)
        }
    }

    private func addRouteAndStops(routeGeojson: [String: Any], stops: [[String: Any]]) {
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

        if let coords = routeGeojson["coordinates"] as? [[Double]] {
            routeCoordinates = coords.compactMap {
                guard $0.count >= 2 else { return nil }
                return CLLocationCoordinate2D(latitude: $0[1], longitude: $0[0])
            }
        }

        let stopFeatures: [[String: Any]] = stops.map { s in
            [
                "type": "Feature",
                "geometry": ["type": "Point", "coordinates": [s["lon"] ?? 0, s["lat"] ?? 0]],
                "properties": [
                    "name": s["name"] ?? "", "power_kw": s["power_kw"] ?? 0,
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

        startFlyover()
    }

    // MARK: Flyover (45s: ~10 waypoints x 4.5s fly segments)

    private func startFlyover() {
        guard routeCoordinates.count >= 2 else { return }
        flyoverActive = true
        flyoverSecondIndex = 0
        flyoverFpsSamples = []

        let n = 10
        var waypoints: [CLLocationCoordinate2D] = []
        for i in 0..<n {
            let idx = Int(Double(i) / Double(n - 1) * Double(routeCoordinates.count - 1))
            waypoints.append(routeCoordinates[idx])
        }
        flySegment(waypoints: waypoints, index: 0)
    }

    private func flySegment(waypoints: [CLLocationCoordinate2D], index: Int) {
        guard index < waypoints.count else {
            finishFlyover()
            return
        }
        let center = waypoints[index]
        let heading: Double = index + 1 < waypoints.count
            ? bearingDegrees(from: center, to: waypoints[index + 1])
            : bearingDegrees(from: waypoints[max(0, index - 1)], to: center)

        let camera = MLNMapCamera(lookingAtCenter: center, altitude: 30_000, pitch: 55, heading: heading)
        mapView.fly(to: camera, withDuration: 4.5) { [weak self] in
            self?.flySegment(waypoints: waypoints, index: index + 1)
        }
    }

    private func finishFlyover() {
        flyoverActive = false
        let samples = flyoverFpsSamples.count > 2 ? Array(flyoverFpsSamples.dropFirst(2)) : flyoverFpsSamples
        let avg = samples.isEmpty ? 0 : samples.reduce(0, +) / Double(samples.count)
        let minFps = samples.min() ?? 0
        print("PROTO SUMMARY fps_avg=\(fmt(avg)) fps_min=\(fmt(minFps)) mem_peak_mb=\(fmt(memPeakMB)) plan_total_ms=\(fmt(totalMs))")
    }

    func rerun() {
        runPlan()
    }
}

// MARK: - SwiftUI shell

struct MapViewRepresentable: UIViewRepresentable {
    let mapView: MLNMapView
    func makeUIView(context: Context) -> MLNMapView { mapView }
    func updateUIView(_ uiView: MLNMapView, context: Context) {}
}

struct ContentView: View {
    @StateObject private var engine = Engine()

    var body: some View {
        ZStack(alignment: .topLeading) {
            MapViewRepresentable(mapView: engine.mapView)
                .ignoresSafeArea()

            if !engine.missingPaths.isEmpty {
                missingView
            } else {
                overlay
            }
        }
    }

    private var missingView: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("PROTO ERROR: missing files")
                .font(.headline)
                .foregroundColor(.white)
            ForEach(engine.missingPaths, id: \.self) { p in
                Text(p)
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(.white)
            }
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.black)
    }

    private var overlay: some View {
        VStack(alignment: .leading, spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(String(format: "fps  cur %.0f  avg %.0f  min %.0f",
                            engine.fpsCurrent, engine.fpsAvgOverall,
                            engine.fpsMinOverall.isFinite ? engine.fpsMinOverall : 0))
                Text(String(format: "mem  %.0f MB  peak %.0f MB", engine.memCurrentMB, engine.memPeakMB))
                Text(String(format: "init %.0f ms", engine.plannerInitMs))
                Text(String(format: "route %.0f  opt %.0f  total %.0f  wall %.0f ms",
                            engine.routeMs, engine.optimiserMs, engine.totalMs, engine.wallMs))
                Text(String(format: "stops %d  time %.0f min  dist %.1f km",
                            engine.stopsCount, engine.totalTimeS / 60, engine.distKm))
            }
            .font(.system(.footnote, design: .monospaced))
            .foregroundColor(.white)
            .padding(8)
            .background(Color.black.opacity(0.6))
            .cornerRadius(8)

            Button("Re-run") { engine.rerun() }
                .font(.system(.footnote, design: .monospaced))
                .padding(8)
                .background(Color.black.opacity(0.6))
                .foregroundColor(.white)
                .cornerRadius(8)
        }
        .padding(.top, 50)
        .padding(.leading, 12)
    }
}

@main
struct SliceProtoApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
    }
}
