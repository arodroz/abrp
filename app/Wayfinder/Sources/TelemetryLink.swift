// The byte-pipe + lifecycle abstraction between the telemetry pump/policy and a transport
// (wayfinder #78). Exists because CBCentralManager reports `.unsupported` on the simulator --
// `CxBleLink` (real CoreBluetooth, device-only) and `StubTelemetryLink` (deterministic,
// sim-only) share this one contract so TelemetryPump, TelemetryLinkPolicy, and
// TelemetryLinkStore never touch CoreBluetooth directly. Reconnection policy is NOT here -- see
// TelemetryLinkPolicy; a link only does what it's told (open/close/send) and reports what
// happened to it.
//
// Callback shape: plain closures (`onStateChange`/`onIncoming`), matching this codebase's
// existing cross-object notification idiom (PlanStore.onUserMapGesture,
// PackInstaller.onDeepVerifyWillStart/onInstallDidEnd) rather than a delegate protocol or
// AsyncStream -- see TelemetryPump's header for how it bridges these callbacks into its own
// async drive loop without requiring either shape at this protocol's public seam.
import Foundation

protocol TelemetryLink: AnyObject {
    /// Current lifecycle state; also delivered via `onStateChange` on every transition. A
    /// caller that sets `onStateChange` after construction should read `state` once itself too,
    /// to avoid missing whatever transition already happened.
    var state: TelemetryLinkState { get }
    /// Fires on every state transition. Queue is unspecified: CxBleLink fires on its private CB
    /// dispatch queue; StubTelemetryLink fires synchronously on the caller's thread. Only
    /// TelemetryLinkStore (the @MainActor boundary) may assume no particular queue -- it hops
    /// itself via `Task { @MainActor in ... }`.
    var onStateChange: ((TelemetryLinkState) -> Void)? { get set }
    /// Fires once per received notify chunk, in arrival order. Same queue caveat as
    /// `onStateChange`. A new subscriber REPLACES the previous one -- TelemetryPump chains onto
    /// whatever was already installed (see its header) rather than assuming it's the only
    /// subscriber.
    var onIncoming: ((Data) -> Void)? { get set }
    /// Max bytes per single write. Known precisely once `.ready` (CxBleLink:
    /// `peripheral.maximumWriteValueLength(for:)`; StubTelemetryLink: its configured stub MTU);
    /// a conservative default beforehand. Callers chunk outgoing writes to this via
    /// `TelemetryChunking` -- see `send(_:)`.
    var maxWriteLength: Int { get }

    /// Starts scanning/connecting. A no-op while already scanning/connecting/ready. NEVER call
    /// this while the 12V-safety gate is closed -- see TelemetryLinkPolicy's header; this
    /// protocol has no way to enforce that itself, since a link doesn't know about the gate.
    func open()
    /// Tears down cleanly (unsubscribe, cancel connection) and returns to `.idle`. A no-op
    /// while already idle.
    func close()
    /// Writes bytes, chunked internally to `maxWriteLength`. Only meaningful while `.ready`;
    /// implementations drop the write otherwise (TelemetryPump, the only caller, only sends
    /// while driving a dialogue against an already-`.ready` link).
    func send(_ data: Data)
}

enum TelemetryLinkState: Equatable {
    case idle
    case scanning
    case connecting
    case ready
    /// The link stopped itself over a transport-level problem (Bluetooth unavailable, a scan
    /// that never found the CX, a failed connection attempt, or a disconnect -- expected or
    /// not) and is waiting for the next explicit `open()`; a link NEVER retries on its own. This
    /// is distinct from TelemetryLinkPolicy's own CAN-quiet backoff schedule (a higher-level,
    /// timed decision the policy makes about WHEN to call `open()` again) -- see that type's
    /// header.
    case backoff(reason: TelemetryLinkFailureReason)
}

enum TelemetryLinkFailureReason: Equatable {
    /// `CBManagerState` was/became poweredOff, unauthorized, resetting, or unsupported -- the
    /// simulator always reports unsupported (docs/research/obdlink-cx-ble.md's premise for why
    /// this protocol exists at all).
    case bluetoothUnavailable
    case scanTimedOut
    case connectionFailed
    /// The peripheral disconnected, expectedly (a clean `close()` still reports `.idle`, not
    /// this -- see CxBleLink) or not.
    case disconnected
}

/// Splits an outgoing write into pieces no longer than `maxLength` -- shared by CxBleLink
/// (chunked to the CoreBluetooth-negotiated `maximumWriteValueLength`, per
/// docs/research/obdlink-cx-ble.md §1's "Chunking and reassembly") and StubTelemetryLink
/// (chunked to its configurable stub MTU), so both transports chunk identically and obd-smoke's
/// chunk-sequence assertion exercises the same logic real hardware would use.
enum TelemetryChunking {
    static func chunks(_ data: Data, maxLength: Int) -> [Data] {
        guard !data.isEmpty else { return [] }
        guard maxLength > 0 else { return [data] }
        var result: [Data] = []
        var offset = data.startIndex
        while offset < data.endIndex {
            let end = data.index(offset, offsetBy: maxLength, limitedBy: data.endIndex) ?? data.endIndex
            result.append(data[offset..<end])
            offset = end
        }
        return result
    }
}
