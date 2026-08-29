// Codable models for the hosted pack catalog (ADR 0011, wayfinder #47): the top-level
// `packs/index.json` region list and each region's `packs/<id>/catalog.json`. This is the
// app's one hosting seam -- `PackCatalogClient.baseURL` is the only hardcoded host, everything
// else (index/catalog/epoch/artifact paths) comes from the fetched JSON itself. Because of
// that, `PackCatalogValidator` (wayfinder #47 audit remediation, M-02 --
// docs/codebase-audit-2026-08-29.md) validates the decoded shape right after every fetch,
// before any of it reaches the filesystem or another network request: a malformed or
// compromised catalog must fail loudly here rather than let a `../escape`-style path or a
// region-id mismatch reach `PackInstaller`.
import Foundation

struct PackIndex: Decodable {
    let indexFormat: Int
    let regions: [PackIndexRegion]
}

struct PackIndexRegion: Decodable {
    let id: String
    let name: String
    let latestEpoch: Int
    let totalBytes: Int64
}

/// One artifact's catalog entry, extended (per ADR 0011) with a bucket-relative `path` beyond
/// the pipeline's own `file`/`bytes`/`sha256`.
struct PackArtifact: Decodable {
    let file: String
    let bytes: Int64
    let sha256: String
    let path: String
}

struct PackArtifacts: Decodable {
    let regionPack: PackArtifact
    let chargerPack: PackArtifact
    let mapPack: PackArtifact

    /// Keyed the same way as `InstalledRecord.artifacts` and the publish script's kind
    /// strings, so installer code can iterate generically instead of naming each field.
    var byKind: [(kind: String, artifact: PackArtifact)] {
        [("region_pack", regionPack), ("charger_pack", chargerPack), ("map_pack", mapPack)]
    }
}

struct PackCatalog: Decodable {
    let regionId: String
    let regionName: String
    let osmSnapshotEpoch: Int
    let artifacts: PackArtifacts
}

enum PackCatalogError: Error {
    case badResponse
}

/// M-02 (docs/codebase-audit-2026-08-29.md): the catalog is server-controlled JSON that gets
/// appended directly into local and remote URLs, so every field that becomes part of a path
/// gets checked here before anything downstream trusts it. Every validator is a pure function
/// over the decoded types (no filesystem/network access), so it's directly unit-testable --
/// `--autotest install-smoke` exercises the malformed-input cases offline.
enum PackCatalogValidationError: Error, LocalizedError {
    case invalidRegionId(String)
    case regionMismatch(requested: String, got: String)
    case invalidArtifactFile(kind: String, file: String)
    case invalidSha256(kind: String, sha256: String)
    case negativeBytes(kind: String, bytes: Int64)
    case pathEscapesBase(kind: String, path: String)
    case destinationEscapesDocuments(kind: String, file: String)

    var errorDescription: String? {
        switch self {
        case .invalidRegionId(let id):
            return "Invalid region id: \(id)"
        case .regionMismatch(let requested, let got):
            return "Catalog region mismatch: requested \(requested), got \(got)"
        case .invalidArtifactFile(let kind, let file):
            return "Invalid artifact file for \(kind): \(file)"
        case .invalidSha256(let kind, let sha256):
            return "Invalid sha256 for \(kind): \(sha256)"
        case .negativeBytes(let kind, let bytes):
            return "Negative byte count for \(kind): \(bytes)"
        case .pathEscapesBase(let kind, let path):
            return "Artifact path escapes the base URL for \(kind): \(path)"
        case .destinationEscapesDocuments(let kind, let file):
            return "Destination escapes Documents for \(kind): \(file)"
        }
    }
}

enum PackCatalogValidator {
    private static let regionIdPattern = "^[a-z0-9-]{1,32}$"
    private static let sha256Pattern = "^[0-9a-f]{64}$"

    static func validateRegionId(_ id: String) -> Bool {
        id.range(of: regionIdPattern, options: .regularExpression) != nil
    }

    static func validateSha256(_ sha256: String) -> Bool {
        sha256.range(of: sha256Pattern, options: .regularExpression) != nil
    }

    /// Artifact `file` values must be leaf names: no path separators, no `..`, nonempty --
    /// they get appended straight onto both the staging dir and Documents.
    static func validateFileName(_ file: String) -> Bool {
        !file.isEmpty && !file.contains("/") && !file.contains("\\") && !file.contains("..")
    }

    static func validateBytes(_ bytes: Int64) -> Bool {
        bytes >= 0
    }

    /// Remote artifact paths must be plain relative `packs/...` paths (the ADR 0011 layout):
    /// no absolute paths, no `.`/`..` components at all. A pure prefix check against the base
    /// URL's path is NOT enough -- the base is the host root (empty path), so after
    /// standardization every `../escape` clamps back to `/` and would pass; pinning to the
    /// `packs/` prefix and rejecting dot components is what gives the check teeth.
    static func validateRemotePath(_ path: String, baseURL: URL) -> Bool {
        guard !path.isEmpty, !path.hasPrefix("/") else { return false }
        let components = path.split(separator: "/")
        guard components.first == "packs", !components.contains(".."), !components.contains(".") else {
            return false
        }
        let resolvedPath = baseURL.appendingPathComponent(path).standardized.path
        let packsPrefix = baseURL.appendingPathComponent("packs").standardized.path + "/"
        return resolvedPath.hasPrefix(packsPrefix)
    }

    /// Same shape of check as `validateRemotePath`, for a constructed local destination URL
    /// (M-02: "assert every constructed local destination URL, standardized, remains a
    /// descendant of Documents").
    static func isDescendant(_ url: URL, of base: URL) -> Bool {
        let childPath = url.standardizedFileURL.path
        let basePath = base.standardizedFileURL.path
        let basePrefix = basePath.hasSuffix("/") ? basePath : basePath + "/"
        return childPath.hasPrefix(basePrefix)
    }

    static func validate(index: PackIndex) throws {
        for region in index.regions {
            guard validateRegionId(region.id) else { throw PackCatalogValidationError.invalidRegionId(region.id) }
            guard validateBytes(region.totalBytes) else {
                throw PackCatalogValidationError.negativeBytes(kind: region.id, bytes: region.totalBytes)
            }
        }
    }

    static func validate(catalog: PackCatalog, requestedRegion: String) throws {
        guard validateRegionId(catalog.regionId) else {
            throw PackCatalogValidationError.invalidRegionId(catalog.regionId)
        }
        guard catalog.regionId == requestedRegion else {
            throw PackCatalogValidationError.regionMismatch(requested: requestedRegion, got: catalog.regionId)
        }
        for (kind, artifact) in catalog.artifacts.byKind {
            guard validateFileName(artifact.file) else {
                throw PackCatalogValidationError.invalidArtifactFile(kind: kind, file: artifact.file)
            }
            guard validateSha256(artifact.sha256) else {
                throw PackCatalogValidationError.invalidSha256(kind: kind, sha256: artifact.sha256)
            }
            guard validateBytes(artifact.bytes) else {
                throw PackCatalogValidationError.negativeBytes(kind: kind, bytes: artifact.bytes)
            }
            guard validateRemotePath(artifact.path, baseURL: PackCatalogClient.baseURL) else {
                throw PackCatalogValidationError.pathEscapesBase(kind: kind, path: artifact.path)
            }
        }
    }
}

/// Fetches the index and per-region catalogs over plain HTTPS (`URLSession.shared`) against
/// the one base URL ADR 0011 designates as the app's hosting seam.
enum PackCatalogClient {
    static let baseURL = URL(string: "https://wayfinder-packs.home.anteras.org")!

    static func fetchIndex() async throws -> PackIndex {
        let index: PackIndex = try await fetch(baseURL.appendingPathComponent("packs/index.json"))
        try PackCatalogValidator.validate(index: index)
        return index
    }

    static func fetchCatalog(region: String) async throws -> PackCatalog {
        let catalog: PackCatalog = try await fetch(baseURL.appendingPathComponent("packs/\(region)/catalog.json"))
        try PackCatalogValidator.validate(catalog: catalog, requestedRegion: region)
        return catalog
    }

    private static func fetch<T: Decodable>(_ url: URL) async throws -> T {
        let (data, response) = try await URLSession.shared.data(from: url)
        guard let http = response as? HTTPURLResponse, 200..<300 ~= http.statusCode else {
            throw PackCatalogError.badResponse
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try decoder.decode(T.self, from: data)
    }
}
