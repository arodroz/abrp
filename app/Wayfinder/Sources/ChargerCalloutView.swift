// Charger tap callout (wayfinder #40): a small informational card shown when the user taps
// an unclustered Charger dot on the map (the chargers layer is NOT clustered -- see
// ChargersLayer.swift). Ported from prototype/planner-ui's ChargerCallout/ChargerCalloutCard
// (VariantDGoogle.swift), dropping the prototype's cluster-zoom branch since there's nothing
// to cluster here.
import SwiftUI

/// A tapped Charger's callout content, read from the chargers point layer's feature
/// attributes (name, power_kw always present -- see ChargersLayer.makeSource; operator only
/// when the cpack-1 record has one).
struct ChargerCalloutInfo: Identifiable {
    let id = UUID()
    let name: String
    let powerKw: Double
    let operatorName: String?
}

/// Dismissed by its own x button or by a map tap that misses every charger (RootView clears
/// the callout state either way).
struct ChargerCalloutView: View {
    let info: ChargerCalloutInfo
    var onDismiss: () -> Void

    var body: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                Text(info.name).font(.headline).lineLimit(1)
                Text(
                    "\(Int(info.powerKw)) kW" + (info.operatorName.map { " \u{00B7} \($0)" } ?? "")
                )
                .font(.caption)
                .foregroundColor(.secondary)
            }
            Spacer()
            Button(action: onDismiss) {
                Image(systemName: "xmark.circle.fill").foregroundColor(.secondary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
    }
}
