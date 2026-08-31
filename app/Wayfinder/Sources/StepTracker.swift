// Maneuver banner step tracking (wayfinder #67, ADR 0012 point 3's HUD extended to turn-by-turn
// guidance): pure, no MapLibre/CoreLocation-delegate deps, same RouteSnap idiom -- resolves each
// Plan's `FfiLeg.steps` into a flat, display-ready `[GuidanceStep]` once per snapshot (Plan
// adoption or a mid-drive replan swap), then a cheap binary search finds the upcoming one per
// fix. On a v1 pack every leg's `steps` is empty, so `guidanceSteps` comes back `[]` and
// `upcomingIndex` always `nil` -- the whole banner degrades to "nothing shown" for free.
//
// Anchoring: `distFromLegStartM` is a PACK-metric distance (summed edge lengths), but the app's
// polyline is downsampled and its own cumulative distance reads slightly short over a route --
// comparing the two directly would drift. Instead each step's own (lat, lon) -- a real junction
// node, which lies ON the route -- is projected onto the polyline with the same local-
// equirectangular point-to-segment math RouteSnap.snap uses (its `project`/`closestPointOnSegment`
// helpers are shared, not copy-pasted), searched in a small forward window off the previous
// anchor so anchors stay monotonic and the walk stays cheap on multi-thousand-point polylines.
import CoreLocation
import Foundation
import PlannerKit

enum StepTracker {
    /// One banner-eligible step, resolved to display strings and a polyline-metric anchor at
    /// snapshot time. Everything here is static per Plan.
    struct GuidanceStep: Equatable {
        let distAlongRouteM: Double
        let iconSystemName: String
        let primary: String
        let secondary: String?   // signage line ("toward ..."), nil when no signage
        let then: String?        // next eligible step's primary when its anchor is < 400 m past this one
    }

    static let thenChainThresholdM = 400.0
    static let passedBufferM = 10.0

    /// A `.arrive` at a leg boundary is only a real Charging Stop when a stop's own along-route
    /// distance lands within this of the leg boundary's expected pack distance -- otherwise it's
    /// a pass-through charger boundary and gets skipped.
    private static let stopMatchToleranceM = 150.0
    /// Search-window slack past a step's expected pack distance (see the anchoring comment
    /// above) -- generous enough to absorb the polyline/pack distance drift over one leg-sized
    /// span without scanning the whole remaining route per step.
    private static let anchorSearchSlackM = 500.0

    static func guidanceSteps(
        legs: [FfiLeg], stops: [ChargingStopVM],
        polyline: [CLLocationCoordinate2D], cumulativeM: [Double]
    ) -> [GuidanceStep] {
        guard polyline.count >= 2, let routeLenM = cumulativeM.last else { return [] }

        var result: [GuidanceStep] = []
        var legStartPackM = 0.0
        var prevAnchorSegment = 0
        var prevAnchorM = 0.0
        let finalLegIndex = legs.count - 1

        for (legIndex, leg) in legs.enumerated() {
            let isFinalLeg = legIndex == finalLegIndex
            for step in leg.steps {
                let expectedM = legStartPackM + step.distFromLegStartM

                let arriveLabel: String?
                let eligible: Bool
                switch step.maneuver {
                case .depart:
                    eligible = false
                    arriveLabel = nil
                case .arrive:
                    if isFinalLeg {
                        eligible = true
                        arriveLabel = nil
                    } else if let matchedStop = stops.first(where: { abs($0.distFromStartM - expectedM) <= stopMatchToleranceM }) {
                        eligible = true
                        arriveLabel = matchedStop.name
                    } else {
                        eligible = false
                        arriveLabel = nil
                    }
                default:
                    eligible = true
                    arriveLabel = nil
                }
                guard eligible else { continue }

                let anchorM = anchorDistanceM(
                    stepLat: step.lat, stepLon: step.lon, expectedM: expectedM,
                    polyline: polyline, cumulativeM: cumulativeM, routeLenM: routeLenM,
                    prevAnchorSegment: &prevAnchorSegment, prevAnchorM: &prevAnchorM
                )

                result.append(GuidanceStep(
                    distAlongRouteM: anchorM, iconSystemName: StepFormatter.iconSystemName(step),
                    primary: StepFormatter.primary(step, arriveLabel: arriveLabel),
                    secondary: StepFormatter.secondary(step), then: nil
                ))
            }
            legStartPackM += leg.distM
        }

        for i in 0..<result.count {
            guard i + 1 < result.count, result[i + 1].distAlongRouteM - result[i].distAlongRouteM < thenChainThresholdM else { continue }
            let step = result[i]
            result[i] = GuidanceStep(
                distAlongRouteM: step.distAlongRouteM, iconSystemName: step.iconSystemName,
                primary: step.primary, secondary: step.secondary, then: result[i + 1].primary
            )
        }
        return result
    }

    /// Index of the upcoming step: first with `distAlongRouteM > distanceAlongRouteM +
    /// passedBufferM`. `nil` when none remain or `steps` is empty. Binary search (anchors are
    /// non-decreasing).
    static func upcomingIndex(steps: [GuidanceStep], distanceAlongRouteM: Double) -> Int? {
        guard !steps.isEmpty else { return nil }
        let threshold = distanceAlongRouteM + passedBufferM
        var lo = 0
        var hi = steps.count
        while lo < hi {
            let mid = (lo + hi) / 2
            if steps[mid].distAlongRouteM > threshold {
                hi = mid
            } else {
                lo = mid + 1
            }
        }
        return lo < steps.count ? lo : nil
    }

    /// Projects `(stepLat, stepLon)` onto `polyline` within a forward window starting at
    /// `prevAnchorSegment` through the first segment whose cumulative distance passes
    /// `expectedM + anchorSearchSlackM` (clamped to the polyline's end) -- same point-to-segment
    /// math RouteSnap.snap uses. The result is clamped to `prevAnchorM` so anchors never regress,
    /// and both `inout` cursors advance for the next step's search. An empty/degenerate window
    /// (the previous anchor already past this step's search range) falls back to `expectedM`
    /// clamped to the route length -- junction nodes lie on the route, so this only matters for
    /// pathological inputs.
    private static func anchorDistanceM(
        stepLat: Double, stepLon: Double, expectedM: Double,
        polyline: [CLLocationCoordinate2D], cumulativeM: [Double], routeLenM: Double,
        prevAnchorSegment: inout Int, prevAnchorM: inout Double
    ) -> Double {
        let fallback = max(0, min(expectedM, routeLenM))
        let segmentCount = polyline.count - 1
        guard segmentCount >= 1 else { return fallback }

        let startSegment = min(prevAnchorSegment, segmentCount - 1)
        let windowEndVertex = cumulativeM.firstIndex { $0 > expectedM + anchorSearchSlackM } ?? cumulativeM.count - 1
        let endSegment = min(windowEndVertex, segmentCount - 1)
        guard endSegment >= startSegment else { return fallback }

        let stepCoordinate = CLLocationCoordinate2D(latitude: stepLat, longitude: stepLon)
        let cosLat = cos(stepLat * .pi / 180)
        let stepProjected = RouteSnap.project(stepCoordinate, cosLat: cosLat)

        var best: (segmentIndex: Int, t: Double, distM: Double)?
        for i in startSegment...endSegment {
            let a = RouteSnap.project(polyline[i], cosLat: cosLat)
            let b = RouteSnap.project(polyline[i + 1], cosLat: cosLat)
            let (t, distM) = RouteSnap.closestPointOnSegment(fix: stepProjected, a: a, b: b)
            if best == nil || distM < best!.distM {
                best = (i, t, distM)
            }
        }
        guard let result = best else { return fallback }

        let segLenM = cumulativeM[result.segmentIndex + 1] - cumulativeM[result.segmentIndex]
        let rawAnchorM = cumulativeM[result.segmentIndex] + result.t * segLenM
        let anchorM = max(rawAnchorM, prevAnchorM)
        prevAnchorSegment = result.segmentIndex
        prevAnchorM = anchorM
        return anchorM
    }
}
