# Reading live pack-level EV telemetry from a Hyundai Ioniq 5 (E-GMP) over OBD-II

> Imported research report (wayfinder live-telemetry map). Source: user-provided Claude
> research artifact, captured 2026-09-01 from
> <https://claude.ai/public/artifacts/fd9f47e8-a60a-4360-a408-3d10e682ffa9>.
> Community-derived reverse-engineering facts, cross-validated across OVMS / EVNotify /
> Torque PID sets but NOT verified against our car yet — see its Caveats section, and
> treat every byte offset as "verify on live car or test vectors before trusting".
> Plain-text import; tables read as tab-separated lines.

Reading Live Pack-Level EV Telemetry from a Hyundai Ioniq 5 (E-GMP) over OBD-II: An Implementation Guide for Tool Builders
TL;DR
You can read everything you listed directly over standard OBD-II. The BMS answers UDS Mode 22 (ReadDataByIdentifier) on 11-bit CAN, 500 kbps (ELM ATSP6), request header 7E4, response 7EC. DIDs 22 01 01 and 22 01 05 carry true BMS SoC, display SoC, SoH, pack current/voltage, cell min/max, module temps, and cumulative charge/discharge counters; there is no security gateway blocking these read-only reads.
The decoding is already fully reverse-engineered and cross-validated across three independent FOSS codebases (OVMS vehicle_hyundai_ioniq5, EVNotify/EVNotiPi, and the Esprit1st/JejuSoul Torque-Pro PID sets). For a 72.6 kWh EU (marketed "73 kWh") AWD car the formulas are identical to the 77.4 kWh set — only two capacity constants differ. Build against these, but write your own code: the PID facts are freely reusable, yet most of the reference code is GPL/CC-BY-SA, not MIT.
The real risks are 12V-battery drain and keeping the car awake, not warranty or DTCs. The DLC stays powered with the car off, CAN data only flows when the car is "on"/accessory/charging, and a dongle left plugged in can flatten the (already fragile, ICCU-recall-prone) 12V battery. Polling during DC fast charging is safe and is exactly when the richest data is available.
Key Findings
Bus/protocol: ISO 15765-4 CAN, 11-bit headers, 500 kbps, on OBD-II pins 6 (CAN-H) and 14 (CAN-L). ELM/STN protocol 6 (ATSP6). No 29-bit needed for the pack data.
ECUs: BMS = 7E4→7EC (all your primary parameters). ICCU/aux = 7E5→7ED (12V SoC, may need an extended diagnostic session). Cluster/odometer = 7C6→7CE. A/C+speed+cabin temp = 7B3. TPMS = 7A0. Steering = 730.
The two DIDs that matter: 22 01 01 (status, current, voltage, temps, cell min/max, cumulative counters, relay/charge flags) and 22 01 05 (display SoC, SoH, remaining energy, available power, more temps). Both return multi-frame ISO-TP responses requiring flow control.
73 kWh vs 77 vs 84: decode formulas are the same; only capacity ceilings, cell/module counts, and the 2024+ 84.0 kWh facelift (plus some 2025 MY PID shifts) differ.
Prior art you can fork/read: OVMS (C++), EVNotiPi (Python), SoulEVSpy (Java/Android), evDash (C++/ESP32), OBDb signal database (JSON), the Esprit1st Torque CSVs, JejuSoul HKMC repo. Bluelink cloud path via bluelinky (Node) / hyundai_kia_connect_api (Python).
Gotchas: 12V drain, car-wake behavior, ELM327-clone multiframe failures, and the ICCU/12V recall context (NHTSA Campaign 24V-204 / Hyundai Recall 257, DTC P1A9096) that makes 12V drain especially consequential on this platform.
Details
1. Protocol specifics (E-GMP CAN / DLC)
Physical/link layer: The Ioniq 5 diagnostic (OBD) CAN bus is ISO 15765-4 CAN, 11-bit ID, 500 kbps, presented on the standard 16-pin DLC at pin 6 (CAN-H) and pin 14 (CAN-L), with pin 16 = battery +12V and pins 4/5 = ground. Select it with ATSP6 (equivalently STN STP33). 29-bit (ATSP7) is not required for the BMS PIDs. Internally the car uses CAN-FD and a dedicated HV CAN, but the diagnostic DLC bus that answers Mode 22 is classic 500 kbps 11-bit CAN.
Gateway: For the read-only Mode 22 DIDs discussed here, there is no central-gateway lockout — community tools read 7E4/7EC freely without authentication. The one caveat: the ICCU (7E5) aux-SoC PID sometimes requires entering a UDS extended diagnostic session (10 03) before it answers; the BMS PIDs do not.
"Awake" requirement: CAN data is only present when the vehicle is "on" (READY), in accessory/ignition-on, or charging (AC or DC). With the car fully asleep the DLC is still powered (pin 16 hot) but the BMS will not answer, or answers intermittently. For continuous logging you either poll while driving/charging or accept that a parked car goes silent. This is also why a dongle can drain the 12V (below).
2. Exact UDS / DID decoding

Request scheme (ELM327):


ATZ            ; reset
ATE0 ATL0 ATS0 ; echo/linefeed/spaces off
ATSP6          ; ISO 15765-4 CAN 11/500
ATSH 7E4       ; set request header to BMS
ATFCSH 7E4     ; flow-control header
ATFCSD 30 00 00; FC: clear-to-send, block size 0, STmin 0
ATFCSM 1       ; use our FC settings
2201 01        ; ReadDataByIdentifier DID 0101

Response comes back as an ISO-TP multi-frame block; the reassembled payload begins 62 01 01 … (positive response = request SID 0x22 + 0x40 = 0x62).

Byte-index convention used below. I give both the Torque-Pro letter offset (as published in the community CSVs) and the absolute byte index in the reassembled payload (index 0 = the 62). The mapping is absolute_index = letter_position + 1 (a→2, b→3, … e→6, K→12, m→14, …), which I cross-validated against three independently confirmed fields (SoC=byte 6, current=bytes 12–13, voltage=bytes 14–15). In Torque formulas the < operator means left bit-shift (<<) — e.g. (m<8)+n is (m<<8)+n. Signed(x) = interpret that byte as int8; multi-byte signed uses the top byte signed.

DID 22 01 01 — header 7E4 (the workhorse frame):

Parameter	Torque formula	Abs. bytes	Type/scale	Units	Confirmed?
BMS SoC (true/raw)	e/2	6	u8 ÷2	%	Confirmed (3 sources)
Battery current (DC)	((Signed(K)*256)+L)/10	12–13	s16 ÷10	A (– = charge, + = discharge)	Confirmed
Battery voltage (DC)	((m<8)+n)/10	14–15	u16 ÷10	V	Confirmed
Battery max temp	Signed(O)	16	s8	°C	Confirmed
Battery min temp	Signed(P)	17	s8	°C	Confirmed
Module temps B01–B05	Signed(Q..U)	18–22	s8	°C	Confirmed
Max cell voltage	x/50	25	u8 ÷50	V	Confirmed
Max cell #	y	26	u8	—	Community
Min cell voltage	z/50	27	u8 ÷50	V	Confirmed
Min cell #	aa	28	u8	—	Community
12V aux voltage	ad*0.1	31	u8 ×0.1	V	Community (corroborated by OVMS)
Requested charge current	((h<8)+i)/10	9–10	u16 ÷10	A	Community
Charge/relay flag byte j	bits of byte 11	11	bitfield	—	Confirmed
→ BMS main relay	{j:0}	11 b0	bit	on/off	Community
→ AC (normal) charge port	{j:5}	11 b5	bit		Community
→ DC (rapid/CCS) port	{j:6}	11 b6	bit		Community
→ HV charging active	{j:7}	11 b7	bit		Community
BMS ignition	{ay:2}	52 b2	bit		Community
Cumulative charge (Ah)	((ae<24)+(af<16)+(ag<8)+ah)/10	32–35	u32 ÷10	Ah	Confirmed
Cumulative discharge (Ah)	((ai<24)+(aj<16)+(ak<8)+al)/10	36–39	u32 ÷10	Ah	Confirmed
Cumulative energy charged (kWh)	((am<24)+(an<16)+(ao<8)+ap)/10	40–43	u32 ÷10	kWh	Confirmed
Cumulative energy discharged (kWh)	((aq<24)+(ar<16)+(as<8)+at)/10	44–47	u32 ÷10	kWh	Confirmed
Operating time	((au<24)+(av<16)+(aw<8)+ax)/3600	48–51	u32 ÷3600	h	Community
Inverter capacitor V	((az<8)+ba)	53–54	u16	V	Community
Drive motor speed rear	(Signed(BB)*256)+BC	55–56	s16	rpm	Community
Drive motor speed front	(Signed(BD)*256)+BE	57–58	s16	rpm	Community
Isolation resistance	((bf<8)+bg)	59–60	u16	kΩ	Community

DID 22 01 05 — header 7E4:

Parameter	Torque formula	Abs. bytes	Type/scale	Units	Confirmed?
Display SoC (dashboard)	af/2	33	u8 ÷2	%	Confirmed
State of Health (SoH)	((z<8)+aa)/10	27–28	u16 ÷10	%	Confirmed
Battery remaining energy	((ac<8)+ad)*2	29–30	u16 ×2	Wh	Confirmed (best SoH proxy)
Available charge power (max regen)	((q<8)+r)/100	18–19	u16 ÷100	kW	Community
Available discharge power (max)	((s<8)+t)/100	20–21	u16 ÷100	kW	Community
Cell voltage deviation	u/50	22	u8 ÷50	V	Community
Battery heater-1 temp	Signed(X)	25	s8	°C	Community
Module temps B06–B12	Signed(J..P)	11–17	s8	°C	Community
Module temps B13–B16	Signed(AN..AQ)	41–44	s8	°C	Community
Max deterioration cell #	ab	28	u8	—	Community
Min deterioration cell #	ae	31	u8	—	Community

Cell voltages — DIDs 22 01 02 / 03 / 04 (cells 1–96) and 22 01 0A / 0B / 0C (cells 97–192), header 7E4. Each cell is one byte, value/50 volts, laid out at absolute bytes 6–37 (Torque letters e…aj) within each response. Only the populated cell groups are real; on the 77.4 kWh pack cells 181–192 read 0 V and must be discarded (see pack-count nuance in §5).

Aux (12V) SoC — DID 22 E0 11, header 7E5 → single byte = %. Earlier community PID sets that read aux SoC/voltage from the VMCU (7E2) were wrong on the Ioniq 5 and were removed; on E-GMP this data lives in the ICCU. (12V voltage is more reliably taken from 7E4 22 01 01 byte 31 above.)

Odometer — DID 22 B0 02, header 7C6 → Int24 at bytes 8–10 = km (bytes 11–13 = miles). Confirmed working on a EU 2022 car.

Derived power: kW = current(A) × voltage(V) / 1000 (sign gives charge/discharge). This is a computed value, not a native PID.

SoC (BMS) vs SoC (display) — the key semantic: BMS SoC is the true electrochemical state over the manufacturer-defined usable window; the dashboard/display SoC is a buffered remap. On the Ioniq 5 the display roughly spans BMS ~1.5%→95%, so the two diverge most near full charge (community reports up to ~5% gap, occasionally much more if the pack is unbalanced). For "true" SoC use 22 01 01 e/2. For SoH, note the raw SoH PID (22 01 05 bytes 27–28) is sticky at 100% for years because Hyundai unlocks buffer to mask early degradation (owners report the PID not moving until ~7% capacity is lost); the community consensus is that "remaining energy at 100% SOC" (22 01 05 remaining-energy field, ~74 kWh new on your pack) is the more sensitive real-world capacity/health proxy until it falls far enough that the SoH PID finally starts moving.

3. ISO-TP / multi-frame handling

Both 22 01 01 and 22 01 05 return well over 7 bytes, so the ECU sends a First Frame and waits for your Flow Control frame before streaming Consecutive Frames.

Generic ELM327 sequence (works on any ELM327 v1.4+ and OBDLink):


ATSH 7E4         ; request header
ATCRA 7EC        ; (optional) only accept responses from 7EC
ATFCSH 7E4       ; flow-control header = same ECU
ATFCSD 30 00 00  ; FC data: 0x30 CTS, block size 0, STmin 0ms
ATFCSM 1         ; apply user FC
ATST 20          ; response timeout (~128ms); raise if truncated
2201 05

Set ATCAF1 (auto-formatting on) and Torque/most libs will hand you the reassembled payload; set ATCAF0 if you want to reassemble raw frames yourself (you then parse the 10 LL … First Frame length and 2x Consecutive Frame sequence counters).

OBDLink STN fast path (STPX). The STN chipset in the OBDLink MX+/LX can transmit the request and auto-handle flow control and collect the whole multiframe in one command, which is markedly faster and more robust than round-tripping ELM flow-control for high-rate logging. Form (verify exact tokens against the OBDLink FRPM):


STPX h:7E4, d:2201 05, r:0     ; h=header, d=data, r=expected responses (0=auto)

STP33 selects the protocol; STCFCP/STCFCPA manage FC pairing on STN. Caveat: STPX is only on specific OBDLink models — supported on MX+/LX (STN11xx/12xx); the FRPM flags certain STPX/feature behavior as not available on the OBDLink CX / Carbyte, so if you're on a CX, fall back to the standard ATFC* sequence. Confirm in the "OBDLink Family Reference and Programming Manual (FRPM)."

Library reality check: python-OBD supports custom OBDCommand objects but its ISO-TP/flow-control handling for long manufacturer multiframe responses is limited; most Hyundai/Kia projects either drive the ELM AT/ST commands directly over pyserial or use SocketCAN + a real ISO-TP stack (python-can + can-isotp) with a native CAN interface instead of an ELM327. If you want clean ISO-TP, a SocketCAN adapter + can-isotp kernel module is the most Unix-friendly route; with your OBDLink you'll be doing AT/ST string I/O.

4. Open-source prior art (read / fork / contribute)
Project	Lang	What it gives you	License (verify)
OVMS vehicle_hyundai_ioniq5 (openvehicles/Open-Vehicle-Monitoring-System-3)	C++	The most authoritative poll list + decoder; metrics for BMS SoC, aux SoC, cell max/min, module temps, inlet temp, LDC/DC-DC, SoH. Credited to EVNotify.	OVMS project license (open; confirm)
EVNotiPi (EVNotify/EVNotiPi, and noradtux/evnotipi fork)	Python	Car modules with cantx/canrx, ATFCSH/ATFCSD/ATFCSM sequences, SocketCAN+ELM327 dongles, watchdog for 12V. Best structural template for your tool.	Confirm in repo
Esprit1st/Hyundai-Ioniq-5-Torque-Pro-PIDs	CSV	The 74 kWh & 77 kWh PID CSVs quoted throughout this report. Directly readable formulas.	No explicit LICENSE file → treat as all-rights-reserved compilation
JejuSoul/OBD-PIDs-for-HKMC-EVs	CSV/Torque	The origin HKMC EV PID database (Kona/Niro/Soul) the Ioniq 5 set was derived from; issue #58 tracks Ioniq 5.	Confirm in repo
OBDb/Hyundai-IONIQ-5	JSON "signalsets/v3"	Structured, testable signal database (has test cases), model-year "generations.yaml". Cleanest machine-readable source to fork.	CC-BY-SA-4.0 (copyleft — matters for you)
SoulEVSpy (langemand/SoulEVSpy; commercial "Soul EV Spy" by EVRanger)	Java/Android	Reference decoder; supports Ioniq 5/6, EV6, EV9; documents 58 kWh vs larger-pack differences.	Confirm (app is "as-is")
evDash (nickn17/evDash)	C++/ESP32	"Fully supported: Ioniq 5/6, EV6." Good embedded reference.	GPL (confirm)
CSS Electronics EV6 DBC / UDS intro	DBC + docs	The clearest write-up of the exact SF-request + FC-frame handshake on this platform (EV6 = same E-GMP decoding).	Proprietary sample data pack
python-OBD (brendan-w)	Python	ELM327 transport + custom command scaffolding.	GPLv2 (confirm)

Licensing guidance for your MIT/anti-lock-in stance: the PID numbers, byte offsets, and scaling formulas are facts and not meaningfully copyrightable — reuse them freely in an MIT-licensed tool. But most reference code here is GPL, and OBDb's data is CC-BY-SA (share-alike) — so don't paste their source/data into an MIT project. Re-implement the decoder yourself from the tables in this report (all derived from multiple independent sources), and you stay clean.

5. Community sources & model-year/pack discrepancies
Primary threads: ioniqforum.com "Torque Pro PIDs for IONIQ 5" (multi-page, the canonical reverse-engineering thread, with the "Zuinige Rijder"/SoulEVSpy-dev/EV-Watchdog-dev corrections), "OBD2 PIDs for Ioniq 5/6" (ICCU 7E5 discovery, extended-session note), "SoH self-test and other BMS info," "SOC BMS and SOC," and "what is SoC versus SoC display."
Pack-size differences that change decoding constants and cell/module layout (not the formulas). SK Innovation NCM811 pouch cells, ~3.63 V nominal, throughout:
58 kWh = 144s2p, 24 modules (288 cells).
72.6 kWh (your car, marketed "73") = 180s2p (30 modules, 360 cells); ≈653 V nominal (3.63 V × 180 groups); ~111 Ah pack / ~55.6 Ah cell. Use the "74 kWh" CSV.
77.4 kWh = 192s2p (32 modules, 384 cells); this is why the PID space exposes 192 cell slots — on the 72.6 kWh pack the higher slots simply aren't populated.
84.0 kWh = 2024+ facelift (revealed 3 March 2024; confirmed 84.0 kWh in Hyundai's Dec 2024 "2025 IONIQ 5 Features and Specifications").
The formulas are identical across packs; what changes is the remaining-energy ceiling (~74,000 vs ~77,000 vs ~79,700 Wh), the number of valid cell/module fields, and the average-cell-voltage divisor. Ignore any cell index beyond your pack's real count (e.g. >180 on the 72.6 kWh, where the extra slots read 0 V). Note: exact module/cell counts per market pack vary slightly and should be verified against your own car's readout before hard-coding.
2025 MY / OTA shifts: community reports that some PIDs moved on the 2025 Ioniq 5 (e.g. 12V battery field returning blank, operating-hours counter suspect) and that the 2025 84 kWh "NEA" pack behaves differently. Treat 2025+ decoding as needing revalidation. Your 2021+ EU 72.6 kWh car is squarely covered by the mature 74 kWh definitions.
ICCU/session gotcha: if 7E5 PIDs return nothing, the ICCU likely needs a 10 03 extended session first; the BMS 7E4 PIDs never do.
6. Gotchas & warnings
12V drain is the real hazard. The DLC stays powered with the car off; a Bluetooth dongle that doesn't sleep will slowly flatten the 12V battery — and the Ioniq 5's 12V is a known weak point tied to a major recall. NHTSA Campaign No. 24V-204 (Hyundai internal Recall 257 / TSB RECALL 24-01-023H, March 2024) covers "Certain 2022-2024MY IONIQ 5 (NE1)… [with] a condition where low 12V auxiliary battery charging occurs due to an ICCU… and may set the following DTC P1A9096 – 'DC/DC Converter Input Voltage Sensor Fault'" (preceded by the earlier July 2023 Service Campaign 997). The scope is large: per WardsAuto (Nov 2024), Hyundai recalled ~145,000 electrified Ioniq/Genesis models (2022-2024 Ioniq 5, 2023-2025 Ioniq 6), following an earlier March recall of ~99,000; with a parallel ~63,000-unit Kia recall (incl. EV6) the E-GMP total is just over 208,000 vehicles. Owners specifically warn that always-on OBD dongles and API-polling apps that keep the car awake accelerate 12V depletion. Use a dongle that auto-sleeps (the OBDLink MX+ has low-power/sleep modes; STN STSLEEP/low-power config), or unplug when parked, or run off an external supply. Log the 12V with a cheap BM2-type monitor during your first long unattended session.
Plug/unplug wear: repeatedly hot-plugging the DLC risks connector damage; prefer a short extension pigtail if you cycle often.
DC fast-charge polling is safe and is the best time to capture current/voltage/power/temperature curves — the pack is active and every PID updates. There's no evidence that read-only Mode 22 polling interferes with charging.
Warranty / logging: read-only Mode 22 queries don't write to the car and are not known to set DTCs or be flagged; this is ordinary diagnostic traffic. (Contrast with write/actuation or bidirectional tools, which are a different risk class.) No evidence of read-polling voiding warranty.
ELM327 clones fail on E-GMP multiframe. Cheap clones with small buffers or broken flow-control mangle the 180+ cell frames; SoulEVSpy's docs explicitly warn that many Konnwei KW902 variants and low-buffer clones don't work. Your genuine OBDLink (STN-based) is the right tool. Also: don't run two apps against one dongle simultaneously — they collide and cause connect/disconnect loops.
"Unused PID returns garbage." If you request a slot the car doesn't populate, you get a stale/wrong value, not an error — validate lengths and ignore cell slots beyond your pack's real count.
7. Alternative / complementary data paths

Hyundai Bluelink cloud API (EU/Luxembourg available; Hyundai Bluelink is offered across Western Europe including Luxembourg):

Wrappers: bluelinky (Node.js, Hacksore) and hyundai_kia_connect_api (Python, Hyundai-Kia-Connect org; also feeds Home Assistant kia_uvo). EU login uses username/password with RSA-encrypted password (pycryptodome) and weekly "stamps."
What it gives: display SoC, charging state, range, odometer, location, remote climate/charge control — pulled from the last vehicle report, typically at coarse intervals (minutes), not sub-second, and display SoC only, not true BMS SoC, no per-cell voltages, no raw pack current.
What only OBD gives: true BMS SoC, per-cell voltages, min/max module temps, pack current/voltage at sub-second sampling, cumulative Ah/kWh counters, and instantaneous kW — essential for anything real-time (charging curves, efficiency, cell-balance diagnostics).
Critical caveat: Bluelink polling wakes the car and is itself a documented 12V-drain vector; the hyundai_kia_connect_monitor project (ZuinigeRijder) is explicitly designed around not draining the 12V. Rate limits also apply (roughly once/hour is safe in EU).

Bottom line on paths: OBD for high-fidelity live telemetry while driving/charging; Bluelink only as a low-rate, when-parked complement for location/coarse SoC — and even then, throttle it.

Recommendations
Start here (validate the transport in an afternoon): genuine OBDLink, car in READY or charging, run the ELM sequence in §3 and issue 2201 01 then 2201 05. Confirm you get 62 01 01… / 62 01 05… multiframe payloads. Decode SoC (byte 6 ÷2), current (bytes 12–13 signed ÷10), voltage (bytes 14–15 ÷10) first — these three are your ground truth and are triple-confirmed. If they're right, the rest of the table is trustworthy.
Pick your transport architecture: for a terminal-first Unix tool, either (a) drive the OBDLink over /dev/rfcomm* or USB with your own AT/ST state machine (use STPX on MX+/LX for speed), or (b) go SocketCAN + can-isotp with a native CAN adapter for clean ISO-TP. Model your polling loop and car-state ("OFF/AWAKE/DRIVE/CHARGE") gating on EVNotiPi's car-module structure — it's the closest existing design to what you're building.
Re-implement, don't copy: transcribe the §2 tables into your own MIT code. Treat OBDb (CC-BY-SA) and the GPL projects as reference, not source to lift. The facts (DIDs, offsets, scaling) are freely reusable.
Instrument for the 12V from day one: implement a sleep/back-off when CAN goes quiet, and default to not polling a parked car. Given the ICCU/12V recall history on this exact platform (NHTSA 24V-204, ~208k E-GMP vehicles), this is the single most important engineering decision.
For health tracking, log "remaining energy at 100% SOC" (§2), not just the sticky SoH PID; snapshot it at full charge across months. On your pack, ~74 kWh new is the reference.
Benchmarks that change the plan: if 2201 01 returns nothing → car isn't awake (or clone dongle); if only 7E5 fails → add a 10 03 extended session; if cell fields look garbled → your flow-control/STmin is wrong (raise ATST, set ATFCSD 30 00 00); if you're on a 2025+ or 84 kWh car → revalidate every offset against a known-good app before trusting it.
Caveats
Confirmation levels are marked in the tables. SoC(bms), current, voltage, SoH, display SoC, and the four cumulative counters are confirmed across OVMS + EVNotify + Torque community. Aux voltage/SoC, odometer, flags, and secondary temps are community-reported and corroborated but individually less triangulated.
Battery inlet temperature specifically: OVMS publishes an inlet-temp metric, but the exact byte offset within 22 01 05 could not be pinned to a verbatim source line; verify against a live car before coding it. The Esprit1st CSV author actually removed a separate inlet-temp field, exposing only module temps B01–B16 + heater-1.
Byte indices are derived (letter_position + 1) and validated on three fields; if any single field looks off, trust the OVMS C++ decoder's offsets over the letter translation.
Pack cell/module counts (58 kWh = 288 cells/24 modules; 72.6 kWh = 360 cells/30 modules/180s2p; 77.4 kWh = 384 cells/32 modules/192s2p) are from community teardown/forum sources and vary by market; verify against your own car's cell readout before hard-coding limits.
STPX exact syntax and CX support should be confirmed in the current OBDLink FRPM for your specific model/firmware.
Licenses above are best-effort and must be checked in each repo before reuse; several had no explicit LICENSE visible.
Some downstream/AI-generated "guides" about E-GMP SoH calculation circulating online are inaccurate; prefer the forum primary sources and the FOSS code.
