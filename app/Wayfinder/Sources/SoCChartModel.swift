// Pure computations for the SoC-over-distance chart overhaul (wayfinder #83): arrival-SoC
// callout positions/labels/colors, the red/amber/green margin rule, curve interpolation +
// splitting, and the actual-SoC trail's thinning decision. Kept out of SoCChartView entirely so
// `soc-chart-smoke` (AutotestSocChart.swift) can assert every one of these without any UI --
// the view (SoCChartView.swift) just renders what these functions hand it. All distances are
// meters, all SoC values PERCENT (0-100) unless a parameter/field name says `Fraction`.
import PlannerKit
import SwiftUI

enum SoCChartModel {
    /// One arrival-SoC callout (design point 1): a Charging Stop's arrival point, or the
    /// destination's final curve value.
    struct Callout: Equatable {
        let distM: Double
        let socPct: Double
        let label: String
        let color: Color
    }

    /// One kept point of the ACTUAL SoC trail (design point 5, the Tesla-style overlay).
    struct SoCTrailPoint: Equatable {
        let distM: Double
        let socPct: Double
    }

    /// Margin coloring (design point 1): red below `floorPct` (the charger arrival-SoC floor --
    /// `PlanStore.chargerArrivalMinSoc * 100` when reachable, else this 10 default, matching the
    /// same 0.10 the planner request itself defaults to), amber up to `floorPct + amberSpanPct`,
    /// green above. Boundaries are inclusive at their lower edge per the ticket's own decision:
    /// exactly `floorPct` is amber, not red; exactly `floorPct + amberSpanPct` is amber, not green.
    static func socMarginColor(_ pct: Double, floorPct: Double = 10, amberSpanPct: Double = 10) -> Color {
        if pct < floorPct { return .red }
        if pct <= floorPct + amberSpanPct { return .orange }
        return .green
    }

    static func roundedPctLabel(_ pct: Double) -> String {
        "\(Int(pct.rounded()))%"
    }

    /// Charge-duration label for a stop glyph (design point 2), e.g. "22m".
    static func chargeMinutesLabel(_ chargeS: Double) -> String {
        "\(Int((chargeS / 60).rounded()))m"
    }

    /// Arrival-SoC callouts for every Charging Stop plus the destination (design point 1).
    /// Takes the minimal per-stop/destination inputs, not a whole `FfiPlan`, so a synthetic
    /// scenario needs no `FfiPlan` construction at all -- see soc-chart-smoke.
    static func arrivalCallouts(
        stops: [ChargingStopVM], destinationDistM: Double, destinationSocFraction: Double,
        floorPct: Double = 10, amberSpanPct: Double = 10
    ) -> [Callout] {
        var callouts = stops.map { stop -> Callout in
            let pct = stop.arrivalSoc * 100
            return Callout(
                distM: stop.distFromStartM, socPct: pct, label: roundedPctLabel(pct),
                color: socMarginColor(pct, floorPct: floorPct, amberSpanPct: amberSpanPct)
            )
        }
        let destPct = destinationSocFraction * 100
        callouts.append(Callout(
            distM: destinationDistM, socPct: destPct, label: roundedPctLabel(destPct),
            color: socMarginColor(destPct, floorPct: floorPct, amberSpanPct: amberSpanPct)
        ))
        return callouts
    }

    /// Linear interpolation of a SoC curve (sorted ascending by `distM`) at `distanceM`, clamped
    /// to the curve's ends. A pure chart-side twin of `DriveStore`'s private `interpolatedSoc` --
    /// not shared, since that one is drive-only bookkeeping bound up with the HUD (see this
    /// ticket's own note); same algorithm. Returns a FRACTION (0-1), matching `FfiSocPoint.soc`.
    static func interpolatedSocFraction(_ curve: [FfiSocPoint], at distanceM: Double) -> Double {
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

    /// Splits a SoC curve at `distanceM` into "behind" (<= distanceM) and "ahead" (>= distanceM)
    /// segments, each carrying an interpolated boundary point at exactly `distanceM` so the two
    /// segments join with no visual gap when drawn as separate LineMarks (design point 4, driven
    /// vs. ahead).
    static func splitCurve(_ curve: [FfiSocPoint], at distanceM: Double) -> (behind: [FfiSocPoint], ahead: [FfiSocPoint]) {
        let boundary = FfiSocPoint(distM: distanceM, soc: interpolatedSocFraction(curve, at: distanceM))
        var behind = curve.filter { $0.distM <= distanceM }
        var ahead = curve.filter { $0.distM >= distanceM }
        if behind.last?.distM != distanceM { behind.append(boundary) }
        if ahead.first?.distM != distanceM { ahead.insert(boundary, at: 0) }
        return (behind, ahead)
    }

    /// Whether a new actual-SoC reading is worth keeping in the trail (design point 5): the
    /// first point always is; after that, only once the route has moved >=500 m past the last
    /// kept point OR the reading has drifted >=0.5 SoC points -- keeps the trail sparse from a
    /// ~1 Hz fix stream without smoothing over a real jump (e.g. a charge event).
    static func shouldAppendTrailPoint(lastKept: SoCTrailPoint?, candidate: SoCTrailPoint) -> Bool {
        guard let lastKept else { return true }
        return candidate.distM - lastKept.distM >= 500 || abs(candidate.socPct - lastKept.socPct) >= 0.5
    }
}
