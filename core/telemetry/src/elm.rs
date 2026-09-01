//! ELM327 line protocol (wayfinder #76): incremental CR/LF tokenizing of
//! arbitrary byte chunks (a BLE transport may split anywhere) and
//! classification of the resulting lines under the engine's fixed
//! configuration -- headers on, auto-formatting off (`ATH1`+`ATCAF0`), so
//! a data response is a raw `<id><data hex>` line rather than a
//! pre-reassembled payload. See `docs/research/ioniq5-obd-telemetry.md` §3.

/// One tokenized unit out of [`LineBuffer::feed`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// Text between two terminators (CR and/or LF, both consumed), or
    /// between a terminator/start-of-input and a `>` prompt.
    Line(String),
    /// The adapter's `>` prompt, which may arrive with no terminator
    /// before or after it.
    Prompt,
}

/// Incremental CR/LF tokenizer. Bytes are buffered across calls to
/// [`feed`](Self::feed) so a transport chunk boundary may split a line,
/// a hex pair, or land right before/after a prompt without losing data.
/// Invalid UTF-8 is replaced (never panics), since the buffered bytes are
/// only ever adapter text or hex digits.
#[derive(Default)]
pub struct LineBuffer {
    buf: Vec<u8>,
}

impl LineBuffer {
    pub fn new() -> Self {
        LineBuffer::default()
    }

    /// Feeds one chunk of bytes as read from the transport, returning
    /// every line and prompt sighting it completed, in order. CR and LF
    /// are both accepted as terminators and stripped; a `>` flushes any
    /// buffered text as its own `Line` first (if non-empty), then yields
    /// `Prompt`.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Token> {
        let mut tokens = Vec::new();
        for &b in bytes {
            match b {
                b'\r' | b'\n' => {
                    tokens.push(Token::Line(self.take_line()));
                }
                b'>' => {
                    if !self.buf.is_empty() {
                        tokens.push(Token::Line(self.take_line()));
                    }
                    tokens.push(Token::Prompt);
                }
                _ => self.buf.push(b),
            }
        }
        tokens
    }

    fn take_line(&mut self) -> String {
        let line = String::from_utf8_lossy(&self.buf).into_owned();
        self.buf.clear();
        line
    }
}

/// One 11-bit CAN data frame parsed from a header-on, auto-format-off
/// response line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u16,
    pub data: Vec<u8>,
}

/// A classified response line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    Frame(Frame),
    Ok,
    QuestionMark,
    NoData,
    CanError,
    /// `BUS INIT` and anything after it (e.g. `BUS INIT: OK`, `BUS INIT: ERROR`).
    BusInit,
    Stopped,
    /// `SEARCHING...` -- informational only.
    Searching,
    /// Equal to the last command this engine sent: echo is on until ATE0
    /// takes effect.
    Echo,
    /// A bare terminator with nothing between it and the previous one.
    Blank,
    /// Anything else: version banners (`ELM327 v1.5`) and other adapter
    /// text the engine takes no action on.
    Banner(String),
}

/// Classifies one tokenized line. `last_sent` is the ASCII command text
/// (no trailing CR) this engine most recently wrote, used only for echo
/// detection.
pub fn classify_line(line: &str, last_sent: Option<&str>) -> Line {
    let stripped: String = line.chars().filter(|&c| c != ' ').collect();

    if stripped.is_empty() {
        return Line::Blank;
    }
    if last_sent == Some(stripped.as_str()) {
        return Line::Echo;
    }
    if let Some(frame) = parse_frame(&stripped) {
        return Line::Frame(frame);
    }
    match stripped.as_str() {
        "OK" => Line::Ok,
        "?" => Line::QuestionMark,
        "NODATA" => Line::NoData,
        "CANERROR" => Line::CanError,
        "STOPPED" => Line::Stopped,
        "SEARCHING..." => Line::Searching,
        _ if stripped.starts_with("BUSINIT") => Line::BusInit,
        _ => Line::Banner(line.to_string()),
    }
}

/// 3 hex digits (CAN id) followed by 2-16 hex digits in pairs (data bytes).
fn parse_frame(stripped: &str) -> Option<Frame> {
    let bytes = stripped.as_bytes();
    if bytes.len() < 3 {
        return None;
    }
    let (id_bytes, data_bytes) = bytes.split_at(3);
    if !id_bytes.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    let data_len = data_bytes.len();
    if !(2..=16).contains(&data_len) || data_len % 2 != 0 {
        return None;
    }
    if !data_bytes.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }

    let id_str = std::str::from_utf8(id_bytes).ok()?;
    let id = u16::from_str_radix(id_str, 16).ok()?;
    let mut data = Vec::with_capacity(data_len / 2);
    for pair in data_bytes.as_chunks::<2>().0 {
        let s = std::str::from_utf8(pair).ok()?;
        data.push(u8::from_str_radix(s, 16).ok()?);
    }
    Some(Frame { id, data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_buffer_splits_on_cr_and_strips_it() {
        let mut lb = LineBuffer::new();
        let tokens = lb.feed(b"ATZ\r\rELM327 v1.5\r");
        assert_eq!(
            tokens,
            vec![
                Token::Line("ATZ".to_string()),
                Token::Line(String::new()),
                Token::Line("ELM327 v1.5".to_string()),
            ]
        );
    }

    #[test]
    fn line_buffer_tolerates_lf_as_terminator() {
        let mut lb = LineBuffer::new();
        assert_eq!(lb.feed(b"OK\n"), vec![Token::Line("OK".to_string())]);
    }

    #[test]
    fn line_buffer_prompt_without_terminator_flushes_pending_text() {
        let mut lb = LineBuffer::new();
        let tokens = lb.feed(b"7EC1062");
        assert_eq!(tokens, vec![]);
        let tokens = lb.feed(b">");
        assert_eq!(
            tokens,
            vec![Token::Line("7EC1062".to_string()), Token::Prompt]
        );
    }

    #[test]
    fn line_buffer_prompt_with_no_pending_text_yields_bare_prompt() {
        let mut lb = LineBuffer::new();
        lb.feed(b"OK\r\r");
        let tokens = lb.feed(b">");
        assert_eq!(tokens, vec![Token::Prompt]);
    }

    #[test]
    fn line_buffer_handles_chunk_split_mid_hex_pair() {
        let mut lb = LineBuffer::new();
        assert_eq!(lb.feed(b"7EC21A"), vec![]);
        let tokens = lb.feed(b"A\r");
        assert_eq!(tokens, vec![Token::Line("7EC21AA".to_string())]);
    }

    #[test]
    fn classify_line_parses_frame_tolerating_spaces() {
        assert_eq!(
            classify_line("7EC 10 3E 62 01 01", None),
            Line::Frame(Frame {
                id: 0x7EC,
                data: vec![0x10, 0x3E, 0x62, 0x01, 0x01],
            })
        );
    }

    #[test]
    fn classify_line_recognizes_adapter_strings() {
        assert_eq!(classify_line("OK", None), Line::Ok);
        assert_eq!(classify_line("?", None), Line::QuestionMark);
        assert_eq!(classify_line("NO DATA", None), Line::NoData);
        assert_eq!(classify_line("CAN ERROR", None), Line::CanError);
        assert_eq!(classify_line("STOPPED", None), Line::Stopped);
        assert_eq!(classify_line("SEARCHING...", None), Line::Searching);
        assert_eq!(classify_line("BUS INIT: OK", None), Line::BusInit);
    }

    #[test]
    fn classify_line_detects_echo_against_last_sent() {
        assert_eq!(classify_line("ATE0", Some("ATE0")), Line::Echo);
        assert_eq!(classify_line("OK", Some("ATE0")), Line::Ok);
    }

    #[test]
    fn classify_line_falls_back_to_banner() {
        assert_eq!(
            classify_line("ELM327 v1.5", None),
            Line::Banner("ELM327 v1.5".to_string())
        );
    }
}
