// Maneuver banner (wayfinder #67, ADR 0012 point 3's HUD extended to turn-by-turn guidance):
// sits across the top of the drive overlay while `DriveStore.banner` is non-nil -- a countdown
// distance, the maneuver icon, the instruction, an optional signage line, and an optional
// "then ..." preview of a step chained close behind (StepTracker.thenChainThresholdM). Visual
// idiom matches DriveCard (regularMaterial, shadow) at a tighter corner radius since it's a
// top-edge banner, not a bottom-anchored card. "maneuver-banner" rides the leaf icon Image, NOT
// the container -- a container identifier shadows the descendant Text identifiers from XCUITest
// (banner-distance stopped resolving), the same trap DriveCard's capsule comment records.
import SwiftUI

struct ManeuverBannerView: View {
    let banner: DriveStore.ManeuverBanner

    var body: some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: banner.iconSystemName)
                .font(.title)
                .fontWeight(.bold)
                .accessibilityIdentifier("maneuver-banner")
            VStack(alignment: .leading, spacing: 2) {
                Text(StepFormatter.formatDistance(banner.distanceM))
                    .font(.title2).bold()
                    .accessibilityIdentifier("banner-distance")
                Text(banner.primary)
                    .font(.headline)
                    .lineLimit(2)
                    .accessibilityIdentifier("banner-primary")
                if let secondary = banner.secondary {
                    Text(secondary)
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                        .lineLimit(1)
                }
                if let then = banner.then {
                    Text("then \(then)")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                        .accessibilityIdentifier("banner-then")
                }
            }
            Spacer()
        }
        .padding(16)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
    }
}
