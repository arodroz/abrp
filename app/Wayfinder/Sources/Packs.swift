// Locates a sideloaded Region Pack + Map Pack in the app's Documents directory, by region
// id: `<region>.rpack` + `<region>-chargers.json` + `<region>.pmtiles`, plus the pipeline's
// shared (non-region-specific) `style-light.json` / `style-dark.json`. Packs are sideloaded
// externally for now (see app/README.md); this is also where M3's installer will write them.
import Foundation

enum Packs {
    struct Located {
        let rpackURL: URL
        let chargersURL: URL
        let pmtilesURL: URL
        let styleLightURL: URL
        let styleDarkURL: URL
    }

    static func locate(region: String) -> Located? {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let rpackURL = docs.appendingPathComponent("\(region).rpack")
        let chargersURL = docs.appendingPathComponent("\(region)-chargers.json")
        let pmtilesURL = docs.appendingPathComponent("\(region).pmtiles")
        let styleLightURL = docs.appendingPathComponent("style-light.json")
        let styleDarkURL = docs.appendingPathComponent("style-dark.json")

        let allURLs = [rpackURL, chargersURL, pmtilesURL, styleLightURL, styleDarkURL]
        guard allURLs.allSatisfy({ FileManager.default.fileExists(atPath: $0.path) }) else {
            return nil
        }
        return Located(
            rpackURL: rpackURL, chargersURL: chargersURL, pmtilesURL: pmtilesURL,
            styleLightURL: styleLightURL, styleDarkURL: styleDarkURL
        )
    }
}
