// The settings sheet (wayfinder #44): SoC sliders, Stops Bias, conditions (temperature,
// headwind), a Reference Consumption override, the (fixed) vehicle, and a light/dark
// appearance override. Ported from prototype/planner-ui's SettingsForm.swift, re-typed off
// PlanStore's @Observable (via @Bindable, not the prototype's @ObservedObject) so every field
// binds straight to a PlanStore var and its own didSet-triggered replan -- there's no local
// @State here. Dropped "Max speed" (Speed Caps are a planner OUTPUT, not a request field --
// ADR 0010 point 1) and "Extra weight" (no request field); added the Headwind stepper and the
// Appearance section. The Packs section (wayfinder #47) lists every index region against
// PackInstaller's rows with Install/Update/Use/Delete actions.
import Foundation
import PlannerKit
import SwiftUI

struct SettingsForm: View {
    @Bindable var store: PlanStore
    @Bindable var installer: PackInstaller
    @Environment(\.dismiss) private var dismiss
    @State private var pendingDeleteRegionId: String?

    var body: some View {
        NavigationStack {
            Form {
                Section("Packs") {
                    ForEach(installer.rows) { row in
                        packRow(row)
                    }
                    Toggle("Allow cellular downloads", isOn: $installer.allowCellularDownloads)
                }
                .confirmationDialog(
                    "Delete \(pendingDeleteRegionName)?",
                    isPresented: Binding(
                        get: { pendingDeleteRegionId != nil },
                        set: { if !$0 { pendingDeleteRegionId = nil } }
                    )
                ) {
                    Button("Delete", role: .destructive) {
                        if let id = pendingDeleteRegionId { try? installer.delete(region: id) }
                        pendingDeleteRegionId = nil
                    }
                    Button("Cancel", role: .cancel) { pendingDeleteRegionId = nil }
                }

                Section("Battery") {
                    VStack(alignment: .leading) {
                        Text("Departure SoC: \(formatSocPct(store.departSoc))")
                        Slider(value: $store.departSoc, in: 0.1...1.0)
                    }
                    VStack(alignment: .leading) {
                        Text("Destination Arrival SoC: \(formatSocPct(store.arrivalMinSoc))")
                        Slider(value: $store.arrivalMinSoc, in: 0...0.5)
                    }
                    VStack(alignment: .leading) {
                        Text("Charger Arrival SoC: \(formatSocPct(store.chargerArrivalMinSoc))")
                        Slider(value: $store.chargerArrivalMinSoc, in: 0...0.5)
                    }
                    VStack(alignment: .leading) {
                        Text("Charger Max SoC: \(formatSocPct(store.chargerMaxSoc))")
                        Slider(value: $store.chargerMaxSoc, in: 0.5...1.0)
                    }
                }

                Section("Route") {
                    Picker("Stops Bias", selection: stopsBias) {
                        ForEach(StopsBias.allCases) { bias in
                            Text(bias.rawValue).tag(bias)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                Section("Conditions") {
                    Stepper("Temperature: \(Int(store.tempC))\u{00B0}C", value: $store.tempC, in: -20...40, step: 1)
                    Stepper("Headwind: \(Int(store.headwindMs)) m/s", value: $store.headwindMs, in: 0...20, step: 1)
                }

                Section("Vehicle") {
                    Text("Hyundai Ioniq 5 LR 2WD")
                    Toggle("Override reference consumption", isOn: referenceConsumptionOverrideEnabled)
                    if store.referenceConsumptionWhPerKm != nil {
                        Stepper(
                            "Reference Consumption: \(Int(referenceConsumptionWhPerKm.wrappedValue)) Wh/km",
                            value: referenceConsumptionWhPerKm, in: 120...260, step: 5
                        )
                    }
                }

                Section("Appearance") {
                    Picker("Appearance", selection: $store.appearanceOverride) {
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                    .pickerStyle(.segmented)
                }
            }
            .navigationTitle("Settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    /// The picker binds through the ported `StopsBias` enum; the store keeps the raw Double
    /// request value.
    private var stopsBias: Binding<StopsBias> {
        Binding(
            get: { StopsBias(requestValue: store.stopsBias) ?? .quickest },
            set: { store.stopsBias = $0.requestValue }
        )
    }

    /// EV Database "Highway - Mild" figure for the 2WD LR at 110 km/h, 23°C (research §3 row 3,
    /// reproduced by core/energy/src/gate_tests.rs's gate_3_highway_mild_110kmh) -- the
    /// vehicle's actual reference consumption, used as the stepper's initial value when the
    /// override is switched on.
    private static let defaultReferenceConsumptionWhPerKm = 209.0

    /// `referenceConsumptionWhPerKm` is `Double?` (nil = vehicle default); the Toggle sets it
    /// to the default figure above, or clears it back to nil.
    private var referenceConsumptionOverrideEnabled: Binding<Bool> {
        Binding(
            get: { store.referenceConsumptionWhPerKm != nil },
            set: { store.referenceConsumptionWhPerKm = $0 ? Self.defaultReferenceConsumptionWhPerKm : nil }
        )
    }

    private var referenceConsumptionWhPerKm: Binding<Double> {
        Binding(
            get: { store.referenceConsumptionWhPerKm ?? Self.defaultReferenceConsumptionWhPerKm },
            set: { store.referenceConsumptionWhPerKm = $0 }
        )
    }

    // MARK: Packs section (wayfinder #47)

    private var pendingDeleteRegionName: String {
        guard let id = pendingDeleteRegionId else { return "" }
        return installer.rows.first(where: { $0.id == id })?.name ?? id
    }

    @ViewBuilder
    private func packRow(_ row: PackInstaller.RegionRow) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack {
                VStack(alignment: .leading) {
                    Text(row.name)
                    Text(packRowSubtitle(row)).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                packRowButtons(row)
            }
            if let fraction = row.downloadFraction {
                ProgressView(value: fraction)
            }
        }
    }

    @ViewBuilder
    private func packRowButtons(_ row: PackInstaller.RegionRow) -> some View {
        HStack(spacing: 12) {
            if row.downloadFraction == nil {
                if row.installedEpoch == nil {
                    Button("Install") { Task { try? await installer.install(region: row.id) } }
                } else {
                    if row.updateAvailable {
                        Button("Update") { Task { try? await installer.install(region: row.id) } }
                    }
                    Button("Use") { store.setActiveRegion(row.id) }
                        .disabled(row.id == store.activeRegion)
                    Button("Delete", role: .destructive) { pendingDeleteRegionId = row.id }
                }
            }
        }
        .buttonStyle(.borderless)
    }

    private func packRowSubtitle(_ row: PackInstaller.RegionRow) -> String {
        let sizeSuffix = row.totalBytes.map { " \u{00B7} \(Self.byteCountFormatter.string(fromByteCount: $0))" } ?? ""
        if let fraction = row.downloadFraction {
            return "Downloading \(Int(fraction * 100))%"
        }
        guard let installedEpoch = row.installedEpoch else {
            return "Not installed" + sizeSuffix
        }
        let dateText = Self.epochDateFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(installedEpoch)))
        if row.updateAvailable {
            return "Update available (installed \(dateText))"
        }
        return "Installed \(dateText)" + sizeSuffix
    }

    private static let byteCountFormatter: ByteCountFormatter = {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .file
        return formatter
    }()

    private static let epochDateFormatter: DateFormatter = {
        let formatter = DateFormatter()
        formatter.dateStyle = .medium
        return formatter
    }()
}
