//! The engine state machine (wayfinder #76): a sans-IO ELM327 dialogue
//! driver. Bytes in, bytes/events out -- the caller (BLE, a CLI, or the
//! replay harness in `crate::replay`) owns the actual transport and read
//! timeout. Operating mode is fixed: headers on, auto-formatting off
//! (`ATH1`+`ATCAF0`), so `crate::elm` hands back raw CAN frame lines that
//! `crate::isotp` reassembles here. See
//! `docs/research/ioniq5-obd-telemetry.md` §2-3.

use std::collections::VecDeque;
use std::fmt::Write as _;

use crate::elm::{classify_line, Line, LineBuffer, Token};
use crate::isotp::{encode_single_frame, IsoTpError, Reassembler};

/// One ECU's request/response CAN ids, e.g. `0x7E4`/`0x7EC` for the BMS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EcuTarget {
    pub tx_header: u16,
    pub rx_header: u16,
}

/// One UDS request to poll, e.g. `uds: vec![0x22, 0x01, 0x01]`.
#[derive(Debug, Clone)]
pub struct Request {
    pub target: EcuTarget,
    pub uds: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The init sequence completed.
    AdapterReady,
    /// A full reassembled positive response (starts `0x62`).
    Payload { target: EcuTarget, uds: Vec<u8> },
    /// `uds` echoes the request that failed (mirrors [`Request`]), not any
    /// partial response bytes.
    Failed {
        target: EcuTarget,
        uds: Vec<u8>,
        reason: FailReason,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailReason {
    NoData,
    CanError,
    Stopped,
    Timeout,
    Negative { nrc: u8 },
    IsoTp(IsoTpError),
    Adapter(String),
}

const INIT_COMMANDS: [&str; 7] = ["ATZ", "ATE0", "ATL0", "ATS0", "ATH1", "ATCAF0", "ATSP6"];

/// One step of the fully precomputed command plan. Which requests need
/// per-target setup is decided once, up front, from consecutive targets in
/// the input order (see [`Engine::new`]) -- nothing about which frames
/// actually come back changes what gets sent next.
enum PlanStep {
    /// An init command other than the last.
    Init(String),
    /// The last init command; its prompt emits [`Event::AdapterReady`].
    InitLast(String),
    /// One per-target setup command (`ATSH`/`ATCRA`/`ATFCSH`/`ATFCSD`/`ATFCSM`).
    Setup(String),
    /// Send `requests[usize]` and collect its response.
    SendRequest(usize),
}

/// The ELM/ISO-TP dialogue engine. Construct with the full batch of
/// requests to run, in order; drive it with [`take_outgoing`](Self::take_outgoing),
/// [`feed`](Self::feed), and [`on_timeout`](Self::on_timeout), and collect
/// results with [`poll_event`](Self::poll_event).
pub struct Engine {
    requests: Vec<Request>,
    plan: VecDeque<PlanStep>,
    /// The step already sent, awaiting its concluding `>` prompt.
    sending: Option<PlanStep>,
    /// Live only while `sending` is a `SendRequest` whose outcome (payload
    /// or failure) hasn't been decided yet.
    collector: Option<Reassembler>,
    /// ASCII text (no CR) of the most recently sent command, for echo
    /// detection.
    last_sent: Option<String>,
    lines: LineBuffer,
    events: VecDeque<Event>,
    finished: bool,
}

impl Engine {
    pub fn new(requests: Vec<Request>) -> Engine {
        let mut plan = VecDeque::new();
        let last_init = INIT_COMMANDS.len() - 1;
        for (idx, cmd) in INIT_COMMANDS.iter().enumerate() {
            let text = (*cmd).to_string();
            plan.push_back(if idx == last_init {
                PlanStep::InitLast(text)
            } else {
                PlanStep::Init(text)
            });
        }

        let mut established: Option<EcuTarget> = None;
        for (i, req) in requests.iter().enumerate() {
            if established != Some(req.target) {
                for cmd in setup_commands(req.target) {
                    plan.push_back(PlanStep::Setup(cmd));
                }
                established = Some(req.target);
            }
            plan.push_back(PlanStep::SendRequest(i));
        }

        Engine {
            requests,
            plan,
            sending: None,
            collector: None,
            last_sent: None,
            lines: LineBuffer::new(),
            events: VecDeque::new(),
            finished: false,
        }
    }

    /// Next bytes to write (ASCII command + trailing CR), or `None` if
    /// nothing is ready: either the previous command's prompt hasn't
    /// arrived yet (strict command/prompt lockstep), or the engine is
    /// finished.
    pub fn take_outgoing(&mut self) -> Option<Vec<u8>> {
        if self.finished || self.sending.is_some() {
            return None;
        }
        loop {
            let Some(step) = self.plan.pop_front() else {
                self.finished = true;
                return None;
            };
            let text = match &step {
                PlanStep::Init(s) | PlanStep::InitLast(s) | PlanStep::Setup(s) => s.clone(),
                PlanStep::SendRequest(i) => match encode_single_frame(&self.requests[*i].uds) {
                    Ok(frame) => to_hex_upper(&frame),
                    Err(e) => {
                        self.emit_failed(*i, FailReason::IsoTp(e));
                        continue;
                    }
                },
            };
            self.last_sent = Some(text.clone());
            self.collector = if matches!(step, PlanStep::SendRequest(_)) {
                Some(Reassembler::new())
            } else {
                None
            };
            self.sending = Some(step);
            let mut bytes = text.into_bytes();
            bytes.push(b'\r');
            return Some(bytes);
        }
    }

    /// Bytes read from the transport.
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.finished {
            return;
        }
        let tokens = self.lines.feed(bytes);
        for token in tokens {
            match token {
                Token::Line(text) => self.handle_line(&text),
                Token::Prompt => self.handle_prompt(),
            }
            if self.finished {
                break;
            }
        }
    }

    /// Drains one completed event, oldest first.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The transport's read timeout expired. Meaning depends on what's in
    /// flight: mid-response, it concludes the request as `Failed{Timeout}`
    /// and starts waiting for a possibly-late prompt (a second call here
    /// abandons that wait and moves on); mid-setup, there's no response
    /// content to fail, so it just abandons the wait; mid-init, it fails
    /// the whole engine (synthetic all-zero target -- an init timeout
    /// isn't about any one ECU).
    pub fn on_timeout(&mut self) {
        if self.finished {
            return;
        }
        let Some(step) = self.sending.as_ref() else {
            return;
        };
        if matches!(step, PlanStep::Init(_) | PlanStep::InitLast(_)) {
            self.abort_init_timeout();
            return;
        }
        if let PlanStep::SendRequest(i) = step {
            let i = *i;
            if self.collector.is_some() {
                self.collector = None;
                self.emit_failed(i, FailReason::Timeout);
                return;
            }
        }
        self.sending = None;
        self.advance(false);
    }

    /// All requests have concluded (or the engine aborted during init).
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    fn handle_line(&mut self, text: &str) {
        let i = match &self.sending {
            Some(PlanStep::SendRequest(i)) => *i,
            _ => return,
        };
        let Some(reassembler) = self.collector.as_mut() else {
            return;
        };
        let target = self.requests[i].target;
        match classify_line(text, self.last_sent.as_deref()) {
            Line::Frame(frame) if frame.id == target.rx_header => {
                match reassembler.feed_frame(&frame.data) {
                    Ok(Some(payload)) => self.conclude_payload(i, payload),
                    Ok(None) => {}
                    Err(e) => self.emit_failed(i, FailReason::IsoTp(e)),
                }
            }
            Line::Frame(_) => {}
            Line::NoData => self.emit_failed(i, FailReason::NoData),
            Line::CanError | Line::BusInit => self.emit_failed(i, FailReason::CanError),
            Line::Stopped => self.emit_failed(i, FailReason::Stopped),
            Line::Ok
            | Line::QuestionMark
            | Line::Searching
            | Line::Echo
            | Line::Blank
            | Line::Banner(_) => {}
        }
    }

    fn handle_prompt(&mut self) {
        let Some(step) = self.sending.take() else {
            return;
        };
        if let PlanStep::SendRequest(i) = step {
            if self.collector.take().is_some() {
                // The prompt arrived before any outcome was determined --
                // conclude with a diagnostic failure rather than leaving
                // the request (and the whole lockstep engine) stuck.
                self.emit_failed(
                    i,
                    FailReason::Adapter("prompt with no response".to_string()),
                );
            }
            self.advance(false);
        } else {
            let was_last_init = matches!(step, PlanStep::InitLast(_));
            self.advance(was_last_init);
        }
    }

    fn advance(&mut self, emit_ready: bool) {
        if emit_ready {
            self.events.push_back(Event::AdapterReady);
        }
        if self.plan.is_empty() {
            self.finished = true;
        }
    }

    fn abort_init_timeout(&mut self) {
        self.plan.clear();
        self.sending = None;
        self.collector = None;
        self.finished = true;
        self.events.push_back(Event::Failed {
            target: EcuTarget {
                tx_header: 0,
                rx_header: 0,
            },
            uds: Vec::new(),
            reason: FailReason::Adapter("init timeout".to_string()),
        });
    }

    fn emit_failed(&mut self, i: usize, reason: FailReason) {
        let req = &self.requests[i];
        self.events.push_back(Event::Failed {
            target: req.target,
            uds: req.uds.clone(),
            reason,
        });
        self.collector = None;
    }

    fn conclude_payload(&mut self, i: usize, payload: Vec<u8>) {
        let req = &self.requests[i];
        let target = req.target;
        let uds = req.uds.clone();
        // `req.uds` is always 1..=7 bytes here: an empty or oversized
        // payload would already have failed in `take_outgoing` (via
        // `encode_single_frame`) before this request was ever sent.
        let positive_sid = req.uds[0].wrapping_add(0x40);
        let event = if payload.first() == Some(&0x7F) {
            match payload.get(2) {
                Some(&nrc) => Event::Failed {
                    target,
                    uds,
                    reason: FailReason::Negative { nrc },
                },
                None => Event::Failed {
                    target,
                    uds,
                    reason: FailReason::Adapter(format!("short negative response {payload:02X?}")),
                },
            }
        } else if payload.first() == Some(&positive_sid) {
            Event::Payload {
                target,
                uds: payload,
            }
        } else {
            Event::Failed {
                target,
                uds,
                reason: FailReason::Adapter(format!("unexpected response {payload:02X?}")),
            }
        };
        self.events.push_back(event);
        self.collector = None;
    }
}

fn setup_commands(target: EcuTarget) -> [String; 5] {
    [
        format!("ATSH{:03X}", target.tx_header),
        format!("ATCRA{:03X}", target.rx_header),
        format!("ATFCSH{:03X}", target.tx_header),
        "ATFCSD300000".to_string(),
        "ATFCSM1".to_string(),
    ]
}

fn to_hex_upper(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02X}");
    }
    s
}
