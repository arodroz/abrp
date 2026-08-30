// The settings sheet (wayfinder #44): SoC sliders, Stops Bias, conditions (temperature,
// headwind), a Reference Consumption override, the (fixed) vehicle, and a light/dark
// appearance override. Ported from prototype/planner-ui's SettingsForm.swift, re-typed off
// PlanStore's @Observable (via @Bindable, not the prototype's @ObservedObject) so every field
// binds straight to a PlanStore var and its own didSet-triggered replan -- there's no local
// @State here. Dropped "Max speed" (Speed Caps are a planner OUTPUT, not a request field --
// ADR 0010 point 1) and "Extra weight" (no request field); added the Headwind stepper and the
// Appearance section. The Packs section (wayfinder #47) lists every index region against
// PackInstaller's rows with Install/Update/Use/Delete actions. The Trip Logs section
// (wayfinder #51) lists saved Trip Logs against TripLogStore.logs, each row share-sheet
// exportable (ADR 0009 point 5) with the same Delete-button + confirmationDialog pattern as
// the Packs section's rows. The Calibration section (wayfinder #53), shown once Trip Logs
// exist, renders PlanStore.calibrationResult's fit quality, a per-trip caption on each Trip
// Logs row, and the accept/dismiss proposal row for PlanStore's refit-and-accept flow.
import Foundation
import PlannerKit
import SwiftUI

struct SettingsForm: View {
    @Bindable var store: PlanStore
    @Bindable var installer: PackInstaller
    let tripStore: TripLogStore
    @Environment(\.dismiss) private var dismiss
    @State private var pendingDeleteRegionId: String?
    @State private var pendingDeleteTripLogURL: URL?
    @State private var confirmingDeleteAllLogs = false
    @State private var confirmingClearRecents = false

    var body: some View {
        NavigationStack {
            Form {
                Section("Trip Logs") {
                    ForEach(decodedTripLogs, id: \.url) { entry in
                        tripLogRow(url: entry.url, log: entry.log)
                    }
                }
                .confirmationDialog(
                    "Delete this Trip Log?",
                    isPresented: Binding(
                        get: { pendingDeleteTripLogURL != nil },
                        set: { if !$0 { pendingDeleteTripLogURL = nil } }
                    )
                ) {
                    Button("Delete", role: .destructive) {
                        if let url = pendingDeleteTripLogURL {
                            try? TripLogStorage.delete(url: url)
                            tripStore.refreshLogs()
                            store.refreshCalibration(logURLs: tripStore.logs)
                        }
                        pendingDeleteTripLogURL = nil
                    }
                    Button("Cancel", role: .cancel) { pendingDeleteTripLogURL = nil }
                }

                if !tripStore.logs.isEmpty {
                    Section("Calibration") {
                        calibrationStatusRows
                        if !store.calibrationDismissed, let result = store.calibrationResult,
                           abs(result.referenceConsumptionWhPerKm - currentEffectiveReferenceConsumptionWhPerKm) >= 1.0 {
                            calibrationProposalRow(result)
                        }
                        if let calibrationErrorMessage = store.calibrationErrorMessage {
                            Text(calibrationErrorMessage).font(.caption).foregroundStyle(.red)
                        }
                    }
                }

                Section("Packs") {
                    ForEach(installer.rows) { row in
                        packRow(row)
                    }
                    Toggle("Allow cellular downloads", isOn: $installer.allowCellularDownloads)
                    if installer.lastIndexFetchFailed {
                        Text("Couldn't check for pack updates").font(.caption).foregroundStyle(.secondary)
                    }
                    if let lastOperationError = installer.lastOperationError {
                        Text(lastOperationError).font(.caption).foregroundStyle(.red)
                    }
                }
                .confirmationDialog(
                    "Delete \(pendingDeleteRegionName)?",
                    isPresented: Binding(
                        get: { pendingDeleteRegionId != nil },
                        set: { if !$0 { pendingDeleteRegionId = nil } }
                    )
                ) {
                    Button("Delete", role: .destructive) {
                        if let id = pendingDeleteRegionId {
                            Task {
                                // M-03: failures are no longer swallowed with `try?` --
                                // `installer.delete` already records them into
                                // `lastOperationError`, rendered as the footnote row above.
                                do { try installer.delete(region: id) } catch {}
                            }
                        }
                        pendingDeleteRegionId = nil
                    }
                    Button("Cancel", role: .cancel) { pendingDeleteRegionId = nil }
                }

                Section("Battery") {
                    VStack(alignment: .leading) {
                        Text("Departure SoC: \(formatSocPct(store.departSoc))")
                            .accessibilityIdentifier("departure-soc-label")
                        Slider(value: $store.departSoc, in: 0.1...1.0)
                            .accessibilityIdentifier("departure-soc-slider")
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

                // Data section (issue #56 / SEC-010): local-data deletion controls, ahead of
                // Appearance so it reads as part of the same account/vehicle-level settings
                // rather than tucked below the display options.
                Section("Data") {
                    Button("Delete All Trip Logs", role: .destructive) { confirmingDeleteAllLogs = true }
                        .confirmationDialog(
                            "Delete all Trip Logs?", isPresented: $confirmingDeleteAllLogs
                        ) {
                            Button("Delete", role: .destructive) {
                                tripStore.deleteAllLogs()
                                store.refreshCalibration(logURLs: tripStore.logs)
                            }
                            Button("Cancel", role: .cancel) {}
                        }
                    Button("Clear Recent Destinations", role: .destructive) { confirmingClearRecents = true }
                        .confirmationDialog(
                            "Clear recent destinations?", isPresented: $confirmingClearRecents
                        ) {
                            Button("Clear", role: .destructive) { RecentDestination.clearAll() }
                            Button("Cancel", role: .cancel) {}
                        }
                }

                Section("Appearance") {
                    Picker("Appearance", selection: $store.appearanceOverride) {
                        Text("System").tag("system")
                        Text("Light").tag("light")
                        Text("Dark").tag("dark")
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("appearance-picker")
                }
            }
            .navigationTitle("Settings")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear {
                tripStore.refreshLogs()
                store.refreshCalibration(logURLs: tripStore.logs)
            }
        }
    }

    // MARK: Trip Logs section (wayfinder #51)

    /// Decodes every saved Trip Log up front -- logs are few, so this is fine done here rather
    /// than lazily per row.
    private var decodedTripLogs: [(url: URL, log: TripLog)] {
        tripStore.logs.compactMap { url in
            guard let data = try? Data(contentsOf: url), let log = try? JSONDecoder().decode(TripLog.self, from: data) else {
                return nil
            }
            return (url, log)
        }
    }

    @ViewBuilder
    private func tripLogRow(url: URL, log: TripLog) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 4) {
                Text(Self.epochDateFormatter.string(from: Date(timeIntervalSince1970: TimeInterval(log.startUnix))))
                Text("\(tripLogDurationText(log)) \u{00B7} SoC \(log.startSocPct)% \u{2192} \(log.endSocPct)% \u{00B7} \(log.samples.count) samples")
                    .font(.caption).foregroundStyle(.secondary)
                if let calibrationCaption = calibrationCaption(for: log) {
                    Text(calibrationCaption).font(.caption).foregroundStyle(.secondary)
                }
            }
            Spacer()
            ShareLink(item: url) {
                Image(systemName: "square.and.arrow.up")
            }
            Button("Delete", role: .destructive) { pendingDeleteTripLogURL = url }
                .accessibilityIdentifier("trip-log-delete-button")
        }
        .buttonStyle(.borderless)
    }

    private func tripLogDurationText(_ log: TripLog) -> String {
        let minutes = (log.endUnix - log.startUnix) / 60
        guard minutes >= 60 else { return "\(minutes) min" }
        return "\(minutes / 60)h \(minutes % 60)m"
    }

    // MARK: Calibration section (wayfinder #53)

    /// Second caption line for a Trip Log row once a calibration result exists: why an
    /// excluded trip didn't count, or the fit quality for one that did.
    private func calibrationCaption(for log: TripLog) -> String? {
        guard let fit = store.calibrationResult?.trips.first(where: { $0.id == log.id }) else { return nil }
        if let excludedReason = fit.excludedReason {
            return "Not used: \(excludedReason)"
        }
        if fit.used, fit.qualifying, let errorPoints = fit.errorPoints {
            return String(format: "ratio %.2f \u{00B7} error %.1f pts", fit.ratio, errorPoints)
        }
        if fit.used, fit.qualifying {
            return String(format: "ratio %.2f", fit.ratio)
        }
        if fit.used {
            return String(format: "ratio %.2f \u{00B7} under 100 km", fit.ratio)
        }
        return nil
    }

    @ViewBuilder
    private var calibrationStatusRows: some View {
        if let result = store.calibrationResult {
            let usableCount = result.trips.filter(\.used).count
            // The Rust fit only ever weighs the last-10 qualifying trips (ADR 0009 point 4);
            // this counts every qualifying trip in the set, not just the ones inside that window.
            let qualifyingCount = result.trips.filter(\.qualifying).count
            VStack(alignment: .leading, spacing: 4) {
                Text("\(usableCount) of \(result.trips.count) trips usable")
                if let maxErrorPoints = result.maxErrorPoints, let maePoints = result.maePoints {
                    Text(String(
                        format: "max error %.1f pts \u{00B7} avg %.1f pts over %d qualifying trips",
                        maxErrorPoints, maePoints, qualifyingCount
                    ))
                    .font(.caption).foregroundStyle(.secondary)
                }
                if result.accepted {
                    Label("Calibrated", systemImage: "checkmark.circle").font(.caption).foregroundStyle(.secondary)
                } else {
                    Text("Not yet calibrated").font(.caption).foregroundStyle(.secondary)
                }
            }
        } else if store.calibrationErrorMessage == nil {
            Text(store.plannerStatus == .ready ? "Checking calibration\u{2026}" : "Planner not ready")
                .font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func calibrationProposalRow(_ result: FfiCalibrationResult) -> some View {
        HStack {
            Text(
                "Reference Consumption: \(Int(currentEffectiveReferenceConsumptionWhPerKm)) "
                    + "\u{2192} \(Int(result.referenceConsumptionWhPerKm)) Wh/km"
            )
            Spacer()
            Button("Apply") { store.acceptCalibration() }
            Button("Dismiss") { store.dismissCalibration() }
        }
        .buttonStyle(.borderless)
    }

    /// `store.referenceConsumptionWhPerKm ?? defaultReferenceConsumptionWhPerKm` -- the value
    /// the proposal row's "current" side and its >= 1.0 Wh/km threshold compare against.
    private var currentEffectiveReferenceConsumptionWhPerKm: Double {
        store.referenceConsumptionWhPerKm ?? Self.defaultReferenceConsumptionWhPerKm
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
                    Button("Install") { runInstall(row) }
                } else {
                    // M-01: a needs-repair region reuses the update-available path -- Install
                    // redownloads whatever's missing (the installer's per-artifact sha check
                    // already skips what's still there and valid).
                    if row.updateAvailable || row.needsRepair {
                        Button(row.needsRepair ? "Repair" : "Update") { runInstall(row) }
                    }
                    Button("Use") { store.setActiveRegion(row.id) }
                        .disabled(row.id == store.activeRegion)
                    Button("Delete", role: .destructive) { pendingDeleteRegionId = row.id }
                }
            }
        }
        .buttonStyle(.borderless)
    }

    /// M-03: failures are no longer swallowed with `try?` -- `installer.install` already
    /// records them into `lastOperationError`, rendered as the footnote row above.
    private func runInstall(_ row: PackInstaller.RegionRow) {
        Task {
            do { try await installer.install(region: row.id) } catch {}
        }
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
        if row.needsRepair {
            return "Needs repair (installed \(dateText))"
        }
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
