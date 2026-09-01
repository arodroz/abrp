// Compact drive HUD card (wayfinder #60, ADR 0012 points 3-4): replaces the result card while
// driving. Collapsed shows the ETA clock plus a one-line remaining-distance/remaining-time/
// next-Charging-Stop-arrival-SoC summary and the live SoC, tappable for wayfinder #63's dash
// correction; expanded reveals the existing SoC chart (wayfinder #43) with its marker pinned to
// the live snapped position instead of the scrub selection (SoCChartView.pinnedDistanceM).
// Visual idiom matches ResultCard (regularMaterial, rounded 20, shadow, drag-handle capsule) --
// a separate view rather than reusing ResultCard directly, since the drive HUD's fields
// (ETA/remaining/next-stop) don't map onto ResultCard's plan-summary layout, which is hidden
// entirely while driving.
//
// Live SoC chip (wayfinder #79): beside the predicted-SoC button, reads `telemetryStore`'s
// decoded Display SoC through `LiveSocPresentation` -- hidden with no reading yet, `.primary`
// while fresh, dimmed `.secondary` once stale. Wrapped in a `TimelineView` so the fresh->stale
// flip happens on wall-clock time alone, not just when a new poll happens to land.
//
// SoC chart overhaul (wayfinder #83): the expanded chart now also gets the actual-SoC trail
// (`driveStore.socTrail`), the charger-floor threshold for its danger band/callouts
// (`store.chargerArrivalMinSoc`), and a freshness-checked live SoC for its position dot -- same
// TimelineView idiom as the chip above, its own instance since it wraps a different value.
import SwiftUI

struct DriveCard: View {
    let store: PlanStore
    let driveStore: DriveStore
    let telemetryStore: TelemetryLinkStore
    let onSocTap: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Capsule()
                .fill(Color.secondary.opacity(0.4))
                .frame(width: 36, height: 5)
                .frame(maxWidth: .infinity)
                .padding(.top, 8)
                // Leaf shape (no descendants) -- same reasoning as ResultCard's own identical
                // comment: an ancestor identifier here would shadow `summary`'s own
                // "drive-card-toggle" Button instead of just tagging the container.
                .accessibilityIdentifier("drive-card")

            if let hud = driveStore.hud {
                summary(hud)
            }

            if driveStore.driveCardExpanded {
                Divider().padding(.top, 4)
                // wayfinder #83: the live-position dot prefers the live OBD Display SoC over the
                // model's curve estimate, same freshness gate as `liveSocChip` above -- wrapped in
                // its own TimelineView so a reading that goes stale mid-expansion (no new fix) still
                // falls back to the curve estimate without waiting on the next `ingest`.
                TimelineView(.periodic(from: .now, by: 5)) { context in
                    let liveSocPct: Double? = {
                        guard let soc = telemetryStore.liveDisplaySoc, let at = telemetryStore.lastReadingAt,
                              context.date.timeIntervalSince(at) <= TelemetryLinkStore.snapshotFreshnessS
                        else { return nil }
                        return soc
                    }()
                    SoCChartView(
                        store: store, height: 160, pinnedDistanceM: driveStore.distanceAlongRouteM,
                        trail: driveStore.socTrail, liveSocPct: liveSocPct,
                        amberFloorPct: store.chargerArrivalMinSoc * 100
                    )
                }
                .padding(.horizontal)
                .padding(.vertical, 8)
            } else {
                Spacer().frame(height: 12)
            }
        }
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
    }

    private func summary(_ hud: DriveStore.DriveHud) -> some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                Text(formatClock(hud.etaDate)).font(.title2).bold()
                Text(
                    "\(formatKm(hud.remainingDistM)) \u{00B7} \(formatDuration(hud.remainingTimeS))"
                        + " \u{00B7} \(hud.nextLabel) arrive \(formatSocPct(hud.nextArrivalSoc))"
                )
                .font(.subheadline)
                .foregroundColor(.secondary)
                .lineLimit(1)
                .truncationMode(.tail)
            }
            Spacer()
            // Current model SoC at the live position (ADR 0012 point 5) -- tappable (wayfinder #63):
            // opens RootView's "Correct SoC" alert so the driver can anchor the model to the dash.
            Button(action: onSocTap) {
                HStack(spacing: 3) {
                    Image(systemName: "bolt.batteryblock")
                    Text(formatSocPct(hud.socAtPosition))
                }
                .font(.subheadline.bold())
                .foregroundColor(.secondary)
                .padding(8)
            }
            .accessibilityIdentifier("drive-soc-button")
            TimelineView(.periodic(from: .now, by: 5)) { context in
                liveSocChip(LiveSocPresentation.compute(
                    soc: telemetryStore.liveDisplaySoc,
                    age: telemetryStore.lastReadingAt.map { context.date.timeIntervalSince($0) }
                ))
            }
            Button {
                withAnimation(.spring()) { driveStore.driveCardExpanded.toggle() }
            } label: {
                Image(systemName: driveStore.driveCardExpanded ? "chevron.down" : "chevron.up")
                    .font(.headline)
                    .foregroundColor(.secondary)
                    .padding(8)
            }
            .accessibilityIdentifier("drive-card-toggle")
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
    }

    /// "HH:mm" (24h, device timezone) -- same format as ResultCard's arrival clock, but reading
    /// `hud.etaDate` directly rather than adding a duration to now.
    private func formatClock(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }

    @ViewBuilder
    private func liveSocChip(_ presentation: LiveSocPresentation) -> some View {
        switch presentation {
        case .hidden:
            EmptyView()
        case .fresh(let soc):
            liveSocLabel(soc, color: .primary, opacity: 1.0)
        case .stale(let soc):
            liveSocLabel(soc, color: .secondary, opacity: 0.5)
        }
    }

    private func liveSocLabel(_ soc: Double, color: Color, opacity: Double) -> some View {
        HStack(spacing: 3) {
            Image(systemName: "antenna.radiowaves.left.and.right")
            Text(formatLiveSocPct(soc))
        }
        .font(.subheadline.bold())
        .foregroundColor(color)
        .opacity(opacity)
        .padding(8)
        .accessibilityIdentifier("drive-live-soc")
    }
}

/// `latestReadings` values are already percent (0-100), unlike `formatSocPct`'s fraction input
/// (PlannerModels.swift) -- a distinct formatter rather than a `/100` call site to avoid an
/// easy-to-miss silent unit bug at every future call site.
func formatLiveSocPct(_ pct: Double) -> String {
    "\(Int(pct.rounded()))%"
}
