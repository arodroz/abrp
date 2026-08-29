// The pack installer (ADR 0011, wayfinder #47): per-artifact sha-driven install/refresh over a
// background URLSession, all-or-nothing per region. `installed-<region>.json` in Documents is
// the source of truth for what's installed -- `Packs.locate` (Packs.swift) stays the
// file-existence check load() uses, unrelated to this bookkeeping. One artifact downloads at a
// time; downloads are staged under `Documents/.staging-<region>/` and only moved into place
// (replacing) once every needed artifact is verified, so a failed/cancelled install always
// leaves the previous install fully usable.
import CryptoKit
import Foundation

/// `{region_id, epoch, artifacts: {kind: {file, sha256}}}`, keyed by "region_pack" /
/// "charger_pack" / "map_pack" -- the same kind strings as `PackArtifacts.byKind` and the
/// publish script.
struct InstalledArtifactRecord: Codable, Equatable {
    let file: String
    let sha256: String
}

struct InstalledRecord: Codable, Equatable {
    let regionId: String
    let epoch: Int
    let artifacts: [String: InstalledArtifactRecord]

    enum CodingKeys: String, CodingKey {
        case regionId = "region_id"
        case epoch, artifacts
    }
}

enum PackInstallerError: Error, LocalizedError {
    case checksumMismatch(kind: String, expected: String, got: String)
    case alreadyInstalling
    case badResponse

    var errorDescription: String? {
        switch self {
        case .checksumMismatch(let kind, let expected, let got):
            return "Checksum mismatch for \(kind): expected \(expected), got \(got)"
        case .alreadyInstalling:
            return "An install is already in progress"
        case .badResponse:
            return "Unexpected server response"
        }
    }
}

@MainActor
@Observable
final class PackInstaller: NSObject {
    private static let sessionIdentifier = "org.anteras.wayfinder.packs"
    private static let cellularDefaultsKey = "allowCellularDownloads"

    /// One row per index region, for the settings sheet's Packs section. Seeded at init from
    /// whatever `installed-*.json` records already exist on disk, so an offline cold start
    /// still shows installed regions (the index fetch failing must not empty this list).
    struct RegionRow: Identifiable, Equatable {
        let id: String
        var name: String
        var totalBytes: Int64?
        var installedEpoch: Int?
        var downloadFraction: Double?
        var updateAvailable = false
    }

    private(set) var rows: [RegionRow]
    private(set) var lastIndexFetchFailed = false

    var allowCellularDownloads: Bool {
        didSet {
            guard oldValue != allowCellularDownloads else { return }
            UserDefaults.standard.set(allowCellularDownloads, forKey: Self.cellularDefaultsKey)
            // The background session's config is fixed at creation -- drop it so the next
            // download picks up the new setting.
            backingSession?.invalidateAndCancel()
            backingSession = nil
        }
    }

    private var backingSession: URLSession?
    private var currentInstallRegion: String?
    private var pendingContinuation: CheckedContinuation<Void, Error>?
    private var pendingDestination: URL?

    override init() {
        allowCellularDownloads = UserDefaults.standard.bool(forKey: Self.cellularDefaultsKey)
        rows = Self.scanInstalledRows()
        super.init()
    }

    // MARK: Index / refresh check

    /// Fetches the index (updating `rows`' names/sizes) and, for every region with an
    /// installed record, its catalog, marking `updateAvailable` on a sha256 mismatch -- this
    /// is what catches a charger-only refresh, which doesn't bump the index's `latest_epoch`.
    /// Fire-and-forget from app launch: any failure just leaves `rows` as they were.
    func checkForUpdates() async {
        do {
            let index = try await PackCatalogClient.fetchIndex()
            lastIndexFetchFailed = false
            applyIndex(index)
        } catch {
            lastIndexFetchFailed = true
            return
        }

        for row in rows {
            guard let installed = Self.loadRecord(region: row.id) else { continue }
            guard let catalog = try? await PackCatalogClient.fetchCatalog(region: row.id) else { continue }
            setUpdateAvailable(region: row.id, Self.recordDiffers(installed: installed, catalog: catalog))
        }
    }

    private func applyIndex(_ index: PackIndex) {
        var updated: [RegionRow] = []
        for summary in index.regions {
            var row = rows.first(where: { $0.id == summary.id }) ?? RegionRow(id: summary.id, name: summary.name)
            row.name = summary.name
            row.totalBytes = summary.totalBytes
            if let record = Self.loadRecord(region: summary.id) {
                row.installedEpoch = record.epoch
            }
            updated.append(row)
        }
        for existing in rows where !updated.contains(where: { $0.id == existing.id }) {
            updated.append(existing)
        }
        rows = updated.sorted { $0.id < $1.id }
    }

    private static func recordDiffers(installed: InstalledRecord, catalog: PackCatalog) -> Bool {
        for (kind, artifact) in catalog.artifacts.byKind {
            guard let existing = installed.artifacts[kind], existing.sha256 == artifact.sha256 else { return true }
        }
        return false
    }

    private func setUpdateAvailable(region: String, _ value: Bool) {
        guard let idx = rows.firstIndex(where: { $0.id == region }) else { return }
        rows[idx].updateAvailable = value
    }

    // MARK: Install / refresh

    /// Downloads only the artifacts whose catalog sha256 differs from the installed record (a
    /// fresh install has no record, so all three download) -- one rule covering first install,
    /// epoch refresh, and a future charger-only refresh. Stages under
    /// `Documents/.staging-<region>/`, verifies each download's sha256, and only once every
    /// needed artifact is staged and verified does it commit: move into Documents under the
    /// catalog's own file names, refresh the two style files from the epoch dir, then write
    /// `installed-<region>.json`.
    func install(region: String) async throws {
        guard currentInstallRegion == nil else { throw PackInstallerError.alreadyInstalling }
        currentInstallRegion = region
        defer {
            currentInstallRegion = nil
            setProgress(region: region, nil)
        }

        let catalog = try await PackCatalogClient.fetchCatalog(region: region)
        let existing = Self.loadRecord(region: region)

        let docs = Self.documentsURL()
        let stagingDir = docs.appendingPathComponent(".staging-\(region)", isDirectory: true)
        try? FileManager.default.removeItem(at: stagingDir)
        try FileManager.default.createDirectory(at: stagingDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: stagingDir) }

        var newArtifacts: [String: InstalledArtifactRecord] = [:]
        var toCommit: [(fileName: String, stagedURL: URL)] = []

        setProgress(region: region, 0)
        for (kind, artifact) in catalog.artifacts.byKind {
            try Task.checkCancellation()
            if let existingArtifact = existing?.artifacts[kind], existingArtifact.sha256 == artifact.sha256 {
                newArtifacts[kind] = existingArtifact
                continue
            }

            let sourceURL = PackCatalogClient.baseURL.appendingPathComponent(artifact.path)
            let stagedURL = stagingDir.appendingPathComponent(artifact.file)
            try await downloadArtifact(from: sourceURL, to: stagedURL, region: region)

            // Off the main actor: hashing GBs takes seconds and must neither freeze the UI
            // nor pile autoreleased chunks onto a runloop that never turns.
            let digest = try await Task.detached(priority: .userInitiated) {
                try Self.sha256Hex(of: stagedURL)
            }.value
            guard digest == artifact.sha256 else {
                throw PackInstallerError.checksumMismatch(kind: kind, expected: artifact.sha256, got: digest)
            }
            newArtifacts[kind] = InstalledArtifactRecord(file: artifact.file, sha256: artifact.sha256)
            toCommit.append((artifact.file, stagedURL))
        }

        // All-or-nothing commit: every needed artifact is staged and verified at this point.
        for item in toCommit {
            let dest = docs.appendingPathComponent(item.fileName)
            if FileManager.default.fileExists(atPath: dest.path) {
                try FileManager.default.removeItem(at: dest)
            }
            try FileManager.default.moveItem(at: item.stagedURL, to: dest)
        }

        try await refreshStyleFiles(region: region, epoch: catalog.osmSnapshotEpoch)

        let record = InstalledRecord(regionId: region, epoch: catalog.osmSnapshotEpoch, artifacts: newArtifacts)
        try Self.saveRecord(record)

        if let idx = rows.firstIndex(where: { $0.id == region }) {
            rows[idx].installedEpoch = record.epoch
            rows[idx].updateAvailable = false
        } else {
            rows.append(RegionRow(id: region, name: catalog.regionName, installedEpoch: record.epoch))
            rows.sort { $0.id < $1.id }
        }
    }

    /// Tiny, untracked by sha -- always refetched from the epoch dir on every commit, shared
    /// (non-region-specific) across whichever region installs last.
    private func refreshStyleFiles(region: String, epoch: Int) async throws {
        let docs = Self.documentsURL()
        for name in ["style-light.json", "style-dark.json"] {
            let url = PackCatalogClient.baseURL.appendingPathComponent("packs/\(region)/\(epoch)/\(name)")
            let (data, response) = try await URLSession.shared.data(from: url)
            guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
                throw PackInstallerError.badResponse
            }
            try data.write(to: docs.appendingPathComponent(name), options: .atomic)
        }
    }

    /// Removes the region's artifact files and its installed record; leaves the shared style
    /// files (another region may use them).
    func delete(region: String) throws {
        let docs = Self.documentsURL()
        if let record = Self.loadRecord(region: region) {
            for artifact in record.artifacts.values {
                try? FileManager.default.removeItem(at: docs.appendingPathComponent(artifact.file))
            }
        }
        try? FileManager.default.removeItem(at: Self.recordURL(region: region))

        if let idx = rows.firstIndex(where: { $0.id == region }) {
            rows[idx].installedEpoch = nil
            rows[idx].updateAvailable = false
            rows[idx].downloadFraction = nil
        }
    }

    // MARK: Background download (one artifact at a time)

    private func session() -> URLSession {
        if let backingSession { return backingSession }
        let config = URLSessionConfiguration.background(withIdentifier: Self.sessionIdentifier)
        config.allowsCellularAccess = allowCellularDownloads
        let session = URLSession(configuration: config, delegate: self, delegateQueue: .main)
        backingSession = session
        return session
    }

    private func downloadArtifact(from url: URL, to destination: URL, region: String) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            self.pendingContinuation = continuation
            self.pendingDestination = destination
            let task = self.session().downloadTask(with: url)
            task.resume()
        }
    }

    private func setProgress(region: String, _ fraction: Double?) {
        guard let idx = rows.firstIndex(where: { $0.id == region }) else { return }
        rows[idx].downloadFraction = fraction
    }

    // MARK: Persistence

    private static func documentsURL() -> URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    private static func recordURL(region: String) -> URL {
        documentsURL().appendingPathComponent("installed-\(region).json")
    }

    static func loadRecord(region: String) -> InstalledRecord? {
        guard let data = try? Data(contentsOf: recordURL(region: region)) else { return nil }
        return try? JSONDecoder().decode(InstalledRecord.self, from: data)
    }

    private static func saveRecord(_ record: InstalledRecord) throws {
        let data = try JSONEncoder().encode(record)
        try data.write(to: recordURL(region: record.regionId), options: .atomic)
    }

    /// Offline-safe seed for `rows`: scans Documents for `installed-*.json` files directly,
    /// independent of any index fetch.
    private static func scanInstalledRows() -> [RegionRow] {
        let docs = documentsURL()
        guard let files = try? FileManager.default.contentsOfDirectory(atPath: docs.path) else { return [] }
        var rows: [RegionRow] = []
        for file in files where file.hasPrefix("installed-") && file.hasSuffix(".json") {
            let region = String(file.dropFirst("installed-".count).dropLast(".json".count))
            guard let record = loadRecord(region: region) else { continue }
            rows.append(RegionRow(id: region, name: region, installedEpoch: record.epoch))
        }
        return rows.sorted { $0.id < $1.id }
    }

    /// Streamed (CryptoKit), never loading the whole file into memory -- the Map Pack can be
    /// GBs. Each read is drained in its own autorelease pool: `FileHandle.read` autoreleases
    /// its chunk, and a GB-scale loop on a blocked runloop otherwise accumulates them all
    /// (jetsam-killed the M3 gate's first eu-west install at the 4.37 GB rpack).
    nonisolated private static func sha256Hex(of url: URL) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        var hasher = SHA256()
        while try autoreleasepool(invoking: {
            guard let chunk = try handle.read(upToCount: 4 * 1024 * 1024), !chunk.isEmpty else {
                return false
            }
            hasher.update(data: chunk)
            return true
        }) {}
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

extension PackInstaller: URLSessionDownloadDelegate {
    /// `location` is a temp file deleted as soon as this method returns, so the move to
    /// `pendingDestination` must happen synchronously here, on the main queue (the session was
    /// created with `delegateQueue: .main`), matching every other MLNMapViewDelegate/
    /// CLLocationManagerDelegate callback in this app that mutates @MainActor state directly.
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        guard let destination = pendingDestination else { return }
        do {
            if FileManager.default.fileExists(atPath: destination.path) {
                try FileManager.default.removeItem(at: destination)
            }
            try FileManager.default.moveItem(at: location, to: destination)
            pendingContinuation?.resume()
        } catch {
            pendingContinuation?.resume(throwing: error)
        }
        pendingContinuation = nil
        pendingDestination = nil
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        pendingContinuation?.resume(throwing: error)
        pendingContinuation = nil
        pendingDestination = nil
    }

    func urlSession(
        _ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
    ) {
        guard totalBytesExpectedToWrite > 0, let region = currentInstallRegion else { return }
        setProgress(region: region, Double(totalBytesWritten) / Double(totalBytesExpectedToWrite))
    }
}
