import Foundation

/// Swift-side wrapper around the generated `Planner` object (ADR 0004
/// point 4): synchronous Rust invoked from a background `Task.detached`,
/// results published back to the caller; `cancel()` stays synchronous so it
/// can be called immediately from the main actor without waiting on the
/// in-flight `plan()` call. `Planner` is already `@unchecked Sendable`
/// (generated); wrapping it in a `Sendable` `final class` here is what lets
/// the rest of the app hold one instance across actor boundaries under
/// Swift 6 strict concurrency without re-litigating that at every call site.
public final class PlannerClient: Sendable {
    private let inner: Planner

    public init(regionPackPath: String) throws {
        inner = try Planner(regionPackPath: regionPackPath)
    }

    @discardableResult
    public func loadChargers(bytes: Data, format: String) throws -> UInt32 {
        try inner.loadChargers(bytes: bytes, format: format)
    }

    public func plan(_ request: FfiPlanRequest) async throws -> FfiPlan {
        try await Task.detached { [inner] in
            try inner.plan(request: request)
        }.value
    }

    public func cancel() {
        inner.cancel()
    }

    public func energy(_ input: FfiLegInput) -> Double {
        inner.energy(input: input)
    }

    public func calibrate() throws {
        try inner.calibrate()
    }
}
