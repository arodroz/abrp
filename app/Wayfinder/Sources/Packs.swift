// Locates a sideloaded Region Pack in the app's Documents directory, by
// region id: `<region>.rpack` + `<region>-chargers.json`. Packs are
// sideloaded externally for now (see app/README.md); this is also where
// M3's installer will write them.
import Foundation

enum Packs {
    struct Located {
        let rpackURL: URL
        let chargersURL: URL
    }

    static func locate(region: String) -> Located? {
        let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
        let rpackURL = docs.appendingPathComponent("\(region).rpack")
        let chargersURL = docs.appendingPathComponent("\(region)-chargers.json")
        guard FileManager.default.fileExists(atPath: rpackURL.path),
              FileManager.default.fileExists(atPath: chargersURL.path)
        else {
            return nil
        }
        return Located(rpackURL: rpackURL, chargersURL: chargersURL)
    }
}
