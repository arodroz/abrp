// All plan request inputs, grouped for a Form. `SettingsFieldsSections` is the reusable
// content (embeddable directly inside any Form); `SettingsForm` wraps it for modal/sheet
// presentation.

import SwiftUI

struct SettingsFieldsSections: View {
    @ObservedObject var store: PlanStore

    var body: some View {
        Section("Battery") {
            VStack(alignment: .leading) {
                Text("Departure SoC: \(formatSocPct(store.departSoc))")
                Slider(value: $store.departSoc, in: 0.1...1.0)
            }
            VStack(alignment: .leading) {
                Text("Destination Arrival SoC: \(formatSocPct(store.destinationArrivalSoc))")
                Slider(value: $store.destinationArrivalSoc, in: 0...0.5)
            }
            VStack(alignment: .leading) {
                Text("Charger Arrival SoC: \(formatSocPct(store.chargerArrivalSoc))")
                Slider(value: $store.chargerArrivalSoc, in: 0...0.5)
            }
            VStack(alignment: .leading) {
                Text("Charger Max SoC: \(formatSocPct(store.chargerMaxSoc))")
                Slider(value: $store.chargerMaxSoc, in: 0.5...1.0)
            }
        }

        Section("Route") {
            Stepper("Max speed: \(Int(store.maxSpeedKmh)) km/h", value: $store.maxSpeedKmh, in: 80...180, step: 5)
            Picker("Stops Bias", selection: $store.stopsBias) {
                ForEach(StopsBias.allCases) { bias in
                    Text(bias.rawValue).tag(bias)
                }
            }
            .pickerStyle(.segmented)
        }

        Section("Conditions") {
            Stepper("Temperature: \(Int(store.temperatureC))°C", value: $store.temperatureC, in: -20...40, step: 1)
            Stepper("Extra weight: \(Int(store.extraWeightKg)) kg", value: $store.extraWeightKg, in: 0...300, step: 10)
        }

        Section("Vehicle") {
            Stepper(
                "Reference Consumption: \(Int(store.referenceConsumptionWhKm)) Wh/km",
                value: $store.referenceConsumptionWhKm, in: 120...260, step: 5)
        }
    }
}

struct SettingsForm: View {
    @ObservedObject var store: PlanStore
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form { SettingsFieldsSections(store: store) }
                .navigationTitle("Settings")
                .toolbar {
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Done") { dismiss() }
                    }
                }
        }
    }
}
