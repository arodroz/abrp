# OBDLink CX on iOS — BLE protocol, CoreBluetooth behavior, and sleep/wake

> Research for wayfinder map issue #72 ("live vehicle telemetry — modular OBD for any EV"), ticket #75.
> Covers the BLE **transport** between an iOS Swift app and a ScanTool/OBDLink CX dongle. For the
> OBD-II/UDS **payload** side (PIDs, DIDs, byte offsets for the Hyundai Ioniq 5 BMS), see
> [`ioniq5-obd-telemetry.md`](ioniq5-obd-telemetry.md) §3 and §6, which this document assumes as
> background and does not repeat.
>
> Sourcing note: the canonical "OBDLink Family Reference and Programming Manual" (FRPM) PDF links on
> scantool.net returned HTTP 403 to automated fetches during this research. §3 and §4's FRPM-derived
> facts come from a third-party mirror of **Revision D** (dated 2020-10-06) instead — see the caveat at
> the top of §3. Where a claim rests on that mirror alone, or on forum/community sources rather than
> ScanTool's own site, it is flagged inline and again in the "Unverified" list at the end.

## 1. BLE GATT layout

**Advertised name.** The CX advertises as the literal string `OBDLink CX`, confirmed by two independent
BLE clients that scan for exactly that name against real hardware: an ESP32/NimBLE client
([`vdvornichenko/obd-ble-serial`](https://github.com/vdvornichenko/obd-ble-serial/blob/96ebbcdd7f643422ffc28b0ad4ce92d07bb298ec/src/BLEClientSerial.cpp))
and a Python/bleak client built and tested against a physical CX
([`ryanchen2134/bletest`](https://github.com/ryanchen2134/bletest/blob/669383eda5a02040397a426d15df09397d7a2754/shell.py)).

**Services and characteristics.** ScanTool's own developer documentation, the
["OBDLink CX Adapter Notes"](https://support.obdlink.com/support/solutions/articles/43000746707-obdlink-cx-adapter-notes)
article, documents two GATT services:

| Service | Characteristics | Purpose |
|---|---|---|
| Custom serial/UART, `0xFFF0` (`0000FFF0-0000-1000-8000-00805F9B34FB`) | `0xFFF1` — notify (adapter → app); `0xFFF2` — write (app → adapter) | ELM/ST command I/O |
| Standard Device Information Service, `0x180A` | manufacturer name, model number, firmware/software revision | identification |

This is not just a vendor-PDF claim — it is independently confirmed **in shipped source code**, across
languages and platforms, satisfying the requirement to cite a concrete implementation:

- Swift: [**kkonteh97/SwiftOBD2**](https://github.com/kkonteh97/SwiftOBD2/blob/9f027e6eae0bd8e3536bfac7583ec26ac227c38c/Sources/SwiftOBD2/Communication/BLE/BLECharacteristicHandler.swift) hardcodes `"FFF1"` (notify) / `"FFF2"` (write) under service `FFF0`, one of three supported adapter families.
- Swift: [**Ryokugyoku/ProjectZD8**](https://github.com/Ryokugyoku/ProjectZD8/blob/e088d335af249e617a9f48e5db42149248effad8/ProjectZD8/Data/Devices/OBD/AppleBluetoothUARTProfile.swift) labels the FFF0/FFF1/FFF2 profile as the CX's "official spec."
- Swift: [**Abhi011999/the_gauge_experiment**](https://github.com/Abhi011999/the_gauge_experiment/blob/4caf0b478c8726055fd49d12e545659a4a5e7764/Sources/BLE/OBDBLEManager.swift) declares `fff0Service`/`fff1TX`/`fff2RX` CBUUID constants.
- Kotlin/Android: [**jrmdev/grenadiag-android**](https://github.com/jrmdev/grenadiag-android/blob/f4bdf45c3475224dc495bdbfa4212e848c9b2a05/transport/src/main/java/app/grenadiag/transport/AdapterProfile.kt) has a named `AdapterProfiles.ObdLinkCx` value with the same three UUIDs.
- C++/ESP32: [**obd-ble-serial**](https://github.com/vdvornichenko/obd-ble-serial/blob/96ebbcdd7f643422ffc28b0ad4ce92d07bb298ec/src/BLEClientSerial.cpp) and its fork [**dgaust/SealOBD**](https://github.com/dgaust/SealOBD/blob/main/BLEClientSerial.cpp) connect to real CX hardware using the same triad.
- Python/bleak: [**petrpatek/obd2-mcp-server**](https://github.com/petrpatek/obd2-mcp-server/blob/42006c3fe5025dfbc2b8758ad2ca47d9ac5c1c25/src/obd2_mcp/ble_connection.py) labels its FFF0 profile entry "FFF0 (vLinker FD, OBDLink CX, most modern BLE ELM327)."

One conflicting source: [**RobDeGeorge/OCTAVE**](https://github.com/RobDeGeorge/OCTAVE/blob/08e8245644ada0eb6119820c2f3256c6a70edf4d/android/src/org/octave/app/OctaveOBDBridge.java)
files "OBDLink CX" under a Microchip ISSC Transparent UART profile (`49535343-FE7D-4AE5-8FA9-9FAFD205E455`)
instead. Given the vendor doc plus five independent codebases above all agree on FFF0/FFF1/FFF2, this is
most likely an error in that one project (possibly confused with a similarly-branded clone) — flagged in
the unverified list rather than treated as evidence of a real hardware variant.

**Write vs. write-without-response.** ScanTool's adapter notes state the CX "does not support Queued
Writes" and that a client should wait for each write to complete before sending the next — but this is
guidance about not *pipelining* writes, not a ban on the write-without-response *type* itself.
[`Cornucopia-Swift/CornucopiaStreams`](https://github.com/Cornucopia-Swift/CornucopiaStreams/blob/c35de6e0d642f2e6b98fd9227375b79e32ab6d06/Sources/CornucopiaStreams/Streams/BLECharacteristicOutputStream.swift)
— from the author of the long-running `LTSupportAutomotive` OBD library — has an explicit code comment
warning that without Queued Write support, pipelining `.withoutResponse` writes at the negotiated MTU
will silently drop bytes and stall transfers. A Kotlin implementation states the CX's actual property
assignment directly:
[`rogerneumann/autovakt`](https://github.com/rogerneumann/autovakt/blob/4b3af8954abed0df0c25352dc12634e218408c4f/app/src/main/kotlin/com/rogerneumann/autovakt/obd2/ElmBleTransport.kt)
— "FFF1=NOTIFY, FFF2=WRITE_NO_RESPONSE." The safe patterns seen in real code: either always use
`.withResponse` (SwiftOBD2's simpler, non-dynamic choice), or check
`characteristic.properties.contains(.writeWithoutResponse)` and gate consecutive writes on
`peripheral.canSendWriteWithoutResponse` rather than firing them back-to-back (ProjectZD8,
the_gauge_experiment).

**Notify, not indicate.** Unanimous across every source checked — the vendor doc calls it "notifications
(push model)," and every client (`.notify` / `setNotifyValue(true, …)`) arms Notify, never Indicate, on
FFF1.

**MTU.** Vendor-documented max is **247 bytes**. On iOS there is no fixed value — CoreBluetooth
negotiates per-connection, and real clients query `peripheral.maximumWriteValueLength(for:)` rather than
assume a size before chunking a write.

**Chunking and reassembly.** Writes are chunked to the negotiated max write length (seen in ProjectZD8
and the_gauge_experiment). On the read side, real clients accumulate incoming notification packets into a
buffer and treat the ELM prompt byte `0x3E` (`>`) as the end-of-response marker — confirmed independently
in Swift
([SwiftOBD2's `BLEDataProcessor`](https://github.com/kkonteh97/SwiftOBD2/blob/9f027e6eae0bd8e3536bfac7583ec26ac227c38c/Sources/SwiftOBD2/Communication/BLE/BLEDataProcessor.swift#L580-L586):
`if string.contains(">") { ...; buffer.removeAll() }`), Swift again (ProjectZD8's
`extractPromptResponse()` slices the buffer at the first `0x3E` byte), and Python
([`obd2-mcp-server`](https://github.com/petrpatek/obd2-mcp-server/blob/42006c3fe5025dfbc2b8758ad2ca47d9ac5c1c25/src/obd2_mcp/ble_connection.py)
strips trailing `>` lines, echoed commands, and `SEARCHING` status lines from the accumulated buffer).

## 2. iOS specifics

**Pairing/bonding: not a prerequisite in iOS Settings.** Car Scanner ELM OBD2's own iOS setup guide is
explicit: ["you don't need to setup pairing with adapter in the iPhone/iPad system settings!"](https://www.carscanner.info/ios-bt4/)
— connection happens entirely inside the app via CoreBluetooth. ScanTool's
["OBDLink CX Adapter Notes"](https://support.obdlink.com/support/solutions/articles/43000746707-obdlink-cx-adapter-notes)
explains why an OS pairing *dialog* can still appear without a separate Settings trip: BLE bonding is
enabled for the first 5 minutes after power-on, the legacy pairing PIN is `123456`, and it is triggered as
a side effect of the app's own CoreBluetooth calls — subscribing to FFF1 or writing to FFF2 can itself
prompt the OS pairing sheet. A shipping app's localized strings corroborate the same 5-minute window
independently: [`fdittgen-png/tankstellen`](https://github.com/fdittgen-png/tankstellen) documents that
"the OBDLink CX family pairs on the first connection and only accepts new pairings in the first ~5
minutes after power-on." (ScanTool's more generic consumer-facing "Get Started" guides do tell users to
pair via Settings first — that is blanket guidance covering their Classic-Bluetooth models too, not a
contradiction of the CX's actual BLE behavior.)

**State restoration: a real, working pattern, but not universal.**
[SwiftOBD2](https://github.com/kkonteh97/SwiftOBD2/blob/9f027e6eae0bd8e3536bfac7583ec26ac227c38c/Sources/SwiftOBD2/Communication/BLE/bleManager.swift#L1006-L1013)
initializes `CBCentralManager` with `CBCentralManagerOptionRestoreIdentifierKey` and implements
`centralManager(_:willRestoreState:)` to re-attach to a restored peripheral.
[the_gauge_experiment](https://github.com/Abhi011999/the_gauge_experiment/blob/4caf0b478c8726055fd49d12e545659a4a5e7764/Sources/BLE/OBDBLEManager.swift)
does the same with its own restore identifier and re-discovers services/reconnects on restore.
[tankstellen](https://github.com/fdittgen-png/tankstellen/blob/ca81ce16e5b874a65b69743149d79c0183c0f3c1/ios/Runner/AppDelegate.swift)
goes further, reading `launchOptions[.bluetoothCentrals]` at app-launch time specifically to detect "the
paired adapter powers up while the app is terminated" and resume a hands-free flow. Not every
implementation bothers: ProjectZD8 deliberately omits a restore identifier, relying on the user to reopen
and reconnect — a legitimate simpler alternative if wayfinder does not need launch-while-terminated
behavior.

**Backgrounding.** Per Apple's CoreBluetooth background-processing model, an app needs the
`bluetooth-central` `UIBackgroundModes` entitlement to keep receiving central-role callbacks
(notifications, restoration) while backgrounded; without it the app suspends shortly after backgrounding
and CoreBluetooth events queue until it is foregrounded again. All the state-restoration code above
implies this entitlement is declared (restoration is pointless without it), but no directly-inspected
`Info.plist` was found in these repos to confirm the literal key — treat as inferred, confirm in our own
target's manifest.

**MFi: confirmed not required.** Apple's own Technical Q&A
[QA1657](https://developer.apple.com/library/archive/qa/qa1657/_index.html) states plainly: "Bluetooth
low energy accessories do not interface with the External Accessory framework and are not required to be
MFi compliant. Instead, apps use the CoreBluetooth framework..." ScanTool's own
[adapter comparison page](https://support.obdlink.com/support/solutions/articles/43000713351-which-obdlink-adapter-is-right-for-me-)
classifies the CX as "Bluetooth v5.1 LE (BLE)" versus the MX+'s "Bluetooth v3.0" (Classic) — so Apple's
general BLE rule applies to it. No single ScanTool sentence says "CX doesn't need MFi" in those words, so
this is an inference chain (ScanTool's BLE classification + Apple's general rule), not a direct quote —
but it is corroborated by real, concrete dual-transport app code:
[ProjectZD8](https://github.com/Ryokugyoku/ProjectZD8) implements two entirely separate transport
classes, a [CoreBluetooth-based one](https://github.com/Ryokugyoku/ProjectZD8/blob/e088d335af249e617a9f48e5db42149248effad8/ProjectZD8/Data/Devices/OBD/AppleCoreBluetoothOBDTransport.swift)
for the CX/FFF0 family and a separate
[ExternalAccessory-based one](https://github.com/Ryokugyoku/ProjectZD8/blob/e088d335af249e617a9f48e5db42149248effad8/ProjectZD8/Data/Devices/OBD/IOSExternalAccessoryOBDTransport.swift)
gated on `.bluetoothClassic`, for MFi-class adapters like the OBDLink EX/MX+. Weaker secondary
corroboration: [teslax.app](https://teslax.app/supported-hardware/) and an
[EVNotify GitHub issue comment](https://github.com/EVNotify/EVNotify/issues/245#issuecomment-880719434)
both describe the MX+ as the MFi-certified/"Made for iPhone" model, implicitly contrasting with the CX's
plain-BLE path.

## 3. Command dialect

> Caveat covering this whole section: the FRPM revision actually read (Rev D, dated 2020-10-06, via a
> [third-party mirror](https://device.report/m/9a0c19e7c21abf30e5f5bed54411f5adb256424912a751b845dcfa1eb4611598.pdf)
> after ScanTool's own FRPM PDF links on scantool.net returned HTTP 403 to automated fetches) **never
> mentions "OBDLink CX" by name anywhere in its 60 pages** — not in the device table, the IC table, the
> STPX size table, or the PowerSave defaults section. Newer revisions (E, F) are linked from scantool.net
> but were unreachable the same way. Every FRPM-sourced fact below is a family-level claim whose
> applicability to CX specifically is inferred, not stated — worth another access attempt at the real PDF,
> and worth confirming per-fact on real hardware via the CX's own `STSLCS`/`STI` output.

**Chipset.** The FRPM's IC table lists only STN1110, STN1170, STN2100, and STN2120 as "OBDLink ICs,"
mapping specific finished products to them (e.g. STN2255 = MX+) — CX is not in that table. ScanTool's own
[CX Adapter Notes](https://support.obdlink.com/support/solutions/articles/43000746707-obdlink-cx-adapter-notes)
only say it runs "the STN IC," without naming a part number. A specific-sounding "STN2310" claim surfaced
in web-search synthesis but could not be traced to any document — **unverified, flag for hardware check**
(the `STI`/`STIX` command prints the real `STN<device_id> vX.Y.Z` firmware string per the FRPM). What is
vendor-confirmed: the CX is **Bluetooth 5.1 BLE only** (no Classic Bluetooth) — architecturally distinct
from the MX/LX/MX+ line.

**ELM/ST dialect.** The FRPM states, at the family level, that OBDLink devices are "fully compatible with
the de facto industry standard ELM327 command set" plus a "feature-rich parallel extended ST command
set." No CX-specific AT-command enumeration exists in the revision read, since CX is not named in it —
reasonable to extend the family claim to CX, but not independently confirmed for it.

**STPX — the claim to verify.** The imported report ([`ioniq5-obd-telemetry.md`](ioniq5-obd-telemetry.md)
§3) said "STPX ... supported on MX+/LX (STN11xx/12xx)" and flagged CX as needing FRPM confirmation. The
FRPM's actual "max message size by device" table for `STPX` (§8.6) is more granular than that grouping:
**MX+ and EX (and the bare STN2100/STN2120 chips) → 4 KB; LX and MX (and the bare STN1110/STN1170 chips)
→ 2 KB.** **OBDLink CX is absent from this table entirely.** That absence is the best evidence found that
CX lacks the STPX fast multiframe path, and it is consistent with the CX's positioning as a
BMW/BimmerCode-focused, cost-reduced BLE product versus MX+'s ECU-flashing-capable role — but no single
vendor sentence explicitly says "STPX is unsupported on CX" (ScanTool's
[Carbyte FAQ](https://www.obdlink.com/carbyte-faq/) and
[adapter comparison page](https://support.obdlink.com/support/solutions/articles/43000713351-which-obdlink-adapter-is-right-for-me-)
do not mention STPX at all). **Verdict: likely true, medium-low confidence — confirm on hardware by
sending `STPX d:0100` to a real CX and checking for the FRPM's documented `?` invalid-command response.**

**Other config commands** (family-level, CX-applicability unconfirmed per the caveat above):
- `STSLEEP [delay]` — force immediate or delayed sleep.
- `ATST hh` is explicitly deprecated in the FRPM ("supported for backwards compatibility only. Use STPTO
  instead"); the live timeout command is `STPTO ms` (decimal milliseconds, default **102 ms**, backed by
  programmable parameter PP 03). OBDLink's adaptive-timing mode (`ATAT`, default on) auto-tunes this
  per-vehicle, so manual tuning is mainly useful for atypical ECUs.
- Flow control: `ATFCSD`/`ATFCSH`/`ATFCSM` (AT-namespace, matching the sequence already used in the
  imported PID report) plus `ATCFC 1|0` (default on); ST extensions add multi-target pairing
  (`STCFCPA`/`STCFCPC`) and `STCTOR fcTimeout, cfTimeout` (default **75 ms / 150 ms**).
- Power/diagnostic commands worth knowing for bring-up: `STSLLT` (report last sleep/wake trigger),
  `STSLCS` (dump the active PowerSave config off a real unit — the recommended first step against real CX
  hardware, since its factory defaults are not in the manual revision read), `STSLU`/`STSLUIT`,
  `STSLVL`/`STSLVLS`/`STSLVLW`, `STSLVG`/`STSLVGW`, `STVR`/`STVCAL`.

All of the above are cited to the same
[FRPM Rev D mirror](https://device.report/m/9a0c19e7c21abf30e5f5bed54411f5adb256424912a751b845dcfa1eb4611598.pdf).

## 4. Sleep/wake and 12V safety

This is the backstop against draining the Ioniq 5's 12V battery — a platform with its own well-documented
ICCU/12V weakness (see §6). Precision matters more than brevity here.

**Sleep triggers.** The FRPM defines four independent chip-level sleep triggers — explicit command, UART
inactivity, an external SLEEP pin, and low voltage — **all off by default**; a finished product must
configure at least one. The manual's per-product defaults table only covers OBDLink MX Bluetooth/LX/MX+
(UART-inactivity sleep **on**, default **600 s / 10 min**) and MX Wi-Fi (**2 hr**). **CX's own
factory-configured sleep timeout is not documented anywhere reachable** — flag for hardware check via
`STSLCS`.

**The single most important finding in this document, quoted directly from the FRPM (§15.2.2):**

> "OBDLink UART inactivity sleep trigger is disabled while any command is executing. In other words,
> OBDLink must print the command prompt before it will act on a sleep trigger. Therefore, commands which
> require UART activity to terminate their execution (e.g., ATMA, STMA, etc.) will keep the device awake
> indefinitely. A continuous stream of incoming messages may also prevent the device from going to
> sleep."

In plain terms: **the idle-sleep timer is entirely defeated by app/session behavior**, independent of how
good the CX's own sleep circuitry is. A wayfinder session that leaves a continuous "monitor all" stream
open, or otherwise never returns to the command prompt, will keep the CX awake indefinitely regardless of
any configured timeout. This is corroborated by a community report (Hyundai Ioniq Forum, paraphrased)
that companion apps holding a session open prevent an MX+ from ever reaching sleep, with a documented
workaround of fully disconnecting and letting ScanTool's own app command deep sleep before walking away.
**Design implication: wayfinder's transport must explicitly send `STSLEEP` (or cleanly disconnect in a way
that lets the timer run) whenever a session ends — never rely on the idle timer alone.**

**Wake sources.** The FRPM documents this differently for two product families, and **CX matches neither
documented case exactly**:
- Wired OBDLink hardware with pluggable wireless modules: wireless radios are fully unpowered in sleep,
  so "it is not possible to wake up the device over a wireless link" — voltage-based wake only.
- The integrated-Bluetooth-Classic products (MX Bluetooth, LX, MX+): "By default, they are configured to
  go to sleep on UART inactivity (after 10 minutes), and **wake up on Bluetooth connection or voltage
  change**."
- **CX (BLE) is not covered by either paragraph.** Whether an incoming BLE central-connection attempt can
  wake a sleeping CX (as it does for the Classic-Bluetooth models) or whether CX's radio is fully off in
  sleep (as in the wired-hardware case) is an **open, safety-relevant question** — flag for hardware test:
  put a CX to sleep, then attempt a BLE connection with the ignition off and no cranking event, and see
  whether it responds.
- Voltage-based wake is chip-level and reads the DLC's pin-16 constant-12V line via an analog input, **not
  CAN bus traffic**: `STSLVLW` (level wake, documented example default `>13.20 V for 1 s`) and `STSLVGW`
  (change-based wake, default **0.20 V change within a 1000 ms window**, explicitly intended — per the
  FRPM — "to wake up the device when the starter motor is cranking ... or when the engine starts up,"
  since it does not depend on an absolute voltage level that varies by vehicle). **The FRPM does not
  document CAN-bus activity itself as a wake trigger anywhere in the PowerSave section** — CAN only
  matters once the device is already awake.

**Sleep current draw.** Two ScanTool-branded sources agree in order of magnitude: the
[CX Adapter Notes](https://support.obdlink.com/support/solutions/articles/43000746707-obdlink-cx-adapter-notes)
give **<2 mA** asleep / **55 mA** active; the
[official CX product page](https://www.obdlink.com/products/obdlink-cx/) gives **<1 mA** in its
"BatterySaver Low Power Mode" / **55 mA** active, operating voltage **8–18 V**, overvoltage protection to
**100 V**. No ScanTool material was found comparing this draw to a "safe for N hours/days parked" budget —
for reference only (not vendor-stated), 1–2 mA continuous is roughly 0.024–0.05 Ah/day, a small fraction
of typical AGM aux-battery capacity, so **if the CX actually reaches sleep**, the sleep-current spec
itself is not the risk — whether it reaches sleep at all (see the warning above) is.

**Configurable settings, exact syntax** (family-level, from the FRPM):
- `STSLUIT sec` — UART-inactivity timeout; bare-chip firmware default **1200 s (20 min)** (finished MX
  BT/LX/MX+ products ship reconfigured to 600 s — no equivalent finished-product number found for CX).
- `STSLU sleep, wakeup` — independently toggle the inactivity-sleep trigger and the UART-pulse wake
  trigger.
- `STSLVLS`/`STSLVLW` — voltage-level sleep/wake thresholds and dwell time (documented example: sleep
  below 13.00 V for 600 s, wake above 13.20 V for 1 s).
- `STSLVGW [+|-]volts, ms` — voltage-change wake sensitivity/window (default 0.20 V / 1000 ms).
- `STSLEEP [delay]` — force sleep now (or after a delay) — the command wayfinder should call explicitly
  on clean disconnect.
- `STSLCS` — print the full active PowerSave configuration; **run this against a real CX first**, since
  none of the above defaults are confirmed for it specifically.

## 5. Known-good integrations

| Project | Platform | What it demonstrates |
|---|---|---|
| [kkonteh97/SwiftOBD2](https://github.com/kkonteh97/SwiftOBD2) | Swift (iOS/macOS) | Exact CX CBUUID constants; full CoreBluetooth state-restoration flow; `>`-prompt buffer termination. The single best end-to-end reference. |
| [Ryokugyoku/ProjectZD8](https://github.com/Ryokugyoku/ProjectZD8) | Swift (iOS) | Async/await CoreBluetooth wrapper; write-chunking via `maximumWriteValueLength(for:)`; `0x3E`-byte reassembly; and — uniquely — a second, separate `ExternalAccessory` transport for MFi/Classic adapters, making the CX-vs-MFi split concrete in one codebase. |
| [Abhi011999/the_gauge_experiment](https://github.com/Abhi011999/the_gauge_experiment) | Swift (iOS) | Another independent FFF0/FFF1/FFF2 + state-restoration implementation; defensive write-without-response gating via `canSendWriteWithoutResponse`. |
| [jrmdev/grenadiag-android](https://github.com/jrmdev/grenadiag-android) | Kotlin (Android) | Clean minimal `AdapterProfile` model with `ObdLinkCx` as a named, primary profile — same GATT contract, useful as a non-iOS cross-check. |
| [Cornucopia-Swift/CornucopiaStreams](https://github.com/Cornucopia-Swift/CornucopiaStreams) (+ [mickeyl/LTSupportAutomotive](https://github.com/mickeyl/LTSupportAutomotive)) | Swift (Apple platforms) | Names the CX's no-Queued-Writes quirk explicitly in source comments; wraps BLE characteristics as `InputStream`/`OutputStream` for an ELM/UDS command layer to sit on top of. From the developer behind a widely-used iOS OBD app. |
| [vdvornichenko/obd-ble-serial](https://github.com/vdvornichenko/obd-ble-serial) / [dgaust/SealOBD](https://github.com/dgaust/SealOBD) | C++ (ESP32/Arduino) | Minimal central-role client against real CX hardware; also sets up BLE bonding (PIN 123456) matching the vendor-documented pairing behavior. |
| [ryanchen2134/bletest](https://github.com/ryanchen2134/bletest) | Python (bleak) | Independent real-hardware confirmation of FFF1/FFF2 from a third language/stack. |
| [petrpatek/obd2-mcp-server](https://github.com/petrpatek/obd2-mcp-server) | Python (bleak) | Defensive multi-profile discovery pattern: try FFF0 first, send `ATZ`, only commit on an ELM/STN banner; also demonstrates response-buffer cleanup (strip `>`, echo, `SEARCHING`). |
| Car Scanner ELM OBD2 | iOS/Android app (closed-source) | Docs explicitly confirm CX compatibility and are the clearest available statement that no iOS-Settings pairing step is needed for BLE adapters like the CX. |
| BimmerCode | iOS/Android app (closed-source) | ScanTool itself [markets the CX as built for BimmerCode](https://www.obdlink.com/products/obdlink-cx/) — the strongest possible compatibility signal, though it confirms compatibility only, not implementation details (closed source). |
| [RobDeGeorge/OCTAVE](https://github.com/RobDeGeorge/OCTAVE) | Java/Android (via Qt) | Useful for raw CCCD-descriptor notify-arming mechanics, but flagged above for misfiling CX under the wrong GATT profile — read with that caveat. |

Not found despite specific searches: **Sidecar** (CarPlay OBD widget) exists but no docs naming CX
compatibility specifically; **OVMS** has no connection to the CX/BLE transport at all; **EVNotify**'s BLE
work (issues #215, #245) covers vLinker/Veepeak/MX+, not CX by name.

## 6. Known pitfalls

- **App-held sessions defeat sleep** (mechanism vendor-documented in §4; real-world report
  community-sourced from Hyundai Ioniq Forum) — the most actionable pitfall for wayfinder's own
  connection lifecycle: an app that does not cleanly close or send `STSLEEP` when the user walks away can
  keep the adapter awake indefinitely regardless of its configured timeout.
- **Single-client only, vendor-documented, applies to all OBDLink models including CX**:
  ["you may have issues connecting if another OBD app is running, because the OBDLink adapter can only
  connect to one app at a time"](https://support.scantool.net/support/solutions/articles/43000715570-troubleshoot-connection-issues),
  with a
  [companion troubleshooting article](https://support.scantool.net/support/solutions/articles/43000733076-troubleshoot-obdlink-issues-after-using-other-obd-apps)
  confirming the fix is fully closing the first connection. There is no graceful hand-off — a second
  connection attempt simply fails while the first is attached.
- **CX is positioned and marketed around BMW/BimmerCode use**
  ([product page](https://www.obdlink.com/products/obdlink-cx/),
  [adapter comparison](https://support.obdlink.com/support/solutions/articles/43000713351-which-obdlink-adapter-is-right-for-me-)),
  with a stated compatibility carve-out for some pre-2008 GM/Ford/FCA vehicles. Hyundai/Kia E-GMP is not
  called out either way — not a documented incompatibility, just circumstantial support for CX being a
  functional subset of MX+ (see STPX above).
- **Community reports of Ioniq 5 BLE connections that establish but then fail to "reach the vehicle"** on
  [InsideEVs Forum](https://www.insideevsforum.com/community/threads/obdlink-cx-and-abrp-pairing.14066/)
  (one specifically about ABRP) and
  [ioniqforum.com](https://www.ioniqforum.com/threads/obd-dongle-and-ioniq-5.39595/), with fixes reported
  as picking the correct vehicle/protocol profile or updating CX firmware via ScanTool's own app.
- **Counter-evidence exists too**: other Ioniq 5/EV6/Ioniq 6 owners on
  [kiaevforums.com](https://www.kiaevforums.com/threads/dongle-purchase-advice.6671/) and ioniqforum.com
  report leaving a CX or MX+ plugged in continuously with no observed 12V drain — consistent with the
  failure mode being usage/app-pattern-dependent (see the sleep-defeat warning in §4) rather than an
  inherent CX defect.
- **An unverified claim of a Hyundai service bulletin** warning technicians that OBD dongles/insurance
  trackers "cause CAN bus errors" circulates on forums; it could not be located as an actual document. Do
  not conflate it with the real, independently verified
  [NHTSA Recall 24V-204 / Hyundai TSB 24-01-023H](https://static.nhtsa.gov/odi/rcl/2024/RCRIT-24V204-8922.pdf)
  (DTC P1A9096, ICCU hardware/software fault, 2022–2024 Ioniq 5 / 2023–2024 Ioniq 6) — that TSB is real
  and matches the recall context in the imported PID report, but it is about the ICCU itself and says
  nothing about OBD dongles.
- **A CX-specific bug was reported directly on ABRP's own feedback board**:
  ["OBDLink CX connection issue / missing BLE button"](https://abrp.featurebase.app/p/obdlink-cx-connection-issue-missing-ble-button-for-byd-seal)
  for a BYD Seal U profile — the CX itself "works perfectly" with ScanTool's own app; ABRP's vehicle-profile
  logic was hiding the BLE option for that one profile. Marked Completed. Useful precedent: some "CX
  doesn't work" reports are app-side profile bugs, not CX firmware/hardware defects.
- Not found: any CVE or issue describing a CX-specific BLE-stack firmware bug beyond generic dongle
  flakiness, or a report of two phones fighting over one CX beyond the general single-client statement
  above.

## Unverified / must confirm on hardware

Everything below should be checked against the user's real OBDLink CX and Ioniq 5 before being trusted —
this is exactly what the map's later driveway-smoke ticket should verify:

1. **STPX absence on CX** — inferred from its absence in the FRPM's per-device max-message-size table,
   not a direct vendor sentence. Verify: send `STPX d:0100` to a real CX; expect `?` (invalid command) if
   truly unsupported.
2. **Whether a BLE connection attempt wakes a sleeping CX.** Documented as yes for the Classic-Bluetooth
   MX/LX/MX+ line, documented as impossible for wired-hardware-plus-module combos; CX (BLE) is covered by
   neither case in the FRPM revision read. **Safety-critical** — verify by sleeping a CX and attempting a
   fresh BLE connection with the ignition off and no cranking event.
3. **CX's factory-configured idle/sleep timeout and full PowerSave config.** Not published anywhere
   found. Verify by running `STSLCS` against a real unit.
4. **Exact STN chip part number inside the CX.** Only an untraceable "STN2310" claim surfaced. Verify via
   `STI`/`STIX` on real hardware.
5. **FRPM revisions E/F** (linked from scantool.net) were unreachable (HTTP 403 to automated fetches) and
   might add an explicit CX section that resolves items 1–4 directly. Worth a manual/browser fetch attempt
   before relying on hardware testing alone.
6. **No vendor-published comparison of CX sleep-current draw against a safe parked-battery time budget.**
   Worth an explicit multi-day drain test with a 12V monitor, as the imported PID report already
   recommends generally.
7. **The alleged Hyundai TSB about OBD dongles causing CAN bus errors** — could not be located as a real
   document; treat as unconfirmed folklore distinct from the verified ICCU recall TSB.
8. **Literal `UIBackgroundModes: bluetooth-central` Info.plist declaration** — inferred as necessary from
   Apple's docs and from state-restoration code that depends on it, but not directly observed in any
   reference repo's manifest. Confirm in our own target's Info.plist when built.
9. **"CX does not require MFi" as an explicit ScanTool statement** — not found verbatim; currently an
   inference chain (ScanTool's own "BLE" classification of CX + Apple's general BLE/MFi rule + ProjectZD8's
   real dual-transport code split). Low risk, but technically inferred rather than quoted.
10. **RobDeGeorge/OCTAVE's claim that CX uses the ISSC Transparent UART service** (`49535343-...`) instead
    of FFF0 — likely a one-project error (everything else disagrees), not independently resolved against a
    second source.
11. **Which BLE radio/SoC sits behind the STN interpreter chip in the CX** — not documented anywhere
    found; ScanTool's public STN1170/STN2120 datasheets cover the OBD-protocol interpreter silicon, not a
    BLE module.
12. **Sidecar's specific OBDLink CX compatibility** — the app exists and targets generic OBD2/OBDLink
    adapters, but no docs naming CX specifically were found.
13. **OVMS/EVNotify BLE-CX-specific integration** — OVMS has no connection to the CX found at all;
    EVNotify's BLE dongle work covers other adapters (vLinker, Veepeak, MX+), not CX by name.
