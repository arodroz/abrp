// Owns the PlannerClient and the pack/planner load state (salvaged pattern
// from prototype/planner-ui's PlanStore, minus the map/plan-request plumbing
// that belongs to the map-surface ticket). RootView observes this to show
// placeholder status text.
import Foundation
import PlannerKit

@MainActor
@Observable
final class PlanStore {
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

    private(set) var packStatus: PackStatus = .missing
    private(set) var plannerStatus: PlannerStatus = .idle

    private var client: PlannerClient?
    private let region = "corridor"

    func load() {
        guard let located = Packs.locate(region: region) else {
            packStatus = .missing
            return
        }
        packStatus = .loaded(region: region)
        plannerStatus = .loading

        let rpackPath = located.rpackURL.path
        let chargersURL = located.chargersURL
        Task.detached { [weak self] in
            do {
                let client = try PlannerClient(regionPackPath: rpackPath)
                let bytes = try Data(contentsOf: chargersURL)
                try client.loadChargers(bytes: bytes, format: "cpack-1")
                await self?.didLoad(client: client)
            } catch {
                await self?.didFail(error: error)
            }
        }
    }

    private func didLoad(client: PlannerClient) {
        self.client = client
        plannerStatus = .ready
    }

    private func didFail(error: Error) {
        plannerStatus = .failed(String(describing: error))
    }
}
