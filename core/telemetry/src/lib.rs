//! Transport-agnostic ELM327/ISO-TP dialogue engine (wayfinder #76): speaks
//! the ELM327/STN serial command protocol and reassembles ISO-TP
//! multi-frame UDS responses into complete payloads. No transport (BLE,
//! a CLI, or `replay`'s fixtures drive it with bytes) -- see
//! `docs/research/ioniq5-obd-telemetry.md` §2-3.
//!
//! `profile`/`decode` (wayfinder #77) build the Telemetry Profile-driven
//! decoder on top: loading/validating a `tprof-1` file, turning it into
//! this engine's poll list, and decoding its `Payload` events into
//! Canonical Signal readings. See `profile.rs` for the format and the
//! resolved bit-index origin convention (ADR 0013).

pub mod decode;
pub mod dialogue;
pub mod elm;
pub mod isotp;
pub mod profile;
pub mod replay;

pub use decode::{
    decode, requests_for, CanonicalReading, DecodeError, DecodeOutcome, DecodedValue, ExtraReading,
};
pub use dialogue::{EcuTarget, Engine, Event, FailReason, Request};
pub use profile::{
    CanonicalSignal, CommandDef, DecodeRule, PackVariant, ProfileError, TelemetryProfile,
    ValidationTier,
};
