// The Plan result card (wayfinder #43): a Google-directions-style bottom sheet -- collapsed
// shows the arrival clock, total time/distance/arrival SoC, and a horizontal row of Charging
// Stop chips; expanded (chevron or swipe) reveals the itinerary and the SoC-over-distance
// chart. Ported from prototype/planner-ui's VariantDGoogle.swift `ResultCard`, adapted to the
// store's typed `displayedPlan` (FfiPlan), and, new to this ticket, the stop-free alternative
// toggle row (ADR 0010 point 5). Dropped the prototype's per-chip tap handler -- it opened the
// per-stop SoC override UI, which is out of scope for this ticket. The Go button (wayfinder
// #59, ADR 0012 point 2) sits in the summary row, visible only when `driveStore.canGo` --
// origin provenance is current-location and no drive is already in progress.
import PlannerKit
import SwiftUI

struct ResultCard: View {
    let store: PlanStore
    let driveStore: DriveStore

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Capsule()
                .fill(Color.secondary.opacity(0.4))
                .frame(width: 36, height: 5)
                .frame(maxWidth: .infinity)
                .padding(.top, 8)
                // On this leaf shape (no descendants) rather than the card's outer VStack --
                // see the container's own comment below for why an ancestor identifier isn't
                // safe here.
                .accessibilityIdentifier("result-card")

            summary

            if let mainPlan = store.plan, mainPlan.alternative != nil {
                alternativeToggleRow(mainPlan)
            }

            if let plan = store.displayedPlan {
                let stops = ChargingStopVM.stops(from: plan)
                if !stops.isEmpty {
                    stopChipsRow(stops)
                }
            }

            if store.cardExpanded {
                Divider().padding(.top, 4)
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        itinerary
                        SoCChartView(store: store)
                            .padding(.horizontal)
                    }
                    .padding(.vertical, 12)
                }
            } else {
                Spacer().frame(height: 12)
            }
        }
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
        .gesture(
            DragGesture()
                .onEnded { value in
                    if value.translation.height > 40 {
                        withAnimation(.spring()) { store.cardExpanded = false }
                    } else if value.translation.height < -40 {
                        withAnimation(.spring()) { store.cardExpanded = true }
                    }
                }
        )
        // No identifier on this container: it wraps `summary`'s own "result-card-toggle"
        // Button, and tagging an ancestor here overrides/shadows that descendant's specific
        // identifier instead of just tagging the container (same failure mode documented on
        // search-pill/routeEditorCard's row) -- "result-card-toggle" existing is itself proof
        // the result card is showing, so no separate container identifier is needed.
    }

    private var summary: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                if let plan = store.displayedPlan {
                    Text(formatArrivalClock(plan.totalTimeS)).font(.title2).bold()
                    Text(
                        "\(formatDuration(plan.totalTimeS)) \u{00B7} \(formatKm(plan.totalDistM))"
                            + " \u{00B7} arrive \(formatSocPct(plan.socCurve.last?.soc ?? 0))"
                    )
                    .font(.subheadline)
                    .foregroundColor(.secondary)
                } else {
                    Text(store.isPlanning ? "Planning\u{2026}" : "No plan yet").font(.title2).bold()
                }
            }
            Spacer()
            if driveStore.canGo {
                Button {
                    driveStore.go()
                } label: {
                    Label("Go", systemImage: "location.north.fill")
                        .font(.headline).bold()
                        .foregroundColor(.white)
                        .padding(.horizontal, 16)
                        .padding(.vertical, 10)
                        .background(Color.blue, in: Capsule())
                }
                .accessibilityIdentifier("go-button")
            }
            Button {
                withAnimation(.spring()) { store.cardExpanded.toggle() }
            } label: {
                Image(systemName: store.cardExpanded ? "chevron.down" : "chevron.up")
                    .font(.headline)
                    .foregroundColor(.secondary)
                    .padding(8)
            }
            .accessibilityIdentifier("result-card-toggle")
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
    }

    /// Off: the main plan's single stop can be skipped by taking the stop-free alternative,
    /// arriving at a lower (but still planner-reported) SoC. On: the alternative is displayed
    /// instead, with a way back to the main plan. The alt's failing leg carries an
    /// "ArrivalSocBelowWanted"-style flag (FfiLeg.flags) -- not parsed here, since the
    /// presence of `alternative` is already the signal that it applies.
    private func alternativeToggleRow(_ mainPlan: FfiPlan) -> some View {
        Button {
            withAnimation { store.toggleAlternative() }
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "bolt.slash").foregroundColor(.blue)
                if store.showingAlternative {
                    Text("Showing stop-free plan \u{00B7} arrives below wanted SoC")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Spacer()
                    Text("Back").font(.caption).bold().foregroundColor(.blue)
                } else {
                    let stopMinutes = Int(((mainPlan.stops.first?.chargeS ?? 0) / 60).rounded())
                    let altArrivalSoc = formatSocPct(mainPlan.alternative?.socCurve.last?.soc ?? 0)
                    Text("Skip the \(stopMinutes)-min stop \u{2014} arrive at \(altArrivalSoc)")
                        .font(.caption)
                        .foregroundColor(.secondary)
                    Spacer()
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 6)
        }
        .buttonStyle(.plain)
    }

    private func stopChipsRow(_ stops: [ChargingStopVM]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(stops) { stop in
                    HStack(spacing: 6) {
                        Image(systemName: "bolt.fill").foregroundColor(.orange)
                        VStack(alignment: .leading, spacing: 0) {
                            Text(stop.name)
                                .font(.caption).bold()
                                .lineLimit(1)
                                .truncationMode(.tail)
                            Text(
                                "+\(Int(((stop.departSoc - stop.arrivalSoc) * 100).rounded()))% \u{00B7} "
                                    + formatDuration(stop.chargeS)
                            )
                            .font(.caption2)
                            .foregroundColor(.secondary)
                        }
                        .frame(maxWidth: 110, alignment: .leading)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 6)
                    .background(Color.orange.opacity(0.12), in: Capsule())
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
    }

    private var itinerary: some View {
        VStack(alignment: .leading, spacing: 0) {
            row(title: "Origin", subtitle: "Depart at \(formatSocPct(store.departSoc))")
            if let plan = store.displayedPlan {
                ForEach(ChargingStopVM.stops(from: plan)) { stop in
                    stopRow(stop)
                }
                row(title: "Destination", subtitle: "Arrive at \(formatSocPct(plan.socCurve.last?.soc ?? 0))")
            }
        }
        .accessibilityIdentifier("itinerary")
    }

    private func row(title: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.headline)
            Text(subtitle).font(.caption).foregroundColor(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    private func stopRow(_ stop: ChargingStopVM) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(stop.name).font(.headline)
            Text(
                "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))\u{2192}"
                    + "\(formatSocPct(stop.departSoc)) \u{00B7} " + formatDuration(stop.chargeS)
            )
            .font(.caption)
            .foregroundColor(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    /// "HH:mm" (24h, device timezone) for now + total_time_s, Google-directions-style.
    private func formatArrivalClock(_ totalTimeS: Double) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: Date().addingTimeInterval(totalTimeS))
    }
}
