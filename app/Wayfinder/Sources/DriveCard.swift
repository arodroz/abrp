// Compact drive HUD card (wayfinder #60, ADR 0012 points 3-4): replaces the result card while
// driving. Collapsed shows the ETA clock plus a one-line remaining-distance/remaining-time/
// next-Charging-Stop-arrival-SoC summary; expanded reveals the existing SoC chart (wayfinder
// #43) with its marker pinned to the live snapped position instead of the scrub selection
// (SoCChartView.pinnedDistanceM). Visual idiom matches ResultCard (regularMaterial, rounded 20,
// shadow, drag-handle capsule) -- a separate view rather than reusing ResultCard directly,
// since the drive HUD's fields (ETA/remaining/next-stop) don't map onto ResultCard's
// plan-summary layout, which is hidden entirely while driving.
import SwiftUI

struct DriveCard: View {
    let store: PlanStore
    let driveStore: DriveStore

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
                SoCChartView(store: store, height: 160, pinnedDistanceM: driveStore.distanceAlongRouteM)
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
}
