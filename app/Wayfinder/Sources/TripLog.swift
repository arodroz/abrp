// Trip Log schema + storage (wayfinder #51), per ADR 0009: a Trip Log captures a GPS trace,
// timestamps, ambient temperature, and manual start/end dash SoC. On-disk format `tlog-1`,
// snake_case keys via CodingKeys matching cpack-1's style (CPack1.swift). This schema is the
// contract #52's Rust `calibrate()` will parse -- keep it exactly as specified there.
//
// Telemetry auto-capture (wayfinder #80, ADR 0014): an optional `telemetry` block, added without
// bumping the format id -- unknown/missing fields parse fine on both sides. See `TripTelemetry`.
import Foundation

struct TripSample: Codable {
    /// Seconds since trip start.
    let t: Double
    let lat: Double
    let lon: Double
    /// nil when CoreLocation reports an invalid (negative) speed.
    let speedMps: Double?
    /// nil when CLLocation.verticalAccuracy <= 0 (altitude invalid).
    let altM: Double?
    /// nil when CLLocation.horizontalAccuracy < 0 (fix invalid).
    let haccM: Double?

    enum CodingKeys: String, CodingKey {
        case t, lat, lon
        case speedMps = "speed_mps"
        case altM = "alt_m"
        case haccM = "hacc_m"
    }
}

/// Optional telemetry auto-capture block (wayfinder #80, ADR 0014 -- a delta to ADR 0009): raw
/// start/end snapshots, not a precomputed delta -- the counters' absolute unit truth is still
/// pending the driveway check (#81), and snapshots let a stored log be re-audited if that scaling
/// turns out wrong. Every field is individually optional (BMS SoC stays null until #81 maps a
/// source). Custom `encode(to:)` writes all eight keys explicitly, `null` when unknown, rather
/// than the synthesized `encodeIfPresent` that would omit a nil key -- the schema's slots exist
/// even for a value that never lands.
struct TripTelemetry: Codable, Equatable {
    let startDisplaySocPct: Double?
    let endDisplaySocPct: Double?
    let startBmsSocPct: Double?
    let endBmsSocPct: Double?
    let startCumulativeChargeKwh: Double?
    let endCumulativeChargeKwh: Double?
    let startCumulativeDischargeKwh: Double?
    let endCumulativeDischargeKwh: Double?

    enum CodingKeys: String, CodingKey {
        case startDisplaySocPct = "start_display_soc_pct"
        case endDisplaySocPct = "end_display_soc_pct"
        case startBmsSocPct = "start_bms_soc_pct"
        case endBmsSocPct = "end_bms_soc_pct"
        case startCumulativeChargeKwh = "start_cumulative_charge_kwh"
        case endCumulativeChargeKwh = "end_cumulative_charge_kwh"
        case startCumulativeDischargeKwh = "start_cumulative_discharge_kwh"
        case endCumulativeDischargeKwh = "end_cumulative_discharge_kwh"
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(startDisplaySocPct, forKey: .startDisplaySocPct)
        try container.encode(endDisplaySocPct, forKey: .endDisplaySocPct)
        try container.encode(startBmsSocPct, forKey: .startBmsSocPct)
        try container.encode(endBmsSocPct, forKey: .endBmsSocPct)
        try container.encode(startCumulativeChargeKwh, forKey: .startCumulativeChargeKwh)
        try container.encode(endCumulativeChargeKwh, forKey: .endCumulativeChargeKwh)
        try container.encode(startCumulativeDischargeKwh, forKey: .startCumulativeDischargeKwh)
        try container.encode(endCumulativeDischargeKwh, forKey: .endCumulativeDischargeKwh)
    }
}

struct TripLog: Codable {
    let format: String
    let id: String
    let vehicle: String
    let startUnix: Int
    let endUnix: Int
    let startSocPct: Int
    let endSocPct: Int
    let ambientTempC: Double?
    let samples: [TripSample]
    /// Present only when at least one telemetry field landed during the trip (wayfinder #80);
    /// omitted entirely (not merely null) otherwise -- see `TripLogStore.confirmEndSoc`.
    let telemetry: TripTelemetry?

    enum CodingKeys: String, CodingKey {
        case format, id, vehicle
        case startUnix = "start_unix"
        case endUnix = "end_unix"
        case startSocPct = "start_soc_pct"
        case endSocPct = "end_soc_pct"
        case ambientTempC = "ambient_temp_c"
        case samples
        case telemetry
    }

    init(
        format: String, id: String, vehicle: String, startUnix: Int, endUnix: Int,
        startSocPct: Int, endSocPct: Int, ambientTempC: Double?, samples: [TripSample],
        telemetry: TripTelemetry? = nil
    ) {
        self.format = format
        self.id = id
        self.vehicle = vehicle
        self.startUnix = startUnix
        self.endUnix = endUnix
        self.startSocPct = startSocPct
        self.endSocPct = endSocPct
        self.ambientTempC = ambientTempC
        self.samples = samples
        self.telemetry = telemetry
    }
}

/// Trip Log storage in Documents/trip-logs/ (ADR 0009 point 5: local JSON, share-sheet
/// exportable, never uploaded). Static-function enum, matching Packs.swift's style.
enum TripLogStorage {
    private static var directory: URL {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        return docs.appendingPathComponent("trip-logs", isDirectory: true)
    }

    /// Filename `tlog-<start_unix>-<first 8 of id>.json`; sortedKeys makes the output
    /// deterministic (matters for the byte-level snake_case check in triplog-smoke).
    @discardableResult
    static func save(_ log: TripLog) throws -> URL {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("tlog-\(log.startUnix)-\(log.id.prefix(8)).json")
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        try encoder.encode(log).write(to: url, options: .atomic)
        return url
    }

    /// Newest first -- the filename's leading start_unix sorts lexically for a fixed-width
    /// epoch, which unix timestamps are for the foreseeable future.
    static func list() -> [URL] {
        let urls = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return urls.filter { $0.pathExtension == "json" }.sorted { $0.lastPathComponent > $1.lastPathComponent }
    }

    static func delete(url: URL) throws {
        try FileManager.default.removeItem(at: url)
    }
}
