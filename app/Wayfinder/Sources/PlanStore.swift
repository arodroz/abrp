// Owns the MLNMapView instance, the PlannerClient, and the pack/planner load state --
// salvaged from prototype/planner-ui's PlanStore (map plumbing: style loading with the
// PMTILES placeholder patch, the Chargers layer, route/stops layers, light/dark style swap,
// camera fit). Skipped for this ticket (map-surface #42), left for later tickets: the
// SoC-scrub marker sync (arrival-card ticket), debounced replan + origin/destination flow
// (editor ticket), and CoreLocation bootstrap (editor ticket) -- there is no search UI yet,
// so `runPlan` is only driven by the map-smoke autotest, not by RootView.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

@MainActor
@Observable
final class PlanStore: NSObject, MLNMapViewDelegate {
    enum PackStatus: Equatable {
        case missing
        case loaded(region: String)
    }

    enum PlannerStatus: Equatable {
        case idle
        case loading
        case ready
        case failed(String)
    }

    enum PlanStoreError: Error {
        case plannerNotReady
    }

    let mapView = MLNMapView(frame: .zero)

    private(set) var packStatus: PackStatus = .missing
    private(set) var plannerStatus: PlannerStatus = .idle
    private(set) var chargerCount = 0
    private(set) var isStyleLoaded = false
    private(set) var plan: FfiPlan?

    private var client: PlannerClient?
    private var located: Packs.Located?
    private var chargersForMap: [CPack1Charger]?
    private var isDarkAppearance = false
    private var hasStartedLoad = false
    private var hasSetInitialCamera = false

    private let region = "corridor"

    override init() {
        super.init()
        mapView.delegate = self
    }

    func load() {
        guard !hasStartedLoad else { return }
        hasStartedLoad = true

        guard let located = Packs.locate(region: region) else {
            packStatus = .missing
            return
        }
        self.located = located
        packStatus = .loaded(region: region)
        plannerStatus = .loading
        applyStyle(located: located)

        let rpackPath = located.rpackURL.path
        let chargersURL = located.chargersURL
        Task.detached { [weak self] in
            do {
                let client = try PlannerClient(regionPackPath: rpackPath)
                let bytes = try Data(contentsOf: chargersURL)
                try client.loadChargers(bytes: bytes, format: "cpack-1")
                let chargers = try CPack1.parseChargers(data: bytes)
                await self?.didLoad(client: client, chargers: chargers)
            } catch {
                await self?.didFail(error: error)
            }
        }
    }

    private func didLoad(client: PlannerClient, chargers: [CPack1Charger]) {
        self.client = client
        chargersForMap = chargers
        chargerCount = chargers.count
        plannerStatus = .ready
        addChargersLayerIfPossible()
    }

    private func didFail(error: Error) {
        plannerStatus = .failed(String(describing: error))
    }

    // MARK: Appearance (light/dark style)

    /// Called from RootView on appear and whenever the SwiftUI color scheme changes. Swaps
    /// the map style only when the appearance actually changed; `didFinishLoading` re-adds
    /// the chargers/route/stops layers once the new style finishes loading.
    func setAppearance(dark: Bool) {
        guard dark != isDarkAppearance else { return }
        isDarkAppearance = dark
        guard let located else { return } // load() hasn't run yet -- it applies isDarkAppearance itself
        applyStyle(located: located)
    }

    private func applyStyle(located: Packs.Located) {
        let styleFile = isDarkAppearance ? located.styleDarkURL : located.styleLightURL
        guard let styleURL = MapStyle.patchedStyleURL(pmtilesURL: located.pmtilesURL, styleURL: styleFile) else {
            return
        }
        mapView.styleURL = styleURL
    }

    // MARK: Planning

    /// Runs a plan request and draws the route + Charging Stop layers. There is no search UI
    /// yet (later ticket), so RootView doesn't call this -- only the map-smoke autotest does,
    /// to prove the map layers against the golden LU -> Amsterdam plan.
    @discardableResult
    func runPlan(_ request: FfiPlanRequest) async throws -> FfiPlan {
        guard let client else { throw PlanStoreError.plannerNotReady }
        let plan = try await client.plan(request)
        self.plan = plan
        if let style = mapView.style {
            RouteLayer.addLayers(to: style, plan: plan)
            RouteLayer.fitToRoute(mapView: mapView, plan: plan)
        }
        return plan
    }

    // MARK: MLNMapViewDelegate

    func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
        isStyleLoaded = true
        addChargersLayerIfPossible()
        if let plan {
            RouteLayer.addLayers(to: style, plan: plan)
        }
        setInitialCameraIfNeeded()
    }

    /// With no search UI yet (later ticket), there's no route to fit the camera to on plain
    /// launch -- center on the pack's Luxembourg corridor at a zoom that shows charger dots
    /// (ChargersLayer's minimumZoomLevel is 8) instead of leaving the default world view.
    private func setInitialCameraIfNeeded() {
        guard !hasSetInitialCamera else { return }
        hasSetInitialCamera = true
        let luxembourg = CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319)
        mapView.setCenter(luxembourg, zoomLevel: 10, animated: false)
    }

    private func addChargersLayerIfPossible() {
        guard let style = mapView.style, let chargersForMap else { return }
        guard style.source(withIdentifier: ChargersLayer.sourceId) == nil else { return }
        guard let source = ChargersLayer.makeSource(chargers: chargersForMap) else { return }
        ChargersLayer.addLayers(to: style, source: source)
    }
}
