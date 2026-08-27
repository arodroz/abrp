// VariantA "Sheet stack" (Apple Maps idiom): full-screen map, a custom draggable bottom
// panel (not .sheet, so it never covers the floating variant switcher pill) with plan
// summary + itinerary + SoC chart, and a gear button opening Settings as a real modal sheet.

import SwiftUI

struct VariantASheetStack: View {
    @ObservedObject var store: PlanStore
    @State private var showSettings = false
    @State private var panelHeightFraction: CGFloat = 0.42
    @State private var dragStartFraction: CGFloat = 0.42
    @State private var expandedStopID: Int?

    private let minFraction: CGFloat = 0.12
    private let maxFraction: CGFloat = 0.82

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .top) {
                PlannerMapView(store: store).ignoresSafeArea()

                HStack {
                    Spacer()
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape.fill")
                            .font(.title2)
                            .padding(12)
                            .background(.thinMaterial, in: Circle())
                    }
                }
                .padding()

                VStack {
                    Spacer()
                    panel(height: geo.size.height * panelHeightFraction)
                }
                .ignoresSafeArea(edges: .bottom)
            }
        }
        .sheet(isPresented: $showSettings) { SettingsForm(store: store) }
    }

    private func panel(height: CGFloat) -> some View {
        VStack(spacing: 0) {
            Capsule()
                .fill(Color.secondary.opacity(0.5))
                .frame(width: 40, height: 5)
                .padding(.top, 8)
                .padding(.bottom, 6)
                .contentShape(Rectangle())
                .gesture(dragGesture)

            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    summaryHeader
                    itinerary
                    if store.plan != nil {
                        SoCChartView(store: store)
                            .padding(.horizontal)
                    }
                }
                .padding(.bottom, 24)
            }
        }
        .frame(height: height)
        .frame(maxWidth: .infinity)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .shadow(radius: 8)
    }

    private var dragGesture: some Gesture {
        DragGesture()
            .onChanged { value in
                let screenHeight = UIScreen.main.bounds.height
                let delta = -value.translation.height / max(screenHeight, 1)
                panelHeightFraction = min(max(dragStartFraction + delta, minFraction), maxFraction)
            }
            .onEnded { _ in
                dragStartFraction = panelHeightFraction
            }
    }

    private var summaryHeader: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let plan = store.plan {
                Text(formatDuration(plan.totalTimeS))
                    .font(.title2).bold()
                Text("\(formatKm(plan.totalDistM)) \u{00B7} arrive at \(formatSocPct(plan.arrivalSoc))")
                    .font(.subheadline)
                    .foregroundColor(.secondary)
            } else {
                Text(store.isPlanning ? "Planning\u{2026}" : "No plan yet")
                    .font(.title2).bold()
            }
        }
        .padding(.horizontal)
    }

    private var itinerary: some View {
        VStack(alignment: .leading, spacing: 0) {
            waypointRow(title: "Luxembourg (origin)", subtitle: "Depart at \(formatSocPct(store.departSoc))")

            if let plan = store.plan {
                ForEach(Array(plan.stops.enumerated()), id: \.element.id) { idx, stop in
                    stopRow(stop: stop, index: idx)
                }
                waypointRow(title: "Amsterdam (destination)", subtitle: "Arrive at \(formatSocPct(plan.arrivalSoc))")
            } else {
                waypointRow(title: "Amsterdam (destination)", subtitle: "")
            }
        }
    }

    private func waypointRow(title: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.headline)
                if !subtitle.isEmpty {
                    Text(subtitle).font(.caption).foregroundColor(.secondary)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 8)
            Divider().padding(.leading)
        }
    }

    private func stopRow(stop: ChargingStopVM, index: Int) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                Button {
                    withAnimation { expandedStopID = expandedStopID == stop.id ? nil : stop.id }
                } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(stop.name).font(.headline)
                            Text(
                                "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))\u{2192}"
                                    + "\(formatSocPct(store.stopOverrides[index] ?? stop.departSoc)) \u{00B7} "
                                    + formatDuration(store.displayedChargeS(for: stop, index: index))
                            )
                            .font(.caption)
                            .foregroundColor(.secondary)
                        }
                        Spacer()
                        Image(systemName: expandedStopID == stop.id ? "chevron.up" : "chevron.down")
                            .foregroundColor(.secondary)
                    }
                }
                .buttonStyle(.plain)

                if expandedStopID == stop.id {
                    VStack(alignment: .leading, spacing: 4) {
                        Text("Charge to \(formatSocPct(store.stopOverrides[index] ?? stop.departSoc))")
                            .font(.caption)
                        Slider(
                            value: Binding(
                                get: { store.stopOverrides[index] ?? stop.departSoc },
                                set: { store.stopOverrides[index] = $0 }
                            ),
                            in: stop.arrivalSoc...1.0
                        )
                    }
                    .padding(.top, 4)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 8)
            Divider().padding(.leading)
        }
    }
}
