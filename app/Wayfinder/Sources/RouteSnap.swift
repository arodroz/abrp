// Pure GPS-to-route snap math for Drive Mode (wayfinder #59, ADR 0012 point 3): projects a
// fix onto the Plan's polyline in a local equirectangular plane (lon scaled by cos(latitude),
// 1 deg lat ~= 111_320 m -- good enough at route scales, matching the codebase's existing
// haversine pragmatism elsewhere, e.g. PlanStore.updateScrubMarker). No MapLibre/CoreLocation
// delegate dependencies here, so this is trivially unit-testable and reusable by DriveStore's
// ingest path and (later) off-route replan detection.
import CoreLocation
import Foundation

enum RouteSnap {
    struct Result {
        let coordinate: CLLocationCoordinate2D
        let distanceFromRouteM: Double
        let distanceAlongRouteM: Double
        let segmentIndex: Int
        let segmentBearingDeg: Double
    }

    private static let metersPerDegLat = 111_320.0

    /// Cumulative walked (haversine) distance along `polyline`, one entry per vertex --
    /// `cumulativeM[0] == 0`. Computed once per Plan (DriveStore.go()), not per fix.
    static func cumulativeDistances(_ polyline: [CLLocationCoordinate2D]) -> [Double] {
        guard !polyline.isEmpty else { return [] }
        var result = [0.0]
        result.reserveCapacity(polyline.count)
        for i in 1..<polyline.count {
            let d = CLLocation(latitude: polyline[i - 1].latitude, longitude: polyline[i - 1].longitude)
                .distance(from: CLLocation(latitude: polyline[i].latitude, longitude: polyline[i].longitude))
            result.append(result[i - 1] + d)
        }
        return result
    }

    /// Projects `fix` onto the nearest point of `polyline`. `hintSegment` narrows the search to
    /// a DISTANCE window around the hint's along-route position (500 m back, 5 km ahead --
    /// segment-count windows broke when wayfinder #84 densified the polyline: 50 segments
    /// shrank from tens of km to a few hundred meters, so a fix past the window could snap to a
    /// wrong-but-nearby parallel pass of the route instead of reaching the fallback) to keep
    /// the per-fix cost O(window) on multi-thousand-point polylines; if the best candidate in
    /// that window is still more than 250 m away (a GPS jump, or a hint from a stale/different
    /// route), falls back to a full scan. `nil` hint always does a full scan. Returns `nil` only
    /// when `polyline` has fewer than 2 points.
    static func snap(
        fix: CLLocationCoordinate2D, polyline: [CLLocationCoordinate2D], cumulativeM: [Double], hintSegment: Int?
    ) -> Result? {
        guard polyline.count >= 2 else { return nil }
        let segmentCount = polyline.count - 1
        let cosLat = cos(fix.latitude * .pi / 180)
        let fixProjected = project(fix, cosLat: cosLat)

        func bestInRange(_ range: Range<Int>) -> (segmentIndex: Int, t: Double, distM: Double)? {
            var best: (segmentIndex: Int, t: Double, distM: Double)?
            for i in range {
                let a = project(polyline[i], cosLat: cosLat)
                let b = project(polyline[i + 1], cosLat: cosLat)
                let (t, distM) = closestPointOnSegment(fix: fixProjected, a: a, b: b)
                if best == nil || distM < best!.distM {
                    best = (i, t, distM)
                }
            }
            return best
        }

        let windowRange: Range<Int>
        if let hintSegment {
            let windowBackM = 500.0
            let windowAheadM = 5_000.0
            let hint = max(0, min(hintSegment, segmentCount - 1))
            let hintDistM = cumulativeM[hint]
            var lo = hint
            while lo > 0, cumulativeM[lo] > hintDistM - windowBackM { lo -= 1 }
            var hi = hint
            while hi < segmentCount - 1, cumulativeM[hi + 1] < hintDistM + windowAheadM { hi += 1 }
            windowRange = lo..<(hi + 1)
        } else {
            windowRange = 0..<segmentCount
        }

        var best = bestInRange(windowRange)
        if hintSegment != nil, let candidate = best, candidate.distM > 250 {
            best = bestInRange(0..<segmentCount)
        }
        guard let result = best else { return nil }

        let a = polyline[result.segmentIndex]
        let b = polyline[result.segmentIndex + 1]
        let snappedCoordinate = CLLocationCoordinate2D(
            latitude: a.latitude + result.t * (b.latitude - a.latitude),
            longitude: a.longitude + result.t * (b.longitude - a.longitude)
        )
        let segmentLenM = cumulativeM[result.segmentIndex + 1] - cumulativeM[result.segmentIndex]
        let distanceAlongRouteM = cumulativeM[result.segmentIndex] + result.t * segmentLenM

        return Result(
            coordinate: snappedCoordinate, distanceFromRouteM: result.distM,
            distanceAlongRouteM: distanceAlongRouteM, segmentIndex: result.segmentIndex,
            segmentBearingDeg: bearingDeg(from: a, to: b)
        )
    }

    /// Internal, not private (wayfinder #67): StepTracker's step-to-polyline anchoring reuses
    /// this same local-equirectangular projection instead of duplicating it.
    static func project(_ coordinate: CLLocationCoordinate2D, cosLat: Double) -> (x: Double, y: Double) {
        (x: coordinate.longitude * metersPerDegLat * cosLat, y: coordinate.latitude * metersPerDegLat)
    }

    /// Standard point-to-segment projection in the local metric plane: `t` in [0, 1] along a->b,
    /// clamped at the endpoints, plus the resulting distance from `fix` to that point. Internal,
    /// not private (wayfinder #67), for the same reason as `project` above.
    static func closestPointOnSegment(
        fix: (x: Double, y: Double), a: (x: Double, y: Double), b: (x: Double, y: Double)
    ) -> (t: Double, distM: Double) {
        let dx = b.x - a.x
        let dy = b.y - a.y
        let lenSq = dx * dx + dy * dy
        let t = lenSq > 0 ? max(0, min(1, ((fix.x - a.x) * dx + (fix.y - a.y) * dy) / lenSq)) : 0
        let px = a.x + t * dx
        let py = a.y + t * dy
        let distM = ((fix.x - px) * (fix.x - px) + (fix.y - py) * (fix.y - py)).squareRoot()
        return (t, distM)
    }

    /// Initial great-circle bearing from `a` to `b`, in degrees clockwise from true north.
    private static func bearingDeg(from a: CLLocationCoordinate2D, to b: CLLocationCoordinate2D) -> Double {
        let lat1 = a.latitude * .pi / 180
        let lat2 = b.latitude * .pi / 180
        let dLon = (b.longitude - a.longitude) * .pi / 180
        let y = sin(dLon) * cos(lat2)
        let x = cos(lat1) * sin(lat2) - sin(lat1) * cos(lat2) * cos(dLon)
        let bearing = atan2(y, x) * 180 / .pi
        return (bearing + 360).truncatingRemainder(dividingBy: 360)
    }
}
