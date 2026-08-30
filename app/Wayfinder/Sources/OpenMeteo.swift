// Minimal Open-Meteo client (wayfinder #51): ambient temperature for a Trip Log's midpoint
// sample. Decided weather source per docs/research/weather-elevation-sources.md §2.1 --
// CC BY 4.0, no API key. A Trip Log only needs one representative temperature, unlike the
// Energy Model's per-Leg weather sampling, so this is a single coordinate/time lookup.
import Foundation

enum OpenMeteo {
    private struct HourlyResponse: Decodable {
        struct Hourly: Decodable {
            let time: [Int]
            let temperature2m: [Double?]

            enum CodingKeys: String, CodingKey {
                case time
                case temperature2m = "temperature_2m"
            }
        }
        let hourly: Hourly
    }

    /// Nearest hourly temperature to `unix` at (lat, lon). Any failure -- network, decode,
    /// all-null hourly data -- returns nil; a Trip Log with a null ambient temp is still valid
    /// (ADR 0009 point 1 makes weather automatic-but-best-effort, not load-bearing for the fit).
    static func temperatureC(lat: Double, lon: Double, unix: Int) async -> Double? {
        // Coarsened to 2 decimal places, ~1.1 km (issue #56 / SEC-009): coarser than
        // Open-Meteo's own forecast-model grid, so no accuracy is lost, and the exact trip
        // midpoint never leaves the device.
        let roundedLat = (lat * 100).rounded() / 100
        let roundedLon = (lon * 100).rounded() / 100

        var components = URLComponents(string: "https://api.open-meteo.com/v1/forecast")!
        components.queryItems = [
            URLQueryItem(name: "latitude", value: String(roundedLat)),
            URLQueryItem(name: "longitude", value: String(roundedLon)),
            URLQueryItem(name: "hourly", value: "temperature_2m"),
            URLQueryItem(name: "past_days", value: "1"),
            URLQueryItem(name: "forecast_days", value: "1"),
            URLQueryItem(name: "timeformat", value: "unixtime"),
        ]
        guard let url = components.url else { return nil }

        // ~8s timeout so a dead network can't hang the Trip Log save.
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 8
        config.timeoutIntervalForResource = 8
        let session = URLSession(configuration: config)

        guard let (data, _) = try? await session.data(from: url),
              let response = try? JSONDecoder().decode(HourlyResponse.self, from: data)
        else { return nil }

        let hourly = response.hourly
        var best: Double?
        var bestDeltaS = Int.max
        for (index, hourUnix) in hourly.time.enumerated() {
            guard index < hourly.temperature2m.count, let temp = hourly.temperature2m[index] else { continue }
            let deltaS = abs(hourUnix - unix)
            if deltaS < bestDeltaS {
                bestDeltaS = deltaS
                best = temp
            }
        }
        return best
    }
}
