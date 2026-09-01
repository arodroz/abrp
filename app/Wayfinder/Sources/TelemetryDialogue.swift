// A Swift protocol mirroring core/telemetry's sans-IO engine shape (wayfinder #78; see
// core/telemetry/src/dialogue.rs's `Engine`): bytes in, bytes/completion out -- the caller (here,
// TelemetryPump) owns the transport and the read timeout, exactly like the Rust engine's own
// doc comment describes. The real FFI-backed adapter wrapping the actual Rust `Engine` arrives in
// a later ticket (the parallel FFI lane hasn't shipped it yet); this protocol is the one seam
// left for it -- nothing else here should need to change when it lands.
import Foundation

protocol TelemetryDialogue: AnyObject {
    /// Next bytes to write, or nil if nothing is ready right now (waiting on a response to the
    /// last write, or finished). Mirrors `Engine.take_outgoing`.
    func takeOutgoing() -> Data?
    /// Bytes read from the transport. Mirrors `Engine.feed`.
    func feed(_ data: Data)
    /// The transport's read timeout expired with no (or an incomplete) response. Mirrors
    /// `Engine.on_timeout`.
    func onTimeout()
    /// All requests have concluded (or the dialogue aborted). Mirrors `Engine.is_finished`.
    var isFinished: Bool { get }
}

/// Deterministic TelemetryDialogue for obd-smoke: replays a fixed `[ScriptedExchange]` list
/// (StubTelemetryLink.swift) byte-for-byte. `takeOutgoing()` hands back each expected outgoing
/// whole -- chunking is TelemetryLink's job, not the dialogue's, see that protocol's header.
/// `feed(_:)` accumulates response bytes and advances once a full expected response has
/// arrived; `onTimeout()` marks the run permanently failed rather than hanging forever, mirroring
/// the Rust engine's own "mid-response timeout concludes as Failed" behavior
/// (core/telemetry/src/dialogue.rs's `on_timeout`).
final class ScriptedDialogue: TelemetryDialogue {
    private var remaining: [ScriptedExchange]
    private var awaitingResponseFor: ScriptedExchange?
    private var receivedSoFar = Data()
    private(set) var timedOut = false

    init(script: [ScriptedExchange]) {
        remaining = script
    }

    var isFinished: Bool { timedOut || (remaining.isEmpty && awaitingResponseFor == nil) }

    func takeOutgoing() -> Data? {
        guard !timedOut, awaitingResponseFor == nil, !remaining.isEmpty else { return nil }
        let next = remaining.removeFirst()
        awaitingResponseFor = next
        receivedSoFar = Data()
        return next.outgoing
    }

    func feed(_ data: Data) {
        guard let exchange = awaitingResponseFor, !timedOut else { return }
        receivedSoFar.append(data)
        guard receivedSoFar == exchange.incoming else { return }
        awaitingResponseFor = nil
        receivedSoFar = Data()
    }

    func onTimeout() {
        guard awaitingResponseFor != nil else { return }
        timedOut = true
    }
}
