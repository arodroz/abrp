// The 12V-safety brain (wayfinder #78, per the map's standing constraint -- see
// docs/research/ioniq5-obd-telemetry.md §6 and docs/research/obdlink-cx-ble.md §4 on the
// Ioniq 5's ICCU/12V recall history and OBDLink's own "app-held sessions defeat sleep" warning).
// Pure, no CoreBluetooth import -- same "enum namespace + State struct + pure static functions"
// shape as VoicePromptScheduler.swift, so this is directly unit-testable (obd-smoke drives it
// with synthetic `now` values, no real waiting) and TelemetryLinkStore is the only thing that
// turns its output Commands into real TelemetryLink calls, and the only thing that feeds it real
// inputs.
//
// The rules, verbatim from the ticket:
//  1. Connect only while the gate is open.
//  2. Close cleanly the moment the gate shuts.
//  3. N consecutive unanswered poll cycles while connected (CAN-quiet, car asleep) means
//     disconnect and back off -- never hammer reconnects.
//  4. NEVER initiate scanning while the gate is closed. A parked car is never scanned, full
//     stop -- checked first, unconditionally, in `tick` below; no other rule may override it.
import Foundation

enum TelemetryLinkPolicy {
    /// `gateOpen` is a single Bool for now: future tickets plumb this from Drive Mode being
    /// active OR a live/diagnostics surface being open (ORed into one Bool before it ever
    /// reaches this type). This type does not know or care why the gate is open.
    struct Input: Equatable {
        var gateOpen: Bool
    }

    enum Command: Equatable {
        case open
        case close
    }

    enum Phase: Equatable {
        case idle
        case opening
        case ready
        case backingOff(reason: BackoffReason, attempt: Int)
    }

    enum BackoffReason: Equatable {
        case canQuiet
        case linkFailure
    }

    struct State {
        var phase: Phase = .idle
        var unansweredCount = 0
        var consecutiveFailures = 0
        /// Time (caller's own clock -- see `tick`) backoff may next attempt `.open`. Only
        /// meaningful while `phase` is `.backingOff`.
        var backoffUntil: TimeInterval?

        init() {}
    }

    /// Consecutive unanswered poll cycles, while connected, before CAN-quiet trips.
    /// docs/research/obdlink-cx-ble.md §3 documents OBDLink's own per-command timeout (STPTO)
    /// defaulting to 102 ms with adaptive tuning -- far too fast a signal on its own to call "the
    /// car is asleep" (a single dropped frame isn't sleep). The research doc has no guidance at
    /// the app-polling-cadence level, so 3 consecutive fully-silent poll cycles is chosen as a
    /// defensible value: long enough that one hiccup doesn't trip it, short enough that a
    /// genuinely sleeping BMS is caught well inside the timescale of the 12V drain this feature
    /// exists to prevent.
    static let canQuietThreshold = 3
    /// Backoff delays after a CAN-quiet trip or a link failure, doubling per consecutive
    /// failure up to a ceiling -- standard exponential backoff, not research-doc-specified (the
    /// doc only covers the CX's own chip-level sleep/wake timers, a different layer from app
    /// reconnect cadence). Even at the floor this is far slower than a reconnect storm.
    static let backoffScheduleS: [TimeInterval] = [5, 15, 30, 60]

    /// Called whenever the gate or link state might have changed, or a scheduled backoff
    /// deadline might have passed. `now` is caller-supplied (a monotonic clock in production;
    /// any counter obd-smoke likes) so this stays real-time-free and deterministic to test.
    /// Returns the command to issue, if any -- `nil` means "nothing to do," not "close."
    static func tick(state: inout State, input: Input, linkState: TelemetryLinkState, now: TimeInterval) -> Command? {
        // Rules 2 + 4, unconditional, checked first: the moment the gate is closed, nothing
        // below this line may open anything, no matter what phase or backoff timer says.
        guard input.gateOpen else {
            let wasActive = state.phase != .idle
            state = State()
            return wasActive ? .close : nil
        }

        switch linkState {
        case .ready:
            state.phase = .ready
            state.unansweredCount = 0
            return nil

        case .idle:
            if case .backingOff = state.phase {
                guard let backoffUntil = state.backoffUntil, now >= backoffUntil else { return nil }
                state.phase = .opening
                state.backoffUntil = nil
                return .open
            }
            guard state.phase == .idle else { return nil }
            state.phase = .opening
            return .open

        case .scanning, .connecting:
            state.phase = .opening
            return nil

        case .backoff:
            // The link failed on its own (Bluetooth unavailable, scan timeout, connection
            // failure, or a disconnect -- expected or not). Treat like a CAN-quiet trip for
            // scheduling purposes: enter backoff, schedule the next attempt. `enterBackoff` is
            // idempotent if we're already backing off (e.g. the link reports `.backoff` again
            // while a schedule is already pending).
            return enterBackoff(state: &state, reason: .linkFailure, now: now)
        }
    }

    /// Called once per completed poll cycle while the link is `.ready` -- `answered` is
    /// TelemetryPump's own observation (did this cycle receive ANY bytes back at all), not a
    /// per-request detail. `canQuietThreshold` consecutive `false`s trips CAN-quiet: closes the
    /// link and starts backoff.
    static func recordPollResult(state: inout State, answered: Bool, now: TimeInterval) -> Command? {
        guard state.phase == .ready else { return nil }
        if answered {
            state.unansweredCount = 0
            return nil
        }
        state.unansweredCount += 1
        guard state.unansweredCount >= canQuietThreshold else { return nil }
        state.unansweredCount = 0
        return enterBackoff(state: &state, reason: .canQuiet, now: now)
    }

    /// NOTE (open question for a future tuning ticket): `consecutiveFailures` never resets on a
    /// recovered, sustained `.ready` period within a session -- a long drive with one early
    /// hiccup keeps climbing the schedule on any LATER failure, rather than starting back at the
    /// floor. Not asked for by the ticket; flagged rather than silently assumed away.
    private static func enterBackoff(state: inout State, reason: BackoffReason, now: TimeInterval) -> Command? {
        if case .backingOff = state.phase { return nil }
        state.consecutiveFailures += 1
        let index = min(state.consecutiveFailures - 1, backoffScheduleS.count - 1)
        state.backoffUntil = now + backoffScheduleS[index]
        state.phase = .backingOff(reason: reason, attempt: state.consecutiveFailures)
        return .close
    }
}
