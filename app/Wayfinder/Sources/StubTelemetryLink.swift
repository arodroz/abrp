// Deterministic scripted fake TelemetryLink for the simulator (wayfinder #78) -- CoreBluetooth
// reports `.unsupported` on the sim (docs/research/obdlink-cx-ble.md), so obd-smoke drives this
// instead of CxBleLink. Fully synchronous: `open()`/`close()`/`send(_:)` resolve before
// returning, so obd-smoke needs no real waits to observe a transition (it still receives every
// intermediate state via `onStateChange`, in order).
import Foundation

/// One scripted request/response pair, ELM-flavored (e.g. `ATZ\r` -> a banner + `>` prompt).
/// Shared with `ScriptedDialogue` (TelemetryDialogue.swift): obd-smoke builds ONE
/// `[ScriptedExchange]` and hands it to both fakes, so the dialogue's expected outgoing bytes
/// and the stub's expected-write/response bytes can never drift apart by hand.
struct ScriptedExchange: Equatable {
    let outgoing: Data
    let incoming: Data
}

final class StubTelemetryLink: TelemetryLink {
    private(set) var state: TelemetryLinkState = .idle {
        didSet { onStateChange?(state) }
    }
    var onStateChange: ((TelemetryLinkState) -> Void)?
    var onIncoming: ((Data) -> Void)?
    /// Configurable so write-chunking is observable (wayfinder #78 acceptance criterion 3).
    /// Default matches a realistic post-negotiation ATT payload (23-byte ATT MTU minus a 3-byte
    /// header) -- the same number CxBleLink would see on a phone that negotiated the minimum.
    var maxWriteLength = 20

    /// The script to replay, set before `open()`. Consumed in order as `send` calls arrive.
    var script: [ScriptedExchange] = []
    /// Every chunk `send` produced, in call order -- obd-smoke's write-chunking assertion reads
    /// this directly.
    private(set) var sentChunks: [Data] = []
    /// Total `open()`/`close()` calls that actually did something (past the idempotency guard)
    /// -- obd-smoke's "zero scan attempts while the gate is closed" and "exactly one reconnect"
    /// assertions read these directly rather than inferring counts from `state` alone.
    private(set) var openCallCount = 0
    private(set) var closeCallCount = 0

    private var exchangeIndex = 0
    private var receivedForCurrentExchange = Data()
    /// Set by `simulateSilence()`: the link stays `.ready` (still "connected") but stops
    /// delivering responses -- CAN-quiet, the BMS going quiet while the transport itself is
    /// fine, as opposed to `simulateUnexpectedDisconnect()`.
    private var silenced = false

    func open() {
        guard state == .idle else { return }
        openCallCount += 1
        state = .scanning
        state = .connecting
        state = .ready
    }

    func close() {
        guard state != .idle else { return }
        closeCallCount += 1
        state = .idle
        exchangeIndex = 0
        receivedForCurrentExchange = Data()
        silenced = false
    }

    func send(_ data: Data) {
        guard state == .ready else { return }
        for chunk in TelemetryChunking.chunks(data, maxLength: maxWriteLength) {
            sentChunks.append(chunk)
            receivedForCurrentExchange.append(chunk)
        }
        guard exchangeIndex < script.count else { return }
        let exchange = script[exchangeIndex]
        guard receivedForCurrentExchange == exchange.outgoing else { return }
        receivedForCurrentExchange = Data()
        exchangeIndex += 1
        guard !silenced else { return }
        onIncoming?(exchange.incoming)
    }

    // MARK: Test controls

    /// Simulates the peripheral dropping the connection without a clean `close()` -- the
    /// scenario TelemetryLinkPolicy's "one reconnect attempt" rule (wayfinder #78) reacts to.
    func simulateUnexpectedDisconnect() {
        guard state != .idle else { return }
        state = .backoff(reason: .disconnected)
    }

    /// CAN-quiet: subsequent `send` calls still match/advance the script bookkeeping (so a
    /// resumed, non-silenced run stays in sync) but never fire `onIncoming` -- mirrors a sleeping
    /// BMS not answering while the BLE link itself stays up.
    func simulateSilence() {
        silenced = true
    }
}
