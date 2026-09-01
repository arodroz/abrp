// The CoreBluetooth transport (wayfinder #78) -- see docs/research/obdlink-cx-ble.md for every
// protocol fact this class codes against. Runs its CBCentralManager/CBPeripheral delegate
// callbacks on its own dedicated serial dispatch queue, never main; TelemetryLinkStore is the
// only thing that hops to the main actor, at the store boundary (TelemetryLink.swift's header).
// Reconnection is NOT this class's job -- see TelemetryLinkPolicy; every failure path here
// reports `.backoff` and stops, never re-scanning on its own (`wantsToOpen` is cleared the
// moment `.backoff` is reported, specifically so a Bluetooth power-cycle while parked can't
// silently resume scanning behind the policy's back).
//
// Can never actually connect on the simulator -- CBCentralManager reports `.unsupported` there,
// which is exactly why StubTelemetryLink exists for obd-smoke. This class degrades to
// `.backoff(.bluetoothUnavailable)` in that case rather than crashing, so it's still safe to
// construct and drive on the sim; real-hardware proof is the separate driveway-smoke ticket.
import CoreBluetooth
import Foundation

final class CxBleLink: NSObject, TelemetryLink {
    /// docs/research/obdlink-cx-ble.md §1: custom serial/UART service `0xFFF0`, independently
    /// confirmed in five shipped codebases across four languages/platforms. Computed rather than
    /// a stored `static let`: `CBUUID` isn't `Sendable`, so a cached global instance isn't
    /// concurrency-safe under Swift 6 -- a fresh, cheap-to-parse value each access avoids that
    /// without an `nonisolated(unsafe)` escape hatch.
    private static var serviceUUID: CBUUID { CBUUID(string: "FFF0") }
    private static var notifyCharacteristicUUID: CBUUID { CBUUID(string: "FFF1") }
    private static var writeCharacteristicUUID: CBUUID { CBUUID(string: "FFF2") }
    /// No BLE scan has a built-in timeout. docs/research/obdlink-cx-ble.md §6's single-client-
    /// only gotcha means a scan that never finds the CX (already connected to another app, out
    /// of range, powered off) should give up rather than run forever.
    private static let scanTimeoutS: TimeInterval = 15

    private(set) var state: TelemetryLinkState = .idle {
        didSet {
            // Never auto-resume on our own -- see this file's header. Only an explicit `open()`
            // may set `wantsToOpen` again.
            if case .backoff = state { wantsToOpen = false }
            onStateChange?(state)
        }
    }
    var onStateChange: ((TelemetryLinkState) -> Void)?
    var onIncoming: ((Data) -> Void)?
    /// A conservative default until `.ready`, when the real negotiated value replaces it -- see
    /// `peripheral(_:didUpdateNotificationStateFor:error:)`.
    private(set) var maxWriteLength = 20

    private let queue = DispatchQueue(label: "org.anteras.wayfinder.cxble")
    private lazy var central = CBCentralManager(delegate: self, queue: queue)
    private var peripheral: CBPeripheral?
    private var writeCharacteristic: CBCharacteristic?
    private var notifyCharacteristic: CBCharacteristic?
    private var wantsToOpen = false
    private var isClosingIntentionally = false
    private var scanTimeoutWorkItem: DispatchWorkItem?
    private var outgoingQueue: [Data] = []
    private var awaitingWriteCompletion = false

    // MARK: TelemetryLink

    func open() {
        queue.async { self.openOnQueue() }
    }

    func close() {
        queue.async { self.closeOnQueue() }
    }

    func send(_ data: Data) {
        queue.async {
            self.outgoingQueue.append(contentsOf: TelemetryChunking.chunks(data, maxLength: self.maxWriteLength))
            self.flushOutgoingIfPossible()
        }
    }

    // MARK: Queue-confined implementation (everything below only ever runs on `queue`)

    private func openOnQueue() {
        guard state == .idle || isBackoffState(state) else { return }
        wantsToOpen = true
        switch central.state {
        case .poweredOn:
            beginScan()
        case .unsupported, .unauthorized, .poweredOff, .resetting:
            state = .backoff(reason: .bluetoothUnavailable)
        case .unknown:
            break // centralManagerDidUpdateState resolves this shortly.
        @unknown default:
            state = .backoff(reason: .bluetoothUnavailable)
        }
    }

    private func beginScan() {
        guard wantsToOpen else { return }
        state = .scanning
        central.scanForPeripherals(withServices: [Self.serviceUUID])
        let workItem = DispatchWorkItem { [weak self] in self?.scanTimedOut() }
        scanTimeoutWorkItem = workItem
        queue.asyncAfter(deadline: .now() + Self.scanTimeoutS, execute: workItem)
    }

    private func scanTimedOut() {
        guard state == .scanning else { return }
        central.stopScan()
        state = .backoff(reason: .scanTimedOut)
    }

    private func closeOnQueue() {
        guard state != .idle else { return }
        isClosingIntentionally = true
        scanTimeoutWorkItem?.cancel()
        scanTimeoutWorkItem = nil
        wantsToOpen = false
        outgoingQueue = []
        awaitingWriteCompletion = false
        if state == .scanning {
            central.stopScan()
        }
        if let notifyCharacteristic, let peripheral, peripheral.state == .connected {
            peripheral.setNotifyValue(false, for: notifyCharacteristic)
        }
        if let peripheral {
            central.cancelPeripheralConnection(peripheral)
        }
        self.peripheral = nil
        writeCharacteristic = nil
        notifyCharacteristic = nil
        state = .idle
    }

    private func flushOutgoingIfPossible() {
        guard state == .ready, !awaitingWriteCompletion, !outgoingQueue.isEmpty,
              let peripheral, let writeCharacteristic
        else { return }
        // docs/research/obdlink-cx-ble.md §1: the CX does not support Queued Writes -- wait for
        // each write's completion (didWriteValueFor) before sending the next chunk, rather than
        // pipelining .withoutResponse writes the way a Queued-Write-capable peripheral would
        // tolerate.
        let chunk = outgoingQueue.removeFirst()
        awaitingWriteCompletion = true
        peripheral.writeValue(chunk, for: writeCharacteristic, type: .withResponse)
    }

    private func isBackoffState(_ state: TelemetryLinkState) -> Bool {
        if case .backoff = state { return true }
        return false
    }
}

// MARK: - CBCentralManagerDelegate

extension CxBleLink: CBCentralManagerDelegate {
    func centralManagerDidUpdateState(_ central: CBCentralManager) {
        switch central.state {
        case .poweredOn:
            if wantsToOpen, state == .idle || isBackoffState(state) { beginScan() }
        case .unsupported, .unauthorized, .poweredOff, .resetting:
            if state != .idle || wantsToOpen {
                state = .backoff(reason: .bluetoothUnavailable)
            }
        case .unknown:
            break
        @unknown default:
            break
        }
    }

    func centralManager(
        _ central: CBCentralManager, didDiscover peripheral: CBPeripheral,
        advertisementData: [String: Any], rssi RSSI: NSNumber
    ) {
        guard state == .scanning else { return }
        scanTimeoutWorkItem?.cancel()
        scanTimeoutWorkItem = nil
        central.stopScan()
        self.peripheral = peripheral
        peripheral.delegate = self
        state = .connecting
        central.connect(peripheral)
    }

    func centralManager(_ central: CBCentralManager, didConnect peripheral: CBPeripheral) {
        peripheral.discoverServices([Self.serviceUUID])
    }

    func centralManager(_ central: CBCentralManager, didFailToConnect peripheral: CBPeripheral, error: Error?) {
        self.peripheral = nil
        state = .backoff(reason: .connectionFailed)
    }

    func centralManager(_ central: CBCentralManager, didDisconnectPeripheral peripheral: CBPeripheral, error: Error?) {
        let wasIntentional = isClosingIntentionally
        isClosingIntentionally = false
        self.peripheral = nil
        writeCharacteristic = nil
        notifyCharacteristic = nil
        outgoingQueue = []
        awaitingWriteCompletion = false
        guard !wasIntentional else { return }
        state = .backoff(reason: .disconnected)
    }
}

// MARK: - CBPeripheralDelegate

extension CxBleLink: CBPeripheralDelegate {
    func peripheral(_ peripheral: CBPeripheral, didDiscoverServices error: Error?) {
        guard let service = peripheral.services?.first(where: { $0.uuid == Self.serviceUUID }) else {
            state = .backoff(reason: .connectionFailed)
            return
        }
        peripheral.discoverCharacteristics([Self.notifyCharacteristicUUID, Self.writeCharacteristicUUID], for: service)
    }

    func peripheral(_ peripheral: CBPeripheral, didDiscoverCharacteristicsFor service: CBService, error: Error?) {
        guard let characteristics = service.characteristics else {
            state = .backoff(reason: .connectionFailed)
            return
        }
        for characteristic in characteristics {
            if characteristic.uuid == Self.notifyCharacteristicUUID {
                notifyCharacteristic = characteristic
            } else if characteristic.uuid == Self.writeCharacteristicUUID {
                writeCharacteristic = characteristic
            }
        }
        guard let notifyCharacteristic else {
            state = .backoff(reason: .connectionFailed)
            return
        }
        peripheral.setNotifyValue(true, for: notifyCharacteristic)
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateNotificationStateFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == Self.notifyCharacteristicUUID else { return }
        guard error == nil, characteristic.isNotifying, writeCharacteristic != nil else {
            state = .backoff(reason: .connectionFailed)
            return
        }
        // docs/research/obdlink-cx-ble.md §1: vendor max is 247 bytes, but iOS negotiates
        // per-connection -- query the real value rather than assume one.
        maxWriteLength = peripheral.maximumWriteValueLength(for: .withResponse)
        state = .ready
    }

    func peripheral(_ peripheral: CBPeripheral, didUpdateValueFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == Self.notifyCharacteristicUUID, error == nil, let data = characteristic.value else { return }
        onIncoming?(data)
    }

    func peripheral(_ peripheral: CBPeripheral, didWriteValueFor characteristic: CBCharacteristic, error: Error?) {
        guard characteristic.uuid == Self.writeCharacteristicUUID else { return }
        awaitingWriteCompletion = false
        flushOutgoingIfPossible()
    }
}
