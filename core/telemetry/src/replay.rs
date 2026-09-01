//! Text fixture replay harness (wayfinder #76) -- the engine's test spine
//! and the future vector-validation mechanism for Telemetry Profiles.
//! Drives an [`Engine`](crate::dialogue::Engine) through a scripted
//! adapter conversation without any real transport.
//!
//! Fixture format, one directive per line (blank lines and `#` comments
//! ignored):
//! - `tx <ascii>` -- asserts the engine's next outgoing write equals
//!   `<ascii>` plus a trailing CR.
//! - `rx <chunk>` -- feeds bytes to the engine. A literal `\r`/`\n`
//!   two-character escape in `<chunk>` is unescaped to 0x0D/0x0A (so
//!   fixtures can spell out CR-terminated adapter lines inline).
//! - `timeout` -- calls [`Engine::on_timeout`].

use crate::dialogue::{Engine, Event};

/// Runs `fixture` against `engine` in one shot per `rx` directive,
/// draining outgoing writes and events as it goes. Panics (with the
/// mismatched bytes) on a `tx` directive that doesn't match what the
/// engine actually produces, or if an outgoing write is still queued once
/// the fixture ends.
pub fn run_replay(engine: &mut Engine, fixture: &str) -> Vec<Event> {
    run(engine, fixture, None)
}

/// Like [`run_replay`], but delivers every `rx` directive's bytes to
/// [`Engine::feed`] in `chunk`-byte slices instead of all at once, to
/// exercise arbitrary transport splits.
pub fn run_replay_chunked(engine: &mut Engine, fixture: &str, chunk: usize) -> Vec<Event> {
    run(engine, fixture, Some(chunk.max(1)))
}

fn run(engine: &mut Engine, fixture: &str, chunk: Option<usize>) -> Vec<Event> {
    let mut events = Vec::new();
    for raw_line in fixture.lines() {
        let trimmed = raw_line.trim_end_matches('\r').trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some(ascii) = trimmed.strip_prefix("tx ") {
            assert_next_outgoing(engine, ascii);
        } else if let Some(chunk_text) = trimmed.strip_prefix("rx ") {
            let bytes = unescape(chunk_text);
            match chunk {
                Some(n) => {
                    for piece in bytes.chunks(n) {
                        engine.feed(piece);
                    }
                }
                None => engine.feed(&bytes),
            }
        } else if trimmed == "timeout" {
            engine.on_timeout();
        } else {
            panic!("replay fixture: unrecognized directive: {trimmed:?}");
        }
        drain(engine, &mut events);
    }
    if let Some(extra) = engine.take_outgoing() {
        panic!(
            "replay fixture ended with an unexpected outgoing write: {:?}",
            String::from_utf8_lossy(&extra)
        );
    }
    events
}

fn drain(engine: &mut Engine, events: &mut Vec<Event>) {
    while let Some(event) = engine.poll_event() {
        events.push(event);
    }
}

fn assert_next_outgoing(engine: &mut Engine, expected_ascii: &str) {
    let mut expected = expected_ascii.as_bytes().to_vec();
    expected.push(b'\r');
    match engine.take_outgoing() {
        Some(actual) if actual == expected => {}
        Some(actual) => panic!(
            "replay fixture: outgoing mismatch\n  expected: {:?}\n  actual:   {:?}",
            String::from_utf8_lossy(&expected),
            String::from_utf8_lossy(&actual)
        ),
        None => panic!(
            "replay fixture: expected outgoing write {expected_ascii:?}, engine had none ready"
        ),
    }
}

/// Unescapes `\r` and `\n` two-character sequences to their raw bytes;
/// anything else (including a lone backslash) passes through unchanged.
fn unescape(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('r') => {
                    chars.next();
                    out.push(b'\r');
                    continue;
                }
                Some('n') => {
                    chars.next();
                    out.push(b'\n');
                    continue;
                }
                _ => {}
            }
        }
        let mut buf = [0u8; 4];
        out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
    }
    out
}
