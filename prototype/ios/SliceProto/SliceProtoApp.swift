// Prototype: three planner-UI variants (wayfinder #23), switchable via floating pill.
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
    @AppStorage("plannerUIVariant") private var variantRaw: String = PlannerVariant.google.rawValue

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

    private var variant: PlannerVariant { PlannerVariant(rawValue: variantRaw) ?? .google }

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

            // Above everything else in the app, including sheets/panels raised by a variant.
            VariantSwitcherPill(variantRaw: $variantRaw)
                .zIndex(999)
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

/// Fixed bottom-center high-contrast pill, deliberately not part of any variant's design
/// language, so it always reads as prototype scaffolding. Cycles with wraparound and
/// persists the chosen variant across relaunch via @AppStorage.
struct VariantSwitcherPill: View {
    @Binding var variantRaw: String

    private var variant: PlannerVariant { PlannerVariant(rawValue: variantRaw) ?? .google }
    private var all: [PlannerVariant] { PlannerVariant.allCases }

    var body: some View {
        VStack {
            Spacer()
            HStack(spacing: 12) {
                Button(action: cyclePrev) {
                    Text("\u{25C0}").bold()
                }
                Text("\(variant.rawValue) \u{00B7} \(variant.label)")
                    .font(.system(.footnote, design: .monospaced))
                    .bold()
                Button(action: cycleNext) {
                    Text("\u{25B6}").bold()
                }
            }
            .foregroundColor(.white)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Color.black)
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color.yellow, lineWidth: 2))
            .padding(.bottom, 24)
        }
    }

    private func cyclePrev() {
        let idx = all.firstIndex(of: variant) ?? 0
        variantRaw = all[(idx - 1 + all.count) % all.count].rawValue
    }

    private func cycleNext() {
        let idx = all.firstIndex(of: variant) ?? 0
        variantRaw = all[(idx + 1) % all.count].rawValue
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
