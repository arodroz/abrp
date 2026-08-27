// VariantC "Driving overlay" (Google Maps navigation idiom): full-screen map, a slim
// always-visible SoC chart strip docked above a horizontal paging card carousel (one card
// per waypoint), and a circular settings button that slides an overlay panel in from the
// trailing edge (not a sheet).

import CoreLocation
import SwiftUI

private enum Waypoint: Identifiable {
    case origin
    case stop(Int)
    case destination

    var id: String {
        switch self {
        case .origin: return "origin"
        case .stop(let i): return "stop-\(i)"
        case .destination: return "destination"
        }
    }
}

struct VariantCDrivingOverlay: View {
    @ObservedObject var store: PlanStore
    @State private var selectedIndex: Int = 0
    @State private var showSettings = false

    private var waypoints: [Waypoint] {
        var w: [Waypoint] = [.origin]
        if let plan = store.plan {
            w += (0..<plan.stops.count).map { Waypoint.stop($0) }
        }
        w.append(.destination)
        return w
    }

    var body: some View {
        ZStack(alignment: .bottom) {
            PlannerMapView(store: store).ignoresSafeArea()

            VStack(spacing: 8) {
                SoCChartView(store: store, height: 90, showAxes: false)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                    .background(.thinMaterial)
                    .clipShape(RoundedRectangle(cornerRadius: 14))
                    .padding(.horizontal)

                TabView(selection: $selectedIndex) {
                    ForEach(Array(waypoints.enumerated()), id: \.element.id) { idx, wp in
                        waypointCard(wp).tag(idx)
                    }
                }
                .tabViewStyle(.page(indexDisplayMode: .never))
                .frame(height: 170)
            }
            .padding(.bottom, 90) // clear the floating variant switcher pill

            HStack {
                Spacer()
                Button {
                    withAnimation { showSettings.toggle() }
                } label: {
                    Image(systemName: "gearshape.fill")
                        .padding(14)
                        .background(Color.black.opacity(0.6), in: Circle())
                        .foregroundColor(.white)
                }
                .padding()
            }
            .frame(maxHeight: .infinity, alignment: .top)

            if showSettings {
                HStack(spacing: 0) {
                    Spacer()
                    SlideInSettingsPanel(store: store, isPresented: $showSettings)
                }
                .ignoresSafeArea()
                .transition(.move(edge: .trailing))
                .zIndex(1)
            }
        }
        .onChange(of: selectedIndex) { _, newValue in
            panToWaypoint(waypoints[safe: newValue])
        }
    }

    private func panToWaypoint(_ wp: Waypoint?) {
        guard let wp else { return }
        switch wp {
        case .origin:
            store.panMap(to: CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        case .destination:
            store.panMap(to: CLLocationCoordinate2D(latitude: 52.3676, longitude: 4.9041))
        case .stop(let i):
            if let stop = store.plan?.stops[safe: i] {
                store.panMap(to: stop.coordinate)
            }
        }
    }

    @ViewBuilder
    private func waypointCard(_ wp: Waypoint) -> some View {
        switch wp {
        case .origin:
            card(title: "Luxembourg", subtitle: "Origin \u{00B7} depart at \(formatSocPct(store.departSoc))")
        case .destination:
            let subtitle = store.plan.map { "Destination \u{00B7} arrive at \(formatSocPct($0.arrivalSoc))" } ?? "Destination"
            card(title: "Amsterdam", subtitle: subtitle)
        case .stop(let i):
            if let stop = store.plan?.stops[safe: i] {
                VStack(alignment: .leading, spacing: 8) {
                    Text(stop.name).font(.headline)
                    Text(
                        "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))\u{2192}"
                            + "\(formatSocPct(store.stopOverrides[i] ?? stop.departSoc)) \u{00B7} "
                            + formatDuration(store.displayedChargeS(for: stop, index: i))
                    )
                    .font(.caption)
                    .foregroundColor(.secondary)
                    Slider(
                        value: Binding(
                            get: { store.stopOverrides[i] ?? stop.departSoc },
                            set: { store.stopOverrides[i] = $0 }
                        ),
                        in: stop.arrivalSoc...1.0
                    )
                }
                .padding()
                .background(.regularMaterial)
                .clipShape(RoundedRectangle(cornerRadius: 16))
                .padding(.horizontal, 24)
            }
        }
    }

    private func card(title: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(title).font(.headline)
            Text(subtitle).font(.caption).foregroundColor(.secondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding()
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 16))
        .padding(.horizontal, 24)
    }
}

private struct SlideInSettingsPanel: View {
    @ObservedObject var store: PlanStore
    @Binding var isPresented: Bool

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Settings").font(.headline)
                Spacer()
                Button {
                    withAnimation { isPresented = false }
                } label: {
                    Image(systemName: "xmark.circle.fill")
                        .foregroundColor(.secondary)
                }
            }
            .padding()
            Form { SettingsFieldsSections(store: store) }
        }
        .frame(width: 320)
        .background(.regularMaterial)
    }
}
