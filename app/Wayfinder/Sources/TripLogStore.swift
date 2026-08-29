// Trip Log capture lifecycle (wayfinder #51), per ADR 0009: manual start/stop with a dash-SoC
// prompt at each end, ~1Hz GPS trace, ambient temperature at the trip midpoint, saved as a
// tlog-1 JSON. Owns its OWN CLLocationManager -- PlanStore's stays untouched, since the two
// serve different purposes (route-editor origin adoption vs. a continuous recording trace) and
// PlanStore's accuracy/background settings must not change for this.
import CoreLocation
import Foundation

@MainActor
@Observable
final class TripLogStore: NSObject, CLLocationManagerDelegate {
    enum Phase: Equatable {
        case idle
        case promptingStartSoc
        case recording
        case promptingEndSoc
    }

    private(set) var phase: Phase = .idle
    private(set) var tripStartDate: Date?
    private(set) var sampleCount = 0
    /// Accumulated haversine distance (via CLLocation.distance(from:)) between consecutive
    /// kept samples, for the recording pill's live km readout.
    private(set) var distanceM: Double = 0
    private(set) var lastSavedURL: URL?
    private(set) var logs: [URL] = []
    private(set) var saveErrorMessage: String?

    /// Injectable so triplog-smoke can stub a deterministic ambient temperature instead of
    /// depending on the network for a pass/fail result.
    var fetchTemperature: @Sendable (Double, Double, Int) async -> Double? = OpenMeteo.temperatureC

    private let locationManager = CLLocationManager()
    private var samples: [TripSample] = []
    private var lastKeptLocation: CLLocation?
    private var startSocPct = 0

    override init() {
        super.init()
        locationManager.delegate = self
    }

    // MARK: Capture lifecycle

    func startTapped() {
        guard phase == .idle else { return }
        phase = .promptingStartSoc
    }

    func cancelStartSoc() {
        guard phase == .promptingStartSoc else { return }
        phase = .idle
    }

    func confirmStartSoc(_ pct: Int) {
        guard phase == .promptingStartSoc else { return }
        startSocPct = min(max(pct, 0), 100)
        tripStartDate = Date.now
        samples = []
        lastKeptLocation = nil
        sampleCount = 0
        distanceM = 0
        phase = .recording
        startLocationUpdates()
    }

    func stopTapped() {
        guard phase == .recording else { return }
        phase = .promptingEndSoc
        locationManager.stopUpdatingLocation()
        locationManager.allowsBackgroundLocationUpdates = false
    }

    /// Cancelling the end-SoC prompt returns to recording and RESTARTS location updates --
    /// data-loss guard: without this, backing out of the prompt would silently truncate the
    /// trace at the stop tap instead of resuming capture.
    func cancelEndSoc() {
        guard phase == .promptingEndSoc else { return }
        phase = .recording
        startLocationUpdates()
    }

    /// Kicks an async save: ambient temperature at the trip's midpoint sample/time, then the
    /// tlog-1 JSON. A save with zero samples still saves -- an empty trace is visible evidence
    /// of a capture problem, not something to silently drop.
    ///
    /// The phase flips to `.idle` HERE, synchronously, not when the async save lands: RootView's
    /// alert `isPresented` binding calls `cancelEndSoc()` on every dismiss, OK included, and if
    /// the phase were still `.promptingEndSoc` at that moment the cancel guard would pass and
    /// restart location updates with nothing left to stop them.
    func confirmEndSoc(_ pct: Int) {
        guard phase == .promptingEndSoc, let tripStartDate else { return }
        phase = .idle
        let endSocPct = min(max(pct, 0), 100)
        let capturedSamples = samples
        let startUnix = Int(tripStartDate.timeIntervalSince1970)
        let endUnix = Int(Date.now.timeIntervalSince1970)
        let capturedStartSocPct = startSocPct

        Task {
            let ambientTempC = await midpointTemperature(samples: capturedSamples, startUnix: startUnix)
            let log = TripLog(
                format: "tlog-1", id: UUID().uuidString, vehicle: "ioniq5_lr_2wd",
                startUnix: startUnix, endUnix: endUnix,
                startSocPct: capturedStartSocPct, endSocPct: endSocPct,
                ambientTempC: ambientTempC, samples: capturedSamples
            )
            do {
                lastSavedURL = try TripLogStorage.save(log)
                saveErrorMessage = nil
                refreshLogs()
            } catch {
                saveErrorMessage = String(describing: error)
            }
        }
    }

    /// Internal (not private) so triplog-smoke can feed synthetic locations directly -- there's
    /// no way to inject CLLocationManager fixes on the simulator. Appends a sample only while
    /// `.recording`, dropping fixes timestamped before the trip start.
    func ingest(_ location: CLLocation) {
        guard phase == .recording, let tripStartDate else { return }
        let t = location.timestamp.timeIntervalSince(tripStartDate)
        guard t >= 0 else { return }

        samples.append(TripSample(
            t: t, lat: location.coordinate.latitude, lon: location.coordinate.longitude,
            speedMps: location.speed >= 0 ? location.speed : nil,
            altM: location.verticalAccuracy > 0 ? location.altitude : nil,
            haccM: location.horizontalAccuracy >= 0 ? location.horizontalAccuracy : nil
        ))

        if let lastKeptLocation {
            distanceM += lastKeptLocation.distance(from: location)
        }
        lastKeptLocation = location
        sampleCount = samples.count
    }

    func refreshLogs() {
        logs = TripLogStorage.list()
    }

    // MARK: CLLocationManagerDelegate

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        for location in locations {
            ingest(location)
        }
    }

    // MARK: Private

    private func midpointTemperature(samples: [TripSample], startUnix: Int) async -> Double? {
        guard !samples.isEmpty else { return nil }
        let midpoint = samples[samples.count / 2]
        return await fetchTemperature(midpoint.lat, midpoint.lon, startUnix + Int(midpoint.t))
    }

    private func startLocationUpdates() {
        locationManager.desiredAccuracy = kCLLocationAccuracyBest
        locationManager.distanceFilter = kCLDistanceFilterNone
        locationManager.activityType = .automotiveNavigation
        locationManager.pausesLocationUpdatesAutomatically = false
        // Setting allowsBackgroundLocationUpdates without the UIBackgroundModes "location"
        // entry (project.yml) crashes; guard further on authorization actually being granted,
        // since it's meaningless -- and best avoided -- before that.
        switch locationManager.authorizationStatus {
        case .authorizedWhenInUse, .authorizedAlways:
            locationManager.allowsBackgroundLocationUpdates = true
        default:
            break
        }
        locationManager.requestWhenInUseAuthorization()
        locationManager.startUpdatingLocation()
    }
}
