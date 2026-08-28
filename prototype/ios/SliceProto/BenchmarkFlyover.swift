// Restored flyover benchmark (originally in the wayfinder #15 vertical-slice benchmark app,
// removed once the app became interactive). Recovered from prototype/vertical-slice's
// SliceProtoApp.swift, the version at commit 38b4440^ (see `git show 38b4440^:...`), and
// adapted to PlanStore owning the map instead of a dedicated Engine. Re-run behind
// -benchmark-flyover to re-measure fps/memory against the fixed basemap style.

import CoreLocation
import Darwin
import MapLibre
import QuartzCore

private func physFootprintMB() -> Double {
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

private func fmt(_ v: Double) -> String { String(format: "%.2f", v) }

/// Initial bearing in degrees from `a` to `b`.
private func bearingDegrees(from a: CLLocationCoordinate2D, to b: CLLocationCoordinate2D) -> Double {
    let lat1 = a.latitude * .pi / 180
    let lat2 = b.latitude * .pi / 180
    let dLon = (b.longitude - a.longitude) * .pi / 180
    let y = sin(dLon) * cos(lat2)
    let x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dLon)
    let brng = atan2(y, x) * 180 / .pi
    return (brng + 360).truncatingRemainder(dividingBy: 360)
}

/// CADisplayLink fps/memory probe + ~45s camera flyover along a route (10 waypoints, 4.5s fly
/// segments each), reused near-verbatim from the original benchmark. Started once, after the
/// initial plan's route is on the map; stops itself (invalidates the display link) once the
/// flyover finishes, printing the same PROTO per-second lines and PROTO SUMMARY line as before.
@MainActor
final class BenchmarkFlyover {
    private let mapView: MLNMapView
    private let routeCoordinates: [CLLocationCoordinate2D]

    private var displayLink: CADisplayLink?
    private var frameCountThisSecond = 0
    private var secondAnchor: CFTimeInterval = 0
    private var memPeakMB: Double = 0

    private var secondIndex = 0
    private var fpsSamples: [Double] = []

    init(mapView: MLNMapView, routeCoordinates: [CLLocationCoordinate2D]) {
        self.mapView = mapView
        self.routeCoordinates = routeCoordinates
    }

    func start() {
        guard routeCoordinates.count >= 2 else { return }

        let link = CADisplayLink(target: self, selector: #selector(tick(_:)))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 80, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        displayLink = link

        let n = 10
        var waypoints: [CLLocationCoordinate2D] = []
        for i in 0..<n {
            let idx = Int(Double(i) / Double(n - 1) * Double(routeCoordinates.count - 1))
            waypoints.append(routeCoordinates[idx])
        }
        flySegment(waypoints: waypoints, index: 0)
    }

    @objc private func tick(_ link: CADisplayLink) {
        frameCountThisSecond += 1
        if secondAnchor == 0 { secondAnchor = link.timestamp }
        let elapsed = link.timestamp - secondAnchor
        guard elapsed >= 1.0 else { return }

        let fps = Double(frameCountThisSecond) / elapsed
        frameCountThisSecond = 0
        secondAnchor = link.timestamp

        let mem = physFootprintMB()
        memPeakMB = max(memPeakMB, mem)

        secondIndex += 1
        fpsSamples.append(fps)
        print("PROTO t=\(secondIndex) fps=\(fmt(fps)) mem_mb=\(fmt(mem))")
    }

    private func flySegment(waypoints: [CLLocationCoordinate2D], index: Int) {
        guard index < waypoints.count else {
            finish()
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

    private func finish() {
        displayLink?.invalidate()
        displayLink = nil
        let samples = fpsSamples.count > 2 ? Array(fpsSamples.dropFirst(2)) : fpsSamples
        let avg = samples.isEmpty ? 0 : samples.reduce(0, +) / Double(samples.count)
        let minFps = samples.min() ?? 0
        print("PROTO SUMMARY fps_avg=\(fmt(avg)) fps_min=\(fmt(minFps)) mem_peak_mb=\(fmt(memPeakMB))")
    }
}
