// Charging Stop view model + small formatting helpers shared by ResultCard and SoCChartView
// (wayfinder #43). Ported from prototype/planner-ui's PlannerModels.swift, re-typed off the
// FFI records (FfiPlan/FfiStop) instead of the prototype's own Plan/ChargingStopVM shape.
// Skipped: StopsBias (nothing in this ticket consumes it -- it lands with #44), the Plan
// struct (FfiPlan is used directly), and the subscript(safe:) extension (not needed).
import CoreLocation
import Foundation
import PlannerKit

/// A Charging Stop enriched with its distance from the route start -- FfiStop doesn't carry
/// this itself (see `stops(from:)` below). Keyed by `chargerId` (stable), not array index.
struct ChargingStopVM: Identifiable {
    let id: String
    let name: String
    let lat: Double
    let lon: Double
    let powerKw: Double
    let arrivalSoc: Double
    let departSoc: Double
    let chargeS: Double
    let distFromStartM: Double

    var coordinate: CLLocationCoordinate2D { CLLocationCoordinate2D(latitude: lat, longitude: lon) }
}

extension ChargingStopVM {
    /// Derives each stop's distance from the route start by walking `plan.legs` in order and
    /// accumulating `distM`: a charger leg's `toLabel` IS the site name (`endpoint_label`,
    /// core/ffi/src/mapping.rs), so matching it against the next unmatched stop's `name` is
    /// exact -- Waypoint endpoints are labeled "Waypoint N" so they never collide with a
    /// charger's name.
    static func stops(from plan: FfiPlan) -> [ChargingStopVM] {
        var result: [ChargingStopVM] = []
        var accumulatedM = 0.0
        var nextStopIndex = 0
        for leg in plan.legs {
            accumulatedM += leg.distM
            guard nextStopIndex < plan.stops.count, leg.toLabel == plan.stops[nextStopIndex].name else { continue }
            let stop = plan.stops[nextStopIndex]
            result.append(ChargingStopVM(
                id: stop.chargerId, name: stop.name, lat: stop.lat, lon: stop.lon, powerKw: stop.powerKw,
                arrivalSoc: stop.arrivalSoc, departSoc: stop.departSoc, chargeS: stop.chargeS,
                distFromStartM: accumulatedM
            ))
            nextStopIndex += 1
        }
        return result
    }
}

func formatDuration(_ seconds: Double) -> String {
    let mins = Int((seconds / 60).rounded())
    let h = mins / 60
    let m = mins % 60
    return h > 0 ? "\(h)h \(m)m" : "\(m)m"
}

func formatSocPct(_ soc: Double) -> String {
    "\(Int((soc * 100).rounded()))%"
}

func formatKm(_ meters: Double) -> String {
    String(format: "%.0f km", meters / 1000)
}
