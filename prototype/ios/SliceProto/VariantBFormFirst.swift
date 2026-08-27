// VariantB "Form-first" (classic ABRP idiom): the first screen IS the plan form (fixed
// origin/destination + inline settings + a prominent Plan button). Plan pushes a results
// screen with a segmented Map / Itinerary / SoC Chart switch.

import SwiftUI

struct VariantBFormFirst: View {
    @ObservedObject var store: PlanStore
    @State private var showResults = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Trip") {
                    LabeledContent("Origin", value: "Luxembourg")
                    LabeledContent("Destination", value: "Amsterdam")
                }
                SettingsFieldsSections(store: store)
            }
            .navigationTitle("Plan a trip")
            .safeAreaInset(edge: .bottom) {
                Button {
                    store.runPlan()
                    showResults = true
                } label: {
                    Text("Plan")
                        .font(.headline)
                        .frame(maxWidth: .infinity)
                        .padding(.vertical, 4)
                }
                .buttonStyle(.borderedProminent)
                .padding()
                .background(.bar)
            }
            .navigationDestination(isPresented: $showResults) {
                VariantBResultsView(store: store)
            }
        }
    }
}

private struct VariantBResultsView: View {
    @ObservedObject var store: PlanStore
    @State private var page: Int = 0

    var body: some View {
        VStack(spacing: 0) {
            Picker("Page", selection: $page) {
                Text("Map").tag(0)
                Text("Itinerary").tag(1)
                Text("SoC Chart").tag(2)
            }
            .pickerStyle(.segmented)
            .padding()

            switch page {
            case 0:
                PlannerMapView(store: store)
            case 1:
                itineraryList
            default:
                chartPage
            }
        }
        .navigationTitle("Plan")
        .navigationBarTitleDisplayMode(.inline)
    }

    private var itineraryList: some View {
        List {
            Section {
                Text("Luxembourg (origin) \u{2014} depart at \(formatSocPct(store.departSoc))")
            }
            if let plan = store.plan {
                Section("Charging Stops") {
                    ForEach(plan.stops) { stop in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(stop.name).font(.headline)
                            Text(
                                "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))"
                                    + "\u{2192}\(formatSocPct(stop.departSoc)) \u{00B7} \(formatDuration(stop.chargeS))"
                            )
                            .font(.caption)
                            .foregroundColor(.secondary)
                        }
                    }
                }
                Section {
                    Text("Amsterdam (destination) \u{2014} arrive at \(formatSocPct(plan.arrivalSoc))")
                }
            } else {
                Section {
                    Text(store.isPlanning ? "Planning\u{2026}" : "No plan yet")
                        .foregroundColor(.secondary)
                }
            }
        }
        .listStyle(.insetGrouped)
    }

    private var chartPage: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                SoCChartView(store: store, height: 320)
                    .padding()
                if let plan = store.plan {
                    ForEach(plan.stops) { stop in
                        VStack(alignment: .leading, spacing: 4) {
                            Text(stop.name).font(.subheadline).bold()
                            Text(
                                "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))"
                                    + "\u{2192}\(formatSocPct(stop.departSoc)) \u{00B7} \(formatDuration(stop.chargeS))"
                            )
                            .font(.caption)
                            .foregroundColor(.secondary)
                        }
                        .padding(.horizontal)
                    }
                }
            }
        }
    }
}
