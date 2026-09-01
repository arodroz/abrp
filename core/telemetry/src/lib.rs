//! Transport-agnostic ELM327/ISO-TP dialogue engine (wayfinder #76): speaks
//! the ELM327/STN serial command protocol and reassembles ISO-TP
//! multi-frame UDS responses into complete payloads. No transport (BLE,
//! a CLI, or `replay`'s fixtures drive it with bytes) and no signal
//! decoding (a later ticket) -- see
//! `docs/research/ioniq5-obd-telemetry.md` §2-3.

pub mod dialogue;
pub mod elm;
pub mod isotp;
pub mod replay;

pub use dialogue::{EcuTarget, Engine, Event, FailReason, Request};
