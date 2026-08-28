// Parses the pipeline's cpack-1 charger pack format (`corridor-chargers.json`): a JSON object
// `{"built_at_epoch", "charger_count", "chargers": [...]}` where each charger record has
// `id`, `name`, `lat`, `lon`, `max_power_kw`, `operator`, `country`, `source`, `access`, and
// `connectors: [{"power_kw", "standard"}]`. The prototype this was salvaged from read a
// pre-built `chargers.geojson`; production has no such file, so this decodes the fields the
// Chargers map layer needs (name, position, max power) directly from the cpack-1 JSON.
import Foundation

struct CPack1Charger: Decodable {
    let name: String
    let lat: Double
    let lon: Double
    let maxPowerKw: Double
    let operatorName: String?

    enum CodingKeys: String, CodingKey {
        case name, lat, lon
        case maxPowerKw = "max_power_kw"
        case operatorName = "operator"
    }
}

private struct CPack1Root: Decodable {
    let chargers: [CPack1Charger]
}

enum CPack1 {
    static func parseChargers(data: Data) throws -> [CPack1Charger] {
        try JSONDecoder().decode(CPack1Root.self, from: data).chargers
    }
}
