// Trip Log schema + storage (wayfinder #51), per ADR 0009: a Trip Log captures a GPS trace,
// timestamps, ambient temperature, and manual start/end dash SoC. On-disk format `tlog-1`,
// snake_case keys via CodingKeys matching cpack-1's style (CPack1.swift). This schema is the
// contract #52's Rust `calibrate()` will parse -- keep it exactly as specified there.
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

    enum CodingKeys: String, CodingKey {
        case format, id, vehicle
        case startUnix = "start_unix"
        case endUnix = "end_unix"
        case startSocPct = "start_soc_pct"
        case endSocPct = "end_soc_pct"
        case ambientTempC = "ambient_temp_c"
        case samples
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
