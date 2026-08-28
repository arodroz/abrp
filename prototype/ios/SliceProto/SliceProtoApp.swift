// Prototype: planner-UI variants (wayfinder #23); locked to the decided Variant D.
// Throwaway branch prototype/planner-ui.

import CoreLocation
import SwiftUI

/// Parses `-autotest-destination "lat,lon"` from the launch arguments, so an external
/// `simctl launch ... -autotest-destination 51.22,4.40` can exercise the geocode-free back
/// half of the search flow end to end.
private func parseAutotestDestination() -> CLLocationCoordinate2D? {
    let args = ProcessInfo.processInfo.arguments
    guard let idx = args.firstIndex(of: "-autotest-destination"), idx + 1 < args.count else { return nil }
    let parts = args[idx + 1].split(separator: ",")
    guard parts.count == 2, let lat = Double(parts[0]), let lon = Double(parts[1]) else { return nil }
    return CLLocationCoordinate2D(latitude: lat, longitude: lon)
}

/// Parses `-benchmark-flyover` from the launch arguments (no value needed), like
/// `-autotest-destination` above: when present, re-runs the original wayfinder #15
/// CADisplayLink fps/memory probe + camera flyover once the initial plan completes, to
/// re-measure against the fixed basemap style. See BenchmarkFlyover.swift.
private func parseBenchmarkFlyover() -> Bool {
    ProcessInfo.processInfo.arguments.contains("-benchmark-flyover")
}

enum PlannerVariant: String, CaseIterable {
    case sheetStack = "A"
    case formFirst = "B"
    case drivingOverlay = "C"
    case google = "D"

    var label: String {
        switch self {
        case .sheetStack: return "Sheet stack"
        case .formFirst: return "Form-first"
        case .drivingOverlay: return "Driving overlay"
        case .google: return "Google"
        }
    }
}

struct RootView: View {
    @StateObject private var store = PlanStore()

    // Autotest launch-argument hook (see parseAutotestDestination): fires once the initial
    // plan (planVersion == 1) completes, then reports the result of the autotest plan.
    @State private var autotestDestination: CLLocationCoordinate2D?
    @State private var autotestFired = false
    @State private var autotestPending = false

    // Benchmark launch-argument hook (see parseBenchmarkFlyover): fires once the initial plan
    // (planVersion == 1) completes, same as the autotest hook above.
    @State private var benchmarkFlyoverRequested = false
    @State private var benchmarkFired = false
    @State private var benchmark: BenchmarkFlyover?

    private let variant: PlannerVariant = .google

    var body: some View {
        ZStack {
            if !store.missingPaths.isEmpty {
                missingView
            } else {
                Group {
                    switch variant {
                    case .sheetStack: VariantASheetStack(store: store)
                    case .formFirst: VariantBFormFirst(store: store)
                    case .drivingOverlay: VariantCDrivingOverlay(store: store)
                    case .google: VariantDGoogle(store: store)
                    }
                }
            }
        }
        .onAppear {
            autotestDestination = parseAutotestDestination()
            benchmarkFlyoverRequested = parseBenchmarkFlyover()
        }
        .onChange(of: store.planVersion) { _, newValue in
            if autotestPending {
                autotestPending = false
                print("PROTO autotest plan ok stops=\(store.plan?.stops.count ?? 0)")
                return
            }
            // SwiftUI coalesces rapid planVersion bumps (planner-init plan + style-load replan),
            // so the first delivery may already be > 1 — fire on the first delivery, whatever it is.
            guard newValue >= 1, !autotestFired, let destination = autotestDestination else { return }
            autotestFired = true
            autotestPending = true
            store.planTo(destination: destination)
        }
        .onChange(of: store.planVersion) { _, newValue in
            // Same first-delivery gate as the autotest hook above.
            guard benchmarkFlyoverRequested, newValue >= 1, !benchmarkFired else { return }
            benchmarkFired = true
            let flyover = BenchmarkFlyover(
                mapView: store.mapView, routeCoordinates: store.plan?.routeCoordinates ?? [])
            benchmark = flyover
            flyover.start()
        }
        .onChange(of: store.planError) { _, newValue in
            guard autotestPending, let newValue else { return }
            autotestPending = false
            print("PROTO autotest plan error \(newValue.message)")
        }
    }

    private var missingView: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("PROTO ERROR: missing files")
                .font(.headline)
                .foregroundColor(.white)
            ForEach(store.missingPaths, id: \.self) { p in
                Text(p)
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(.white)
            }
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.black)
    }
}

@main
struct SliceProtoApp: App {
    var body: some Scene {
        WindowGroup {
            RootView()
        }
    }
}
