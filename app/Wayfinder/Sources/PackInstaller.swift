// The pack installer (ADR 0011, wayfinder #47), hardened per the codebase audit
// (docs/codebase-audit-2026-08-29.md H-01/H-02/H-03/M-01/M-02/M-03): per-artifact sha-driven
// install/refresh over a background URLSession, all-or-nothing per region.
// `installed-<region>.json` in Documents is the source of truth for what's installed --
// `Packs.locate` (Packs.swift) stays the file-existence check load() uses, unrelated to this
// bookkeeping. One artifact downloads at a time; downloads are staged under
// `Documents/.staging/<region>-<epoch>/`.
//
// Commit is journaled roll-forward (H-01): every artifact, plus the two style files, is staged
// and sha-verified BEFORE any destination is touched. A `Documents/.staging/<region>.commit.json`
// journal then records the planned {staged file, destination file, sha256} moves and the full
// new installed record; only after that journal is safely on disk does each destination get
// swapped in with `FileManager.replaceItemAt` (never remove-then-move, which has a data-loss
// window with no destination file at all). A crash mid-commit leaves the journal behind;
// `reconcileJournal` rolls it forward -- or abandons it, leaving the previous install intact --
// on the next launch, before anything reads `rows`.
//
// Background downloads are recorded in `Documents/.staging/downloads.json` (task identifier ->
// region/artifact/expected sha/staged destination) before each task is resumed (H-03): a
// relaunched process (iOS restarts the app to deliver background session events even after the
// old process was killed) can still resolve a completed transfer's destination from the
// manifest and land the file into staging, and an orphaned manifest entry (its background task
// is simply gone) gets a fresh `install(region:)` kicked for its region -- the per-artifact
// staged-and-verified skip below makes that re-entry idempotent instead of redundant.
import CryptoKit
import Foundation
import os
import PlannerKit
import UIKit

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

/// One planned commit move: a staged file, the Documents-relative destination it replaces, and
/// the sha256 both are expected to hash to once committed (H-01).
struct CommitJournalEntry: Codable {
    let stagedFile: String
    let destinationFile: String
    let sha256: String

    enum CodingKeys: String, CodingKey {
        case stagedFile = "staged_file"
        case destinationFile = "destination_file"
        case sha256
    }
}

/// Written to `Documents/.staging/<region>.commit.json` once every entry is staged and
/// verified, before any destination is touched; `reconcileJournal` is the only reader besides
/// the commit step that just wrote it.
struct CommitJournal: Codable {
    let regionId: String
    let epoch: Int
    let entries: [CommitJournalEntry]
    let record: InstalledRecord

    enum CodingKeys: String, CodingKey {
        case regionId = "region_id"
        case epoch, entries, record
    }
}

/// One in-flight background download, keyed by `URLSessionTask.taskIdentifier` (as a string,
/// for JSON) in `Documents/.staging/downloads.json` (H-03).
struct DownloadManifestEntry: Codable {
    let region: String
    let epoch: Int
    let artifactFile: String
    let expectedSha256: String
    let stagedDestinationPath: String

    enum CodingKeys: String, CodingKey {
        case region, epoch
        case artifactFile = "artifact_file"
        case expectedSha256 = "expected_sha256"
        case stagedDestinationPath = "staged_destination_path"
    }
}

enum PackInstallerError: Error, LocalizedError {
    case checksumMismatch(kind: String, expected: String, got: String)
    case alreadyInstalling
    case badResponse
    case partialDelete(files: [String])
    /// R16: a preflight space check before any download starts -- summed from the catalog's
    /// declared artifact sizes (no file hashing) for whatever will actually be (re)downloaded,
    /// plus 500 MB headroom, against the Documents volume's available capacity. Carries
    /// `regionName` (not just the region id used elsewhere in this file) because that's what
    /// belongs in a user-facing message.
    case insufficientSpace(needed: Int64, available: Int64, regionName: String)

    var errorDescription: String? {
        switch self {
        case .checksumMismatch(let kind, let expected, let got):
            return "Checksum mismatch for \(kind): expected \(expected), got \(got)"
        case .alreadyInstalling:
            return "An install is already in progress"
        case .badResponse:
            return "Unexpected server response"
        case .partialDelete(let files):
            return "Failed to remove: \(files.joined(separator: ", "))"
        case .insufficientSpace(let needed, let available, let regionName):
            let formatter = ByteCountFormatter()
            formatter.countStyle = .file
            return "Wayfinder needs about \(formatter.string(fromByteCount: needed)) free to update \(regionName), "
                + "but only \(formatter.string(fromByteCount: available)) is available. Free up space in "
                + "Settings \u{203A} General \u{203A} iPhone Storage, then try again."
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
        /// The UI-facing install/update lifecycle (wayfinder #55): replaces the old bare
        /// `downloadFraction: Double?`, which only ever showed a raw download percentage and
        /// left every other step -- the pre-download re-hash, the post-download verify, the
        /// deep .rpack verify, the commit -- looking like nothing was happening at all. That
        /// silence is what made tonight's real update (eu-west, ~4.6 GB) look hung.
        enum Phase: Equatable {
            case checking
            case downloading(artifact: String)
            case verifying(artifact: String)
            case finishing
        }

        let id: String
        var name: String
        var totalBytes: Int64?
        var installedEpoch: Int?
        var updateAvailable = false
        /// M-01: the installed record exists but one or more of its files are missing on disk
        /// right now -- a deleted/truncated/externally-replaced artifact that reinstalling an
        /// unchanged catalog would otherwise never heal. Set by `refreshRows()`.
        var needsRepair = false
        /// nil when no install/update is in flight for this region. Set synchronously to
        /// `.checking` at the top of `install()`, before the first await, so a tap gets visible
        /// feedback in the same runloop turn it happened in.
        var phase: Phase?
        /// Determinate progress (0...1) within `phase`, when known; nil renders as
        /// indeterminate. Only ever set alongside a non-nil `phase`.
        var phaseFraction: Double?
        /// The byte total `phaseFraction` is a fraction OF (the artifact being downloaded, via
        /// `didWriteData`) -- NOT the region's `totalBytes`: "47% of 4.6 GB" must mean 47% of
        /// the file actually transferring, or the label lies.
        var phaseTotalBytes: Int64?
        /// The full message of the most recent install/update failure for this region, or nil.
        /// Deliberately in-memory only (R13) -- not persisted across relaunch. `install()` sets
        /// it on failure; the next `install()` call (a retry) clears it.
        var lastError: String?
    }

    private(set) var rows: [RegionRow]
    private(set) var lastIndexFetchFailed = false
    /// M-03: the region and message of the most recent install/update/delete/index-fetch
    /// failure, cleared on that operation's next success. Rendered as a footnote-style error
    /// row in SettingsForm's Packs section.
    private(set) var lastOperationError: String?

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

    /// Wired by WayfinderApp (wayfinder #55): `onDeepVerifyWillStart` lets PlanStore release
    /// its mmapped pack right before the deep verify mmaps the staged multi-GB replacement --
    /// at that point only, so planning stays available for the whole (possibly hours-long)
    /// download that precedes it. `onInstallDidEnd` re-adopts afterward, fired only when the
    /// attempt unloaded the Planner or actually (re)staged an artifact (`readoptAfterInstall`).
    var onDeepVerifyWillStart: ((String) -> Void)?
    var onInstallDidEnd: ((String) -> Void)?
    private var readoptAfterInstall = false

    override init() {
        allowCellularDownloads = UserDefaults.standard.bool(forKey: Self.cellularDefaultsKey)
        let docs = Self.documentsURL()
        Self.reconcileJournals(documentsURL: docs)
        rows = Self.scanInstalledRows()
        super.init()
        refreshRows()
        reattachBackgroundSession()
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
            lastOperationError = nil
        } catch {
            lastIndexFetchFailed = true
            lastOperationError = "Packs index: \(Self.userMessage(for: error))"
            return
        }

        refreshRows()

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

    /// Re-derives `rows`' installed-state fields from what's actually on disk right now
    /// (M-01): a record can outlive its files -- deleted, truncated, replaced externally --
    /// without any index fetch or install ever running, so this is a plain on-demand disk
    /// check, not folded only into `applyIndex`/`scanInstalledRows`. Also picks up any
    /// `installed-*.json` not yet represented in `rows` (e.g. before the first index fetch
    /// lands), the same scan `scanInstalledRows()` does at init.
    func refreshRows() {
        let docs = Self.documentsURL()
        for scanned in Self.scanInstalledRows() {
            guard let record = Self.loadRecord(region: scanned.id) else { continue }
            let needsRepair = !Self.filesPresent(record: record, docs: docs)
            if !needsRepair {
                // SEC-010: retroactively excludes an already-installed region's files from
                // backup -- covers regions installed before this flag existed. Idempotent and
                // cheap (a resource-value set, not a content read), so redoing it on every
                // refresh is fine.
                for artifact in record.artifacts.values {
                    Self.excludeFromBackup(docs.appendingPathComponent(artifact.file))
                }
            }
            if let idx = rows.firstIndex(where: { $0.id == scanned.id }) {
                rows[idx].installedEpoch = record.epoch
                rows[idx].needsRepair = needsRepair
            } else {
                var row = scanned
                row.needsRepair = needsRepair
                rows.append(row)
            }
        }
        for name in ["style-light.json", "style-dark.json"] {
            Self.excludeFromBackup(docs.appendingPathComponent(name))
        }
        rows.sort { $0.id < $1.id }
    }

    private static func filesPresent(record: InstalledRecord, docs: URL) -> Bool {
        record.artifacts.values.allSatisfy { FileManager.default.fileExists(atPath: docs.appendingPathComponent($0.file).path) }
    }

    /// Best-effort (issue #56 / SEC-010): a missing file or a `setResourceValues` failure must
    /// not fail the install, or the startup refresh that's re-applying this retroactively.
    private static func excludeFromBackup(_ url: URL) {
        guard FileManager.default.fileExists(atPath: url.path) else { return }
        var url = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? url.setResourceValues(values)
    }

    // MARK: Install / refresh

    func install(region: String) async throws {
        guard currentInstallRegion == nil else { throw PackInstallerError.alreadyInstalling }
        currentInstallRegion = region
        readoptAfterInstall = false
        if let idx = rows.firstIndex(where: { $0.id == region }) {
            rows[idx].lastError = nil
        }
        // R2/R4: set synchronously, before the first await -- the same runloop turn as the
        // tap -- so the UI shows feedback immediately instead of appearing to do nothing while
        // `performInstall` re-hashes multi-GB files first (tonight's #55 failure).
        setPhase(region: region, .checking, fraction: nil)
        defer {
            currentInstallRegion = nil
            setPhase(region: region, nil, fraction: nil)
            // Re-adopt only when this attempt could have changed what the Planner should hold:
            // the deep verify unloaded it, or an artifact was actually (re)staged. A download
            // that failed before either must NOT reset the user's plan on its way out.
            if readoptAfterInstall {
                onInstallDidEnd?(region)
            }
        }

        do {
            try await performInstall(region: region)
        } catch {
            let message = Self.userMessage(for: error)
            if let idx = rows.firstIndex(where: { $0.id == region }) {
                rows[idx].lastError = message
            }
            // R11/R12: the UI now shows only `row.lastError`, which SettingsForm's alert
            // renders in full -- but device consoles should still capture the complete message
            // independent of whatever the UI decides to show (tonight's #55 failure landed
            // truncated in a section footnote with no way to see what deep-verify actually said).
            Logger(subsystem: "org.anteras.wayfinder", category: "packs")
                .error("install failed for \(region, privacy: .public): \(message, privacy: .public)")
            throw error
        }
    }

    /// Downloads only the artifacts whose catalog sha256 differs from the installed record's
    /// (a fresh install has no record, so all three download) -- same rule as before, except a
    /// record match is no longer trusted on its own (M-01): the destination file itself is
    /// re-hashed, so a deleted/truncated/replaced artifact forces a redownload even though its
    /// record looked fine. Stages everything -- artifacts AND the two style files -- under
    /// `Documents/.staging/<region>-<epoch>/` and verifies each by sha256 before anything is
    /// committed (H-01): a `<region>.commit.json` journal recording the planned moves plus the
    /// full new installed record is written first, then every destination is swapped in with
    /// `FileManager.replaceItemAt`, and only once every move has succeeded does the record get
    /// saved and the journal/staging dir get cleaned up. An already-staged, already-verified
    /// artifact is skipped on re-entry (H-03: a resumed install after a relaunch shouldn't
    /// redownload work a background transfer already finished).
    private func performInstall(region: String) async throws {
        let catalog = try await PackCatalogClient.fetchCatalog(region: region)
        let existing = Self.loadRecord(region: region)
        let docs = Self.documentsURL()
        let stagingDir = Self.stagingDirURL(docs: docs, region: region, epoch: catalog.osmSnapshotEpoch)
        // R16: a preflight, hash-free space check -- summed catalog sizes for whatever will
        // actually be (re)downloaded, no file hashing -- so a shortfall fails fast instead of
        // surfacing as a confusing write/verify error partway through a multi-GB transfer.
        try Self.checkDiskSpace(catalog: catalog, existing: existing, stagingDir: stagingDir)
        try FileManager.default.createDirectory(at: stagingDir, withIntermediateDirectories: true)

        var newArtifacts: [String: InstalledArtifactRecord] = [:]
        var journalEntries: [CommitJournalEntry] = []

        // The phase is already `.checking` here (set synchronously by `install()`) for the
        // pre-hash below -- `matches()` stays silent under it, same as before this ticket.
        for (kind, artifact) in catalog.artifacts.byKind {
            try Task.checkCancellation()

            let destURL = docs.appendingPathComponent(artifact.file)
            guard PackCatalogValidator.isDescendant(destURL, of: docs) else {
                throw PackCatalogValidationError.destinationEscapesDocuments(kind: kind, file: artifact.file)
            }

            let expected = InstalledArtifactRecord(file: artifact.file, sha256: artifact.sha256)
            if let existingArtifact = existing?.artifacts[kind], existingArtifact.sha256 == artifact.sha256,
               await Self.matches(existingArtifact, at: destURL) {
                newArtifacts[kind] = existingArtifact
                continue
            }

            let displayName = Self.displayName(forKind: kind)
            let stagedURL = stagingDir.appendingPathComponent(artifact.file)
            if !(await Self.matches(expected, at: stagedURL)) {
                readoptAfterInstall = true
                setPhase(region: region, .downloading(artifact: displayName), fraction: nil)
                let sourceURL = PackCatalogClient.baseURL.appendingPathComponent(artifact.path)
                try await downloadArtifact(
                    from: sourceURL, to: stagedURL, region: region, epoch: catalog.osmSnapshotEpoch,
                    artifactFile: artifact.file, sha256: artifact.sha256
                )

                // R8: throttled (~2%) progress on the post-download verify hash -- this is the
                // one place users actually wait on a silent, seconds-to-minutes step (the sha
                // pre-check under `.checking` stays silent, and R9 keeps the deep .rpack verify
                // below silent too, since it has no natural progress signal of its own).
                setPhase(region: region, .verifying(artifact: displayName), fraction: nil)
                // Off the main actor: hashing GBs takes seconds and must neither freeze the UI
                // nor pile autoreleased chunks onto a runloop that never turns.
                let digest = try await Task.detached(priority: .userInitiated) { [weak self] in
                    try Self.sha256Hex(of: stagedURL) { fraction in
                        Task { @MainActor in
                            self?.setPhase(region: region, .verifying(artifact: displayName), fraction: fraction)
                        }
                    }
                }.value
                guard digest == artifact.sha256 else {
                    throw PackInstallerError.checksumMismatch(kind: kind, expected: artifact.sha256, got: digest)
                }
            }

            if kind == "region_pack" {
                // One-time deep verification at install (issue #56 / SEC-006): sha256 only
                // proves the bytes weren't corrupted/truncated in transit, not that the
                // .rpack's internal structure (checksums + node/edge tables) is sound.
                // Deliberately kept out of Planner's regionPackPath: init/open path so every
                // later launch stays cheap -- this runs once per epoch, here, at install time.
                // Off the main actor, same as the sha256 above: costs seconds-to-minutes on
                // multi-GB packs. R9: no fake progress here -- there's no natural signal to
                // throttle against, unlike the byte-counted hash above.
                // wayfinder #55: release the active region's mmapped pack NOW, not at
                // install() entry -- this mmap is the only address-space-hungry step (the
                // hashes stream), and unloading any earlier would leave the app planner-less
                // for the entire download. Tonight's failure: verify couldn't open a
                // byte-perfect staged 4.6 GB pack while the old pack + map tiles were mapped.
                onDeepVerifyWillStart?(region)
                readoptAfterInstall = true
                setPhase(region: region, .verifying(artifact: "route data"), fraction: nil)
                try await Task.detached(priority: .userInitiated) {
                    try verifyRegionPack(path: stagedURL.path)
                }.value
            }

            newArtifacts[kind] = expected
            journalEntries.append(CommitJournalEntry(stagedFile: artifact.file, destinationFile: artifact.file, sha256: artifact.sha256))
        }

        setPhase(region: region, .finishing, fraction: nil)
        journalEntries.append(
            contentsOf: try await stageStyleFiles(region: region, epoch: catalog.osmSnapshotEpoch, stagingDir: stagingDir)
        )

        // All-or-nothing commit (H-01): everything needed is staged and verified at this
        // point. The journal is the commit point of no return -- once it's on disk, a crash
        // anywhere in the moves below is recoverable by `reconcileJournal` on next launch.
        let record = InstalledRecord(regionId: region, epoch: catalog.osmSnapshotEpoch, artifacts: newArtifacts)
        let journal = CommitJournal(regionId: region, epoch: catalog.osmSnapshotEpoch, entries: journalEntries, record: record)
        try Self.writeJournal(journal, docs: docs)

        for entry in journalEntries {
            let destURL = docs.appendingPathComponent(entry.destinationFile)
            try Self.applyReplace(from: stagingDir.appendingPathComponent(entry.stagedFile), to: destURL)
            // SEC-010: packs (and the shared style files) are multi-GB and reproducible from
            // the hosted catalog, so they don't belong in device backups -- unlike Trip Logs,
            // which deliberately stay backed up as user data.
            Self.excludeFromBackup(destURL)
        }

        try Self.saveRecord(record)
        try? FileManager.default.removeItem(at: Self.journalURL(docs: docs, region: region))
        try? FileManager.default.removeItem(at: stagingDir)
        Self.pruneDownloadManifest(forRegion: region, docs: docs)

        if let idx = rows.firstIndex(where: { $0.id == region }) {
            rows[idx].installedEpoch = record.epoch
            rows[idx].updateAvailable = false
            rows[idx].needsRepair = false
        } else {
            rows.append(RegionRow(id: region, name: catalog.regionName, installedEpoch: record.epoch))
            rows.sort { $0.id < $1.id }
        }
    }

    /// True if `url` exists and hashes to `expected.sha256`. Used both to confirm an installed
    /// record isn't lying about a file that's since been deleted/corrupted (M-01) and to skip
    /// re-downloading an artifact a previous (possibly killed) install attempt already staged
    /// and verified (H-03).
    private static func matches(_ expected: InstalledArtifactRecord, at url: URL) async -> Bool {
        guard FileManager.default.fileExists(atPath: url.path) else { return false }
        let digest = try? await Task.detached(priority: .userInitiated) { try Self.sha256Hex(of: url) }.value
        return digest == expected.sha256
    }

    /// Tiny, untracked by sha against the catalog -- always refetched from the epoch dir on
    /// every commit, shared (non-region-specific) across whichever region installs last.
    /// Staged (not written straight to Documents) like every other commit entry, with its own
    /// content hash recorded so the journal/reconciliation treat it uniformly with artifacts.
    private func stageStyleFiles(region: String, epoch: Int, stagingDir: URL) async throws -> [CommitJournalEntry] {
        var entries: [CommitJournalEntry] = []
        for name in ["style-light.json", "style-dark.json"] {
            let url = PackCatalogClient.baseURL.appendingPathComponent("packs/\(region)/\(epoch)/\(name)")
            let (data, response) = try await URLSession.shared.data(from: url)
            guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
                throw PackInstallerError.badResponse
            }
            try data.write(to: stagingDir.appendingPathComponent(name), options: .atomic)
            let sha256 = SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
            entries.append(CommitJournalEntry(stagedFile: name, destinationFile: name, sha256: sha256))
        }
        return entries
    }

    /// Removes the region's artifact files and its installed record; leaves the shared style
    /// files (another region may use them). M-03: file-removal failures are no longer
    /// swallowed. Deliberately files-first: the record is only removed once every artifact
    /// file is confirmed gone, so a partial failure leaves the record pointing at a
    /// now-file-missing region, which `refreshRows()`'s M-01 check then shows as needing
    /// repair rather than silently forgetting the region was ever installed with orphaned
    /// files left behind.
    func delete(region: String) throws {
        let docs = Self.documentsURL()
        do {
            if let record = Self.loadRecord(region: region) {
                var failed: [String] = []
                for artifact in record.artifacts.values {
                    let url = docs.appendingPathComponent(artifact.file)
                    guard FileManager.default.fileExists(atPath: url.path) else { continue }
                    do {
                        try FileManager.default.removeItem(at: url)
                    } catch {
                        failed.append(artifact.file)
                    }
                }
                guard failed.isEmpty else {
                    refreshRows()
                    throw PackInstallerError.partialDelete(files: failed)
                }
            }

            let recordFileURL = Self.recordURL(region: region)
            if FileManager.default.fileExists(atPath: recordFileURL.path) {
                try FileManager.default.removeItem(at: recordFileURL)
            }

            if let idx = rows.firstIndex(where: { $0.id == region }) {
                rows[idx].installedEpoch = nil
                rows[idx].updateAvailable = false
                rows[idx].needsRepair = false
            }
            lastOperationError = nil
        } catch {
            lastOperationError = "\(region): \(Self.userMessage(for: error))"
            throw error
        }
    }

    private static func userMessage(for error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? String(describing: error)
    }

    /// R16: sums the catalog's declared `bytes` for every artifact whose record sha differs
    /// from the catalog's (i.e. whatever `performInstall`'s loop below will actually download --
    /// a record match downloads nothing), plus 500 MB headroom for the staged copies sitting
    /// alongside the destinations mid-commit, and compares against the Documents volume's
    /// available capacity. Bytes already sitting in staging are credited (a resumed install
    /// must not be blocked by space its own earlier download already claimed). No file hashing
    /// here -- this runs before anything new is staged.
    private static func checkDiskSpace(catalog: PackCatalog, existing: InstalledRecord?, stagingDir: URL) throws {
        var needed: Int64 = 500_000_000
        for (kind, artifact) in catalog.artifacts.byKind
        where existing?.artifacts[kind]?.sha256 != artifact.sha256 {
            let stagedPath = stagingDir.appendingPathComponent(artifact.file).path
            let stagedBytes = (try? FileManager.default.attributesOfItem(atPath: stagedPath))?[.size] as? Int64 ?? 0
            needed += max(0, artifact.bytes - stagedBytes)
        }

        let docs = documentsURL()
        guard let available = try? docs.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage
        else { return } // Can't determine free space -- don't block the install on that.

        guard available >= needed else {
            throw PackInstallerError.insufficientSpace(needed: needed, available: available, regionName: catalog.regionName)
        }
    }

    // MARK: Commit journal (H-01)

    private static func writeJournal(_ journal: CommitJournal, docs: URL) throws {
        let data = try JSONEncoder().encode(journal)
        try data.write(to: journalURL(docs: docs, region: journal.regionId), options: .atomic)
    }

    /// Per-file atomic replace: `replaceItemAt` swaps the destination in a single step, so a
    /// crash mid-commit never leaves a moment with no destination file at all -- unlike
    /// remove-then-move, which has exactly that window. A destination that doesn't exist yet
    /// just gets a plain move. If the staged file is already gone, this move already happened
    /// (earlier in this same commit, or during reconciliation) -- a no-op.
    private static func applyReplace(from stagedURL: URL, to destURL: URL) throws {
        guard FileManager.default.fileExists(atPath: stagedURL.path) else { return }
        if FileManager.default.fileExists(atPath: destURL.path) {
            _ = try FileManager.default.replaceItemAt(destURL, withItemAt: stagedURL)
        } else {
            try FileManager.default.moveItem(at: stagedURL, to: destURL)
        }
    }

    /// Launch-time roll-forward (H-01/H-03): if the process died mid-commit, one or more
    /// `<region>.commit.json` journals are still on disk under `Documents/.staging/`. Called
    /// from `init()` before `rows` is seeded, so the UI never sees a half-committed region.
    static func reconcileJournals(documentsURL docs: URL) {
        guard let files = try? FileManager.default.contentsOfDirectory(atPath: stagingRootURL(docs: docs).path) else { return }
        for file in files where file.hasSuffix(".commit.json") {
            let region = String(file.dropLast(".commit.json".count))
            reconcileJournal(region: region, documentsURL: docs)
        }
    }

    /// Rolls one region's interrupted commit forward, or abandons it. For each journal entry:
    /// if the staged file is still there, replace-move it into place; otherwise, if the
    /// destination already sha-matches the journal (it was already moved before the crash),
    /// treat it as done. If neither holds -- the staged copy is gone and the destination
    /// doesn't match -- the transaction can't be finished (the download would have to be
    /// redone from scratch): abandon it, deleting the journal and staging dir and leaving the
    /// previous install's record exactly as it was. That is the all-or-nothing promise: a
    /// crash never leaves a HALF-applied transaction, not that every crash resumes.
    ///
    /// Exposed (not private) and taking `documentsURL` directly so `--autotest install-smoke`
    /// can drive it with synthetic staged files + journals, without touching the network.
    static func reconcileJournal(region: String, documentsURL docs: URL) {
        let journalFileURL = journalURL(docs: docs, region: region)
        guard let data = try? Data(contentsOf: journalFileURL),
              let journal = try? JSONDecoder().decode(CommitJournal.self, from: data)
        else { return }
        let stagingDir = stagingDirURL(docs: docs, region: journal.regionId, epoch: journal.epoch)

        for entry in journal.entries {
            let stagedURL = stagingDir.appendingPathComponent(entry.stagedFile)
            let destURL = docs.appendingPathComponent(entry.destinationFile)
            if FileManager.default.fileExists(atPath: stagedURL.path) {
                try? applyReplace(from: stagedURL, to: destURL)
            } else if (try? sha256Hex(of: destURL)) != entry.sha256 {
                // Unrecoverable. But abandoning must not leave the OLD record vouching for
                // MIXED bytes: if any other destination was already swapped to its new
                // content (before the crash, or earlier in this loop), the record now lies
                // about what's on disk -- delete it, so the region shows as not installed
                // and a reinstall (which re-hashes every artifact, M-01) heals the mix.
                // With nothing applied, the previous install is genuinely intact: keep it.
                let anyApplied = journal.entries.contains { other in
                    (try? sha256Hex(of: docs.appendingPathComponent(other.destinationFile))) == other.sha256
                }
                if anyApplied {
                    try? FileManager.default.removeItem(at: recordURL(region: journal.regionId))
                }
                try? FileManager.default.removeItem(at: journalFileURL)
                try? FileManager.default.removeItem(at: stagingDir)
                return
            }
        }

        try? saveRecord(journal.record)
        try? FileManager.default.removeItem(at: journalFileURL)
        try? FileManager.default.removeItem(at: stagingDir)
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

    /// Recreates the background session (same identifier, so iOS reattaches any tasks that
    /// kept running or finished while the process was gone) and reconciles it against the
    /// download manifest (H-03). A task still in flight needs nothing further here -- the
    /// delegate methods below already resolve its destination from the manifest. A manifest
    /// entry with no live task is orphaned -- its transfer ended without us finding out -- so
    /// its region's install is resumed to make progress again.
    private func reattachBackgroundSession() {
        session().getAllTasks { [weak self] tasks in
            Task { @MainActor in
                self?.adoptRunningTasks(tasks)
            }
        }
    }

    private func adoptRunningTasks(_ tasks: [URLSessionTask]) {
        for task in tasks {
            print("PackInstaller: reattached background task \(task.taskIdentifier), state=\(task.state.rawValue)")
        }

        let liveTaskIds = Set(tasks.map { $0.taskIdentifier })
        let docs = Self.documentsURL()
        var manifest = Self.loadDownloadManifest(docs: docs)

        // R14/R15: a manifest entry with a still-live task is a background download that's
        // simply continuing across the relaunch -- show that on the row right away rather than
        // waiting for the next `didWriteData` callback (which may be seconds off, or, if the
        // transfer already finished, may never come).
        for (key, entry) in manifest where liveTaskIds.contains(Int(key) ?? -1) {
            setPhase(region: entry.region, .downloading(artifact: Self.displayName(forArtifactFile: entry.artifactFile)), fraction: nil)
        }

        let orphanedRegions = Set(manifest.compactMap { key, entry in
            liveTaskIds.contains(Int(key) ?? -1) ? nil : entry.region
        })
        for key in manifest.keys where !liveTaskIds.contains(Int(key) ?? -1) {
            manifest.removeValue(forKey: key)
        }
        Self.saveDownloadManifest(manifest, docs: docs)

        // Orphaned entries: their region's own `install()` sets `.checking` synchronously,
        // which is feedback enough (R14/R15) -- nothing extra needed here.
        for region in orphanedRegions where currentInstallRegion != region {
            Task { try? await self.install(region: region) }
        }
    }

    private func downloadArtifact(
        from url: URL, to destination: URL, region: String, epoch: Int, artifactFile: String, sha256: String
    ) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            self.pendingContinuation = continuation
            self.pendingDestination = destination
            let task = self.session().downloadTask(with: url)
            var manifest = Self.loadDownloadManifest(docs: Self.documentsURL())
            manifest[String(task.taskIdentifier)] = DownloadManifestEntry(
                region: region, epoch: epoch, artifactFile: artifactFile, expectedSha256: sha256,
                stagedDestinationPath: destination.path
            )
            Self.saveDownloadManifest(manifest, docs: Self.documentsURL())
            task.resume()
        }
    }

    /// R2/R4: the single place that drives `RegionRow.phase`/`phaseFraction` -- mirrors the old
    /// `setProgress`, generalized from a bare fraction to the full install/update phase.
    private func setPhase(region: String, _ phase: RegionRow.Phase?, fraction: Double?) {
        guard let idx = rows.firstIndex(where: { $0.id == region }) else { return }
        if rows[idx].phase != phase {
            // A phase boundary invalidates the byte total; `didWriteData` re-sets it per task.
            rows[idx].phaseTotalBytes = nil
        }
        rows[idx].phase = phase
        rows[idx].phaseFraction = fraction
    }

    /// R3: never show raw kind strings or filenames in the UI.
    private static func displayName(forKind kind: String) -> String {
        switch kind {
        case "region_pack": return "route data"
        case "map_pack": return "map tiles"
        case "charger_pack": return "chargers"
        default: return "packs"
        }
    }

    /// R14/R15: `DownloadManifestEntry.artifactFile` is a filename (e.g. "eu-west.rpack"), not
    /// a kind string -- mapped by suffix rather than threading the kind into the manifest for
    /// this alone. Falls back to the generic "packs" label, same as `displayName(forKind:)`'s
    /// default case.
    private static func displayName(forArtifactFile file: String) -> String {
        if file.hasSuffix(".rpack") { return "route data" }
        if file.hasSuffix(".pmtiles") { return "map tiles" }
        if file.hasSuffix("-chargers.json") { return "chargers" }
        return "packs"
    }

    // MARK: Persistence

    private static func documentsURL() -> URL {
        FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
    }

    private static func stagingRootURL(docs: URL) -> URL {
        docs.appendingPathComponent(".staging", isDirectory: true)
    }

    static func stagingDirURL(docs: URL, region: String, epoch: Int) -> URL {
        stagingRootURL(docs: docs).appendingPathComponent("\(region)-\(epoch)", isDirectory: true)
    }

    static func journalURL(docs: URL, region: String) -> URL {
        stagingRootURL(docs: docs).appendingPathComponent("\(region).commit.json")
    }

    private static func downloadManifestURL(docs: URL) -> URL {
        stagingRootURL(docs: docs).appendingPathComponent("downloads.json")
    }

    private static func loadDownloadManifest(docs: URL) -> [String: DownloadManifestEntry] {
        guard let data = try? Data(contentsOf: downloadManifestURL(docs: docs)) else { return [:] }
        return (try? JSONDecoder().decode([String: DownloadManifestEntry].self, from: data)) ?? [:]
    }

    private static func saveDownloadManifest(_ manifest: [String: DownloadManifestEntry], docs: URL) {
        guard let data = try? JSONEncoder().encode(manifest) else { return }
        try? data.write(to: downloadManifestURL(docs: docs), options: .atomic)
    }

    private static func pruneDownloadManifest(forRegion region: String, docs: URL) {
        var manifest = loadDownloadManifest(docs: docs)
        manifest = manifest.filter { $0.value.region != region }
        saveDownloadManifest(manifest, docs: docs)
    }

    private static func recordURL(region: String) -> URL {
        documentsURL().appendingPathComponent("installed-\(region).json")
    }

    static func loadRecord(region: String) -> InstalledRecord? {
        guard let data = try? Data(contentsOf: recordURL(region: region)) else { return nil }
        return try? JSONDecoder().decode(InstalledRecord.self, from: data)
    }

    /// Internal (not private) so `--autotest install-smoke` can seed a synthetic record for
    /// the M-01 needs-repair check without a real install.
    static func saveRecord(_ record: InstalledRecord) throws {
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
    /// R8: `progress`, when given, is called with a 0...1 fraction at most every ~2% of
    /// `url`'s size -- coarse enough to be cheap against a GB-scale file, fine enough that the
    /// verifying row doesn't look stuck. `matches()` below calls this with no progress (that
    /// check runs silently under `.checking`); `performInstall`'s post-download verify is the
    /// only caller that passes one.
    nonisolated private static func sha256Hex(of url: URL, progress: (@Sendable (Double) -> Void)? = nil) throws -> String {
        let handle = try FileHandle(forReadingFrom: url)
        defer { try? handle.close() }
        let totalBytes = (try? FileManager.default.attributesOfItem(atPath: url.path))?[.size] as? NSNumber
        var hasher = SHA256()
        var bytesRead: Int64 = 0
        var lastReportedFraction = 0.0
        while try autoreleasepool(invoking: {
            guard let chunk = try handle.read(upToCount: 4 * 1024 * 1024), !chunk.isEmpty else {
                return false
            }
            hasher.update(data: chunk)
            if let progress, let totalBytes, totalBytes.int64Value > 0 {
                bytesRead += Int64(chunk.count)
                let fraction = Double(bytesRead) / Double(totalBytes.int64Value)
                if fraction - lastReportedFraction >= 0.02 {
                    lastReportedFraction = fraction
                    progress(fraction)
                }
            }
            return true
        }) {}
        return hasher.finalize().map { String(format: "%02x", $0) }.joined()
    }
}

// Swift 6 strict concurrency (M-05 -- docs/codebase-audit-2026-08-29.md): `@preconcurrency`
// conformance is sound for every method below, including `didFinishDownloadingTo`, because the
// session was created with `delegateQueue: .main` -- every callback on it, not just this one,
// is genuinely delivered on the main thread.
extension PackInstaller: @preconcurrency URLSessionDownloadDelegate {
    /// `location` is a temp file deleted as soon as this method returns, so the move to the
    /// resolved destination must happen synchronously here, on the main queue (the session was
    /// created with `delegateQueue: .main`) -- matching every other MLNMapViewDelegate/
    /// CLLocationManagerDelegate callback in this app that mutates @MainActor state directly.
    /// H-03: the destination is resolved from the on-disk manifest by task identifier first --
    /// the only lookup that survives a process relaunch -- falling back to the in-memory
    /// `pendingDestination` set by `downloadArtifact` for the common same-process case.
    func urlSession(_ session: URLSession, downloadTask: URLSessionDownloadTask, didFinishDownloadingTo location: URL) {
        let docs = Self.documentsURL()
        var manifest = Self.loadDownloadManifest(docs: docs)
        let manifestEntry = manifest[String(downloadTask.taskIdentifier)]
        let resolvedDestination = manifestEntry.map { URL(fileURLWithPath: $0.stagedDestinationPath) } ?? pendingDestination
        guard let destination = resolvedDestination else { return }

        do {
            if FileManager.default.fileExists(atPath: destination.path) {
                try FileManager.default.removeItem(at: destination)
            }
            try FileManager.default.moveItem(at: location, to: destination)
            pendingContinuation?.resume()
        } catch {
            pendingContinuation?.resume(throwing: error)
        }
        // H-03: with no continuation waiting, this transfer finished across a process
        // relaunch -- the file just landed in staging, but no install() is driving the
        // region forward. Resume it, or the staged artifact parks until a manual tap.
        let finishedWithoutInstall = pendingContinuation == nil && currentInstallRegion == nil
        if finishedWithoutInstall, let entry = manifestEntry {
            Task { try? await self.install(region: entry.region) }
        }
        pendingContinuation = nil
        pendingDestination = nil

        manifest.removeValue(forKey: String(downloadTask.taskIdentifier))
        Self.saveDownloadManifest(manifest, docs: docs)
    }

    func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
        guard let error else { return }
        pendingContinuation?.resume(throwing: error)
        pendingContinuation = nil
        pendingDestination = nil

        let docs = Self.documentsURL()
        var manifest = Self.loadDownloadManifest(docs: docs)
        manifest.removeValue(forKey: String(task.taskIdentifier))
        Self.saveDownloadManifest(manifest, docs: docs)
    }

    func urlSession(
        _ session: URLSession, downloadTask: URLSessionDownloadTask, didWriteData bytesWritten: Int64,
        totalBytesWritten: Int64, totalBytesExpectedToWrite: Int64
    ) {
        guard totalBytesExpectedToWrite > 0, let region = currentInstallRegion,
              let idx = rows.firstIndex(where: { $0.id == region })
        else { return }
        // `performInstall` already sets `.downloading` synchronously before this task starts;
        // the fallback below only matters if a callback somehow lands before that (or for a
        // reattached task -- `adoptRunningTasks` also sets it, but belt-and-braces here too).
        if case .downloading = rows[idx].phase {} else {
            rows[idx].phase = .downloading(artifact: "packs")
        }
        rows[idx].phaseFraction = Double(totalBytesWritten) / Double(totalBytesExpectedToWrite)
        rows[idx].phaseTotalBytes = totalBytesExpectedToWrite
    }

    /// H-03: iOS relaunches (or wakes a suspended) app to deliver this once the background
    /// session identifier's events have finished; `AppDelegate` (WayfinderApp.swift) stashed
    /// the completion handler `application(_:handleEventsForBackgroundURLSession:
    /// completionHandler:)` handed it. Calling it tells the OS this app is done processing and
    /// can be suspended/snapshotted again.
    func urlSessionDidFinishEvents(forBackgroundURLSession session: URLSession) {
        guard let appDelegate = UIApplication.shared.delegate as? AppDelegate else { return }
        appDelegate.backgroundSessionCompletionHandler?()
        appDelegate.backgroundSessionCompletionHandler = nil
    }
}
