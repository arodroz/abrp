// Owns policy + link + pump (wayfinder #78), matching TripLogStore/PlanStore's
// `@MainActor @Observable final class` idiom: private(set) state, injected link (CxBleLink in
// production, StubTelemetryLink in obd-smoke -- CBCentralManager reports `.unsupported` on the
// simulator, see TelemetryLink.swift). Exposes phase/state/counters for the future live surface
// a later ticket adds. Logs EVERY lifecycle transition via os_log (subsystem
// "org.anteras.wayfinder", category "telemetry") -- an explicit acceptance criterion for this
// ticket, independent of any UI: `log show`/Console.app must be able to reconstruct what the
// transport did without a screen attached.
import Foundation
import os

@MainActor
@Observable
final class TelemetryLinkStore {
    private static let log = Logger(subsystem: "org.anteras.wayfinder", category: "telemetry")

    private(set) var phase: TelemetryLinkPolicy.Phase = .idle
    private(set) var linkState: TelemetryLinkState = .idle
    private(set) var incomingChunkCount = 0
    private(set) var incomingByteCount = 0

    /// The 12V-safety gate (wayfinder #78 standing constraint -- see TelemetryLinkPolicy's
    /// header): true while Drive Mode is active OR a live/diagnostics surface is open, ORed into
    /// this one Bool by whatever future ticket wires those real signals in. Defaults closed: a
    /// freshly constructed store never scans until something explicitly opens the gate.
    var gateOpen = false {
        didSet {
            guard oldValue != gateOpen else { return }
            Self.log.log("gate \(self.gateOpen ? "open" : "closed", privacy: .public)")
            retick()
        }
    }

    private let link: TelemetryLink
    private var policyState = TelemetryLinkPolicy.State()
    private var backoffTask: Task<Void, Never>?

    init(link: TelemetryLink) {
        self.link = link
        linkState = link.state
        link.onStateChange = { [weak self] newState in
            Task { @MainActor in self?.handleLinkStateChange(newState) }
        }
        link.onIncoming = { [weak self] data in
            Task { @MainActor in self?.recordIncoming(data) }
        }
    }

    /// Runs one dialogue to completion over the current link via TelemetryPump, then reports its
    /// answered/unanswered outcome to the policy (CAN-quiet accounting, TelemetryLinkPolicy
    /// .recordPollResult). The seam a future ticket's real polling loop (an FFI-backed
    /// TelemetryDialogue, run repeatedly while `.ready`) will call; today only obd-smoke calls
    /// it, with ScriptedDialogue. A no-op while not `.ready` -- there is nothing to poll (and,
    /// per the gate rule, nothing should be connected) otherwise.
    func runOnePoll(dialogue: TelemetryDialogue, stepTimeoutS: TimeInterval = 2.0) async {
        guard phase == .ready else { return }
        let answered = await TelemetryPump.run(dialogue: dialogue, link: link, stepTimeoutS: stepTimeoutS)
        Self.log.debug("poll cycle answered=\(answered, privacy: .public)")
        let now = Date().timeIntervalSinceReferenceDate
        let previousPhase = policyState.phase
        let command = TelemetryLinkPolicy.recordPollResult(state: &policyState, answered: answered, now: now)
        finishTick(command: command, previousPhase: previousPhase, now: now)
    }

    private func handleLinkStateChange(_ newState: TelemetryLinkState) {
        linkState = newState
        if case .backoff(let reason) = newState {
            Self.log.error("link stopped: \(String(describing: reason), privacy: .public)")
        } else {
            Self.log.log("link state -> \(String(describing: newState), privacy: .public)")
        }
        retick()
    }

    private func recordIncoming(_ data: Data) {
        incomingChunkCount += 1
        incomingByteCount += data.count
        Self.log.debug("chunk #\(self.incomingChunkCount, privacy: .public) (\(data.count, privacy: .public) bytes)")
    }

    private func retick() {
        let now = Date().timeIntervalSinceReferenceDate
        let previousPhase = policyState.phase
        let input = TelemetryLinkPolicy.Input(gateOpen: gateOpen)
        let command = TelemetryLinkPolicy.tick(state: &policyState, input: input, linkState: linkState, now: now)
        finishTick(command: command, previousPhase: previousPhase, now: now)
    }

    private func finishTick(command: TelemetryLinkPolicy.Command?, previousPhase: TelemetryLinkPolicy.Phase, now: TimeInterval) {
        phase = policyState.phase
        logPhaseTransitionIfNeeded(from: previousPhase, to: policyState.phase)
        apply(command)
        scheduleBackoffWakeIfNeeded(now: now)
    }

    private func apply(_ command: TelemetryLinkPolicy.Command?) {
        switch command {
        case .open:
            Self.log.log("scan start")
            link.open()
        case .close:
            Self.log.log("disconnect requested")
            link.close()
        case nil:
            break
        }
    }

    private func logPhaseTransitionIfNeeded(from previous: TelemetryLinkPolicy.Phase, to current: TelemetryLinkPolicy.Phase) {
        guard previous != current else { return }
        switch current {
        case .backingOff(let reason, let attempt):
            switch reason {
            case .canQuiet:
                Self.log.error("CAN-quiet detected (car asleep) -- disconnecting, backoff attempt \(attempt, privacy: .public)")
            case .linkFailure:
                Self.log.log("backoff entered after link failure, attempt \(attempt, privacy: .public)")
            }
        case .opening:
            if case .backingOff = previous {
                Self.log.log("backoff exit -- retry attempt starting")
            } else {
                Self.log.log("opening (gate open)")
            }
        case .ready:
            Self.log.log("ready")
        case .idle:
            Self.log.log("idle")
        }
    }

    /// Schedules exactly one wake for the policy's own `backoffUntil` deadline rather than
    /// polling on a repeating timer -- cancels any previous wake first, so a superseded backoff
    /// (gate closed mid-wait, or a fresh failure re-entering backoff) can never fire a stale
    /// retry.
    private func scheduleBackoffWakeIfNeeded(now: TimeInterval) {
        backoffTask?.cancel()
        backoffTask = nil
        guard case .backingOff = policyState.phase, let until = policyState.backoffUntil else { return }
        let delay = max(0, until - now)
        backoffTask = Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard !Task.isCancelled else { return }
            self?.retick()
        }
    }
}
