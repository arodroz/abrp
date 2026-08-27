// Data models + small formatting helpers shared by all three planner-UI variants.
// See prototype/slice/src/lib.rs for the plan_json request/response schema this mirrors.

import CoreLocation
import Foundation

/// A Charging Stop as returned by the planner (see CONTEXT.md: "Charging Stop").
struct ChargingStopVM: Identifiable {
    let id: Int // stable index into Plan.stops, not a UUID -- used to key stopOverrides
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

/// A parsed Plan (see CONTEXT.md: "Plan", "SoC Curve").
struct Plan {
    var totalTimeS: Double
    var driveTimeS: Double
    var chargeTimeS: Double
    var totalDistM: Double
    var stops: [ChargingStopVM]
    var socCurve: [(distM: Double, soc: Double)]
    var routeCoordinates: [CLLocationCoordinate2D]
    var arrivalSoc: Double
}

/// Stops Bias (see CONTEXT.md): few-long / quickest / many-short.
enum StopsBias: String, CaseIterable, Identifiable {
    case fewLong = "Few, long"
    case quickest = "Quickest"
    case manyShort = "Many, short"

    var id: String { rawValue }

    /// Maps to the `stops_bias` request field (stop_penalty_s = 300 * stops_bias in lib.rs).
    var requestValue: Double {
        switch self {
        case .fewLong: return 2.0
        case .quickest: return 1.0
        case .manyShort: return 0.4
        }
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

extension Array {
    subscript(safe index: Int) -> Element? {
        indices.contains(index) ? self[index] : nil
    }
}
