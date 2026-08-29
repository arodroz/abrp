// Codable models for the hosted pack catalog (ADR 0011, wayfinder #47): the top-level
// `packs/index.json` region list and each region's `packs/<id>/catalog.json`. This is the
// app's one hosting seam -- `PackCatalogClient.baseURL` is the only hardcoded host, everything
// else (index/catalog/epoch/artifact paths) comes from the fetched JSON itself.
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

/// Fetches the index and per-region catalogs over plain HTTPS (`URLSession.shared`) against
/// the one base URL ADR 0011 designates as the app's hosting seam.
enum PackCatalogClient {
    static let baseURL = URL(string: "https://wayfinder-packs.home.anteras.org")!

    static func fetchIndex() async throws -> PackIndex {
        try await fetch(baseURL.appendingPathComponent("packs/index.json"))
    }

    static func fetchCatalog(region: String) async throws -> PackCatalog {
        try await fetch(baseURL.appendingPathComponent("packs/\(region)/catalog.json"))
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
