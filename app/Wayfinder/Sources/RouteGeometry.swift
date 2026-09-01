// Pure display-only route-geometry helpers (wayfinder #84): the blue route polyline used to
// start/end at the routing-graph junction node the origin/destination snapped to, some distance
// from the point the rider actually asked for, with that gap never drawn -- Google Maps instead
// trims the line to the road and draws a short dashed stub from the pin to it. This file provides
// that trim + connector math, reusing RouteSnap's local-equirectangular projection primitives
// (`project`/`closestPointOnSegment`/`cumulativeDistances`) rather than duplicating them. Pure
// over coordinate arrays -- no MapLibre/UIKit dependency -- so it's unit-testable directly
// (AutotestRouteGeometry.swift's `route-geometry-smoke`). Display-only: DriveStore/RouteSnap/
// StepTracker keep consuming the raw, untrimmed `plan.polyline` -- only RouteLayer.swift (the map
// rendering) calls into this.
import CoreLocation
import Foundation

enum RouteGeometry {
    /// One projection of a point onto a polyline: the nearest point ON the polyline, its
    /// perpendicular distance from the projected point, and that point's cumulative walked
    /// distance along `polyline` from its start.
    struct Projection {
        let point: CLLocationCoordinate2D
        let perpendicularDistanceM: Double
        let cumulativeDistanceM: Double
    }

    /// Projects `point` onto the nearest point of `polyline`, searching only segments that
    /// START within the polyline's leading `withinFirstMeters` (pass `.infinity` to search the
    /// whole polyline). `nil` if `polyline` has fewer than 2 points.
    static func projectOntoPolyline(
        _ point: CLLocationCoordinate2D, polyline: [CLLocationCoordinate2D], withinFirstMeters: Double
    ) -> Projection? {
        guard polyline.count >= 2 else { return nil }
        let cumulativeM = RouteSnap.cumulativeDistances(polyline)
        let cosLat = cos(point.latitude * .pi / 180)
        let pointProjected = RouteSnap.project(point, cosLat: cosLat)

        var best: (segmentIndex: Int, t: Double, distM: Double)?
        for i in 0..<(polyline.count - 1) {
            guard cumulativeM[i] <= withinFirstMeters else { break }
            let a = RouteSnap.project(polyline[i], cosLat: cosLat)
            let b = RouteSnap.project(polyline[i + 1], cosLat: cosLat)
            let (t, distM) = RouteSnap.closestPointOnSegment(fix: pointProjected, a: a, b: b)
            if best == nil || distM < best!.distM {
                best = (i, t, distM)
            }
        }
        guard let result = best else { return nil }

        let a = polyline[result.segmentIndex]
        let b = polyline[result.segmentIndex + 1]
        let projectedCoordinate = CLLocationCoordinate2D(
            latitude: a.latitude + result.t * (b.latitude - a.latitude),
            longitude: a.longitude + result.t * (b.longitude - a.longitude)
        )
        let segmentLenM = cumulativeM[result.segmentIndex + 1] - cumulativeM[result.segmentIndex]
        let cumulativeDistanceM = cumulativeM[result.segmentIndex] + result.t * segmentLenM

        return Projection(
            point: projectedCoordinate, perpendicularDistanceM: result.distM,
            cumulativeDistanceM: cumulativeDistanceM
        )
    }

    /// Trims `polyline`'s leading vertices up to (and inserting) the point at `target`'s
    /// cumulative distance -- unless that point coincides with an existing vertex, in which case
    /// no duplicate is inserted. Recomputes `polyline`'s own cumulative distances (this runs once
    /// per drawn route, not per frame, so the O(n) recompute is not worth threading through).
    private static func trimLeading(
        _ polyline: [CLLocationCoordinate2D], toCumulativeDistanceM target: Double,
        projectedPoint: CLLocationCoordinate2D
    ) -> [CLLocationCoordinate2D] {
        let cumulativeM = RouteSnap.cumulativeDistances(polyline)
        guard let cutIndex = cumulativeM.firstIndex(where: { $0 >= target - 1e-6 }) else {
            return polyline
        }
        if abs(cumulativeM[cutIndex] - target) < 1e-6 {
            return Array(polyline[cutIndex...])
        }
        return [projectedPoint] + Array(polyline[cutIndex...])
    }

    /// The trimmed display polyline and its pin-to-road dashed connectors. Origin: if it
    /// projects within the polyline's leading 2 km at <= 50 m perpendicular, the display
    /// polyline is trimmed to start at that projection; if the remaining straight-line gap
    /// origin -> trimmed-start is still > 10 m, `originConnector` is the two-point stub to draw.
    /// Symmetric at the destination end, against the trailing 2 km. A polyline of 2 points or
    /// fewer is returned untouched -- there's no meaningful "trim toward the road" on a single
    /// segment that IS the whole route. Never trims past 2 remaining points.
    static func trimmedDisplayPolyline(
        _ polyline: [CLLocationCoordinate2D], origin: CLLocationCoordinate2D,
        destination: CLLocationCoordinate2D
    ) -> (
        display: [CLLocationCoordinate2D], originConnector: [CLLocationCoordinate2D]?,
        destinationConnector: [CLLocationCoordinate2D]?
    ) {
        guard polyline.count > 2 else { return (polyline, nil, nil) }

        let searchWithinM = 2_000.0
        let trimPerpendicularM = 50.0
        let connectorGapM = 10.0

        var display = polyline
        var originConnector: [CLLocationCoordinate2D]?
        var destinationConnector: [CLLocationCoordinate2D]?

        if let projection = projectOntoPolyline(origin, polyline: display, withinFirstMeters: searchWithinM),
           projection.perpendicularDistanceM <= trimPerpendicularM {
            let trimmed = trimLeading(
                display, toCumulativeDistanceM: projection.cumulativeDistanceM, projectedPoint: projection.point
            )
            if trimmed.count >= 2 {
                display = trimmed
                let gapM = CLLocation(latitude: origin.latitude, longitude: origin.longitude)
                    .distance(from: CLLocation(latitude: display[0].latitude, longitude: display[0].longitude))
                if gapM > connectorGapM {
                    originConnector = [origin, display[0]]
                }
            }
        }

        // Destination end: search/trim the reversed polyline so its "leading" edge is the
        // route's actual trailing edge, then reverse the result back.
        let reversed = Array(display.reversed())
        if let projection = projectOntoPolyline(destination, polyline: reversed, withinFirstMeters: searchWithinM),
           projection.perpendicularDistanceM <= trimPerpendicularM {
            let trimmedReversed = trimLeading(
                reversed, toCumulativeDistanceM: projection.cumulativeDistanceM, projectedPoint: projection.point
            )
            if trimmedReversed.count >= 2 {
                display = Array(trimmedReversed.reversed())
                let lastIndex = display.count - 1
                let gapM = CLLocation(latitude: destination.latitude, longitude: destination.longitude)
                    .distance(from: CLLocation(
                        latitude: display[lastIndex].latitude, longitude: display[lastIndex].longitude
                    ))
                if gapM > connectorGapM {
                    destinationConnector = [destination, display[lastIndex]]
                }
            }
        }

        return (display, originConnector, destinationConnector)
    }
}
