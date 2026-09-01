// Trip Log capture lifecycle (wayfinder #51), per ADR 0009: manual start/stop with a dash-SoC
// prompt at each end, ~1Hz GPS trace, ambient temperature at the trip midpoint, saved as a
// tlog-1 JSON. Owns its OWN CLLocationManager -- PlanStore's stays untouched, since the two
// serve different purposes (route-editor origin adoption vs. a continuous recording trace) and
// PlanStore's accuracy/background settings must not change for this.
import CoreLocation
import Foundation

// Swift 6 strict concurrency (M-05 -- docs/codebase-audit-2026-08-29.md): `@preconcurrency`
// conformance is sound here because CLLocationManager delivers callbacks on the runloop of the
// thread that started it -- main, since `locationManager` is a stored property initialized from
// this @MainActor class's `init`.
@MainActor
@Observable
final class TripLogStore: NSObject, @preconcurrency CLLocationManagerDelegate {
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
    /// Bumped on every failed save, even a repeated identical error, so RootView's
    /// onChange(saveErrorVersion) fires every time -- same reasoning as PlanStore's
    /// planErrorVersion.
    private(set) var saveErrorVersion = 0
    /// M-06 (docs/codebase-audit-2026-08-29.md): denied/restricted authorization at capture
    /// start, a mid-recording authorization revocation, or a persistent CLLocationManager
    /// failure.
    private(set) var captureErrorMessage: String?
    private(set) var captureErrorVersion = 0

    /// Injectable so triplog-smoke can stub a deterministic ambient temperature instead of
    /// depending on the network for a pass/fail result.
    var fetchTemperature: @Sendable (Double, Double, Int) async -> Double? = OpenMeteo.temperatureC
    /// Injectable so triplog-smoke can simulate denied/restricted authorization -- there's no
    /// way to drive real CLLocationManager authorization on the simulator. Defaults to reading
    /// the app-wide authorization status (a fresh CLLocationManager reads the same status as
    /// any other instance).
    var authorizationStatus: () -> CLAuthorizationStatus = { CLLocationManager().authorizationStatus }

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

    /// M-06: refuses to enter `.recording` when authorization is already denied/restricted --
    /// starting anyway would record nothing while looking like a normal capture. `.notDetermined`
    /// proceeds; the existing request + `locationManagerDidChangeAuthorization` flow below
    /// picks up the user's answer once it arrives.
    func confirmStartSoc(_ pct: Int) {
        guard phase == .promptingStartSoc else { return }
        switch authorizationStatus() {
        case .denied, .restricted:
            phase = .idle
            captureErrorMessage = "Location access denied — enable it in Settings to record trips"
            captureErrorVersion += 1
            return
        default:
            break
        }
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
            // Tag must agree with PlanStore.vehicle, or calibrate() excludes the log as a
            // vehicle mismatch (drive-smoke asserts the stamped tag).
            let log = TripLog(
                format: "tlog-1", id: UUID().uuidString, vehicle: "ioniq5_lr_awd",
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
                saveErrorVersion += 1
            }
        }
    }

    /// Internal (not private) so triplog-smoke can feed synthetic locations directly -- there's
    /// no way to inject CLLocationManager fixes on the simulator. Appends a sample only while
    /// `.recording`; this is the tlog-1 producer contract (M-06 --
    /// docs/codebase-audit-2026-08-29.md), consumed by #52's Rust `calibrate()`:
    /// - drop fixes timestamped before the trip start, and non-finite/invalid coordinates
    ///   (`CLLocationCoordinate2DIsValid`, plus exact (0, 0), which CoreLocation can report for
    ///   a genuinely failed fix even though it passes that validity check);
    /// - drop non-monotonic timestamps -- `t` must exceed the last KEPT sample's `t`, so an
    ///   out-of-order or exact-duplicate fix is dropped, not just a decreasing one;
    /// - thin to at most ~1 Hz by dropping a fix less than 0.5s after the last kept sample.
    /// Accuracy values themselves are kept as plain data -- the Rust fit filters on them, this
    /// producer doesn't.
    func ingest(_ location: CLLocation) {
        guard phase == .recording, let tripStartDate else { return }
        let t = location.timestamp.timeIntervalSince(tripStartDate)
        guard t >= 0 else { return }
        guard CLLocationCoordinate2DIsValid(location.coordinate),
              location.coordinate.latitude != 0 || location.coordinate.longitude != 0
        else { return }
        if let lastKeptT = samples.last?.t {
            guard t > lastKeptT else { return }
            guard t - lastKeptT >= 0.5 else { return }
        }

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

    /// Bulk delete for the Settings "Delete All Trip Logs" button (issue #56 / SEC-010):
    /// best-effort per file, same as the existing per-row Delete button's `try?
    /// TripLogStorage.delete`.
    func deleteAllLogs() {
        for url in TripLogStorage.list() {
            try? TripLogStorage.delete(url: url)
        }
        refreshLogs()
    }

    // MARK: CLLocationManagerDelegate

    func locationManager(_ manager: CLLocationManager, didUpdateLocations locations: [CLLocation]) {
        for location in locations {
            ingest(location)
        }
    }

    /// M-06: only matters while `.recording` -- `confirmStartSoc` already rejected an
    /// already-denied/restricted status before entering that phase, so this handles a change
    /// that happens mid-trip. A grant (e.g. the user answered the system prompt kicked off by
    /// `startLocationUpdates()`) resumes updates; a revocation stops recording -- nothing was
    /// captured while unauthorized, so there's no trace to preserve.
    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        guard phase == .recording else { return }
        switch manager.authorizationStatus {
        case .authorizedWhenInUse, .authorizedAlways:
            manager.startUpdatingLocation()
        case .denied, .restricted:
            phase = .idle
            captureErrorMessage = "Location access denied — enable it in Settings to record trips"
            captureErrorVersion += 1
            manager.stopUpdatingLocation()
            manager.allowsBackgroundLocationUpdates = false
        default:
            break
        }
    }

    /// M-06: surfaces a persistent CLLocationManager failure without leaving `.recording` --
    /// transient CL errors (e.g. a momentary `kCLErrorLocationUnknown`) are common and usually
    /// self-resolve, so dropping out of the capture phase on every one of them would abort
    /// trips over brief GPS loss instead of just gaps in the trace.
    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        captureErrorMessage = error.localizedDescription
        captureErrorVersion += 1
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
