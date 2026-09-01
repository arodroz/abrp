// Owns policy + link + pump (wayfinder #78), matching TripLogStore/PlanStore's
// `@MainActor @Observable final class` idiom: private(set) state, injected link (CxBleLink in
// production, StubTelemetryLink in obd-smoke -- CBCentralManager reports `.unsupported` on the
// simulator, see TelemetryLink.swift). Exposes phase/state/counters for the drive HUD/CarPlay
// live surface (wayfinder #79). Logs EVERY lifecycle transition via os_log (subsystem
// "org.anteras.wayfinder", category "telemetry") -- an explicit acceptance criterion for this
// ticket, independent of any UI: `log show`/Console.app must be able to reconstruct what the
// transport did without a screen attached.
//
// Live readings (wayfinder #79): `makeDialogue` builds a fresh `TelemetryReadingDialogue` (one
// full poll sweep, per-command freq scheduling ignored in v1) each cycle; the poll loop below
// runs it, drains canonical readings into `latestReadings`, sleeps ~1s, and repeats for as long as
// `phase == .ready` and the gate stays open. `latestReadings` is keyed generically on
// `FfiCanonicalSignal` -- ticket #80 will consume more signals than `.displaySoc`.
import Foundation
import os
import PlannerKit

@MainActor
@Observable
final class TelemetryLinkStore {
    private static let log = Logger(subsystem: "org.anteras.wayfinder", category: "telemetry")

    private(set) var phase: TelemetryLinkPolicy.Phase = .idle
    private(set) var linkState: TelemetryLinkState = .idle
    private(set) var incomingChunkCount = 0
    private(set) var incomingByteCount = 0

    /// Live decoded readings (wayfinder #79), keyed by canonical signal -- only readings with a
    /// non-nil `canonicalSignal`/`value` ever land here. `lastReadingAt` advances only on a
    /// sweep that landed at least one such reading, so a sweep that comes back all-NO-DATA (CAN
    /// briefly busy) doesn't itself trip staleness.
    private(set) var latestReadings: [FfiCanonicalSignal: Double] = [:]
    private(set) var lastReadingAt: Date?
    var liveDisplaySoc: Double? { latestReadings[.displaySoc] }

    /// How stale a reading may be and still be trustworthy to WRITE somewhere (wayfinder #80):
    /// the Trip Log telemetry snapshot and the SoC-prompt auto-fill both gate on this window via
    /// `lastReadingAt`. Distinct from `LiveSocPresentation`'s 10s staleness badge, which is a
    /// display concern, not a "is this worth recording" one.
    static let snapshotFreshnessS: TimeInterval = 30

    /// Builds one fresh `TelemetryReadingDialogue` per poll cycle -- `nil` (no dialogue built,
    /// e.g. the bundled profile failed to load) just skips that cycle. Set once at construction
    /// in production (WayfinderApp); obd-smoke/live-soc-smoke set it directly.
    var makeDialogue: (() -> TelemetryReadingDialogue?)?

    /// The 12V-safety gate (wayfinder #78 standing constraint -- see TelemetryLinkPolicy's
    /// header): true while Drive Mode is active OR a live/diagnostics surface is open, ORed into
    /// this one Bool by whatever future ticket wires those real signals in. Defaults closed: a
    /// freshly constructed store never scans until something explicitly opens the gate.
    var gateOpen = false {
        didSet {
            guard oldValue != gateOpen else { return }
            Self.log.log("gate \(self.gateOpen ? "open" : "closed", privacy: .public)")
            if !gateOpen {
                latestReadings = [:]
                lastReadingAt = nil
            }
            retick()
        }
    }

    /// Test/demo seam (wayfinder #83): sets a synthetic Display SoC reading directly, bypassing
    /// the real link/poll loop entirely -- for `soc-chart-smoke`'s trail-thinning assertions and
    /// `chart-demo-drive`'s visual trail, neither of which has real BLE hardware to script a full
    /// engine dialogue for (see AutotestSocChart.swift's header for why this is preferred over
    /// rebuilding live-soc-smoke's scripted engine dialogue just for this).
    func setSyntheticDisplaySoc(_ pct: Double, at date: Date = Date()) {
        latestReadings[.displaySoc] = pct
        lastReadingAt = date
    }

    private let link: TelemetryLink
    private var policyState = TelemetryLinkPolicy.State()
    private var backoffTask: Task<Void, Never>?
    /// The live-readings poll loop (wayfinder #79) -- exactly one at a time, started when `phase`
    /// becomes `.ready` and the gate is open, cancelled/replaced the same way `backoffTask` is.
    private var pollTask: Task<Void, Never>?

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
        updatePollLoop()
    }

    /// Starts the live-readings poll loop the moment `phase` is `.ready` and the gate is open;
    /// stops it (best-effort -- an already in-flight sweep runs to completion, same as
    /// `backoffTask`'s own cancellation idiom) the moment either stops being true.
    private func updatePollLoop() {
        let shouldRun = phase == .ready && gateOpen
        if shouldRun, pollTask == nil {
            pollTask = Task { @MainActor [weak self] in await self?.runPollLoop() }
        } else if !shouldRun, pollTask != nil {
            pollTask?.cancel()
            pollTask = nil
        }
    }

    /// One full poll sweep per iteration (wayfinder #79): a fresh dialogue (one `TelemetrySession`
    /// = one sweep over the whole profile) run to completion via `runOnePoll`, its readings
    /// drained into `latestReadings`, then ~1s of slack before the next sweep. `runOnePoll`
    /// itself no-ops once `phase` leaves `.ready`, so this loop's own `while` condition is the
    /// only thing that needs to stop it.
    private func runPollLoop() async {
        while phase == .ready, gateOpen, !Task.isCancelled {
            guard let dialogue = makeDialogue?() else { break }
            await runOnePoll(dialogue: dialogue)
            let readings = dialogue.drainReadings()
            var landedAny = false
            for reading in readings {
                guard let signal = reading.canonicalSignal, let value = reading.value else { continue }
                latestReadings[signal] = value
                landedAny = true
            }
            if landedAny { lastReadingAt = Date() }
            Self.log.debug("poll sweep complete, \(readings.count, privacy: .public) readings")
            guard !Task.isCancelled else { break }
            try? await Task.sleep(nanoseconds: 1_000_000_000)
        }
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
