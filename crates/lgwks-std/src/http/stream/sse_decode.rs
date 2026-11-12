//! The server-sent event decoder with no I/O: bytes in, events out.
//!
//! [`SseStream`](super::SseStream) feeds it whatever each read returned; a
//! test feeds it the same bytes split at every point, which is how framing is
//! proven independent of the network's segmentation (#913 item 2: a `\r\n`
//! split across two reads used to end two lines and dispatch early).
//!
//! Framing follows the WHATWG HTML event-stream grammar: lines end at `\n`,
//! `\r\n` or `\r`; a blank line dispatches; a line opening with `:` is a
//! comment; one leading byte-order mark is stripped. Dispatch follows the
//! same section: a block with no `data` field fires nothing and resets the
//! event type, while an `id` it carries still becomes the last-seen ID.

use super::super::{Error, FailureCause, FailureKind, FailureStage};
use super::SseEvent;

/// An event stream decoder: line framing plus the event being accumulated.
pub(super) struct SseDecoder {
    /// The line framing.
    lines: SseLines,
    /// The fields of the event being accumulated.
    fields: SseFields,
}

impl SseDecoder {
    /// An empty decoder refusing any line longer than `line_cap` bytes.
    pub(super) fn new(line_cap: usize) -> Self {
        Self {
            lines: SseLines::new(line_cap),
            fields: SseFields::default(),
        }
    }

    /// Append the bytes one read returned.
    pub(super) fn push(&mut self, bytes: &[u8]) {
        self.lines.push(bytes);
    }

    /// The next event the bytes pushed so far complete, if any.
    pub(super) fn next_event(&mut self) -> Result<Option<SseEvent>, Error> {
        while let Some(span) = self.lines.next_line()? {
            let line = self.lines.bytes.get(span).ok_or(Error::Failure {
                stage: FailureStage::Body,
                kind: FailureKind::Transport,
                cause: None,
            })?;
            if let Some(event) = self.fields.feed(line)? {
                return Ok(Some(event));
            }
        }
        Ok(None)
    }

    /// The stream ended: dispatch a pending event that carries data rather
    /// than lose it. An unterminated last line is not a line and is dropped.
    pub(super) fn finish(&mut self) -> Option<SseEvent> {
        self.fields.dispatch()
    }
}

/// The UTF-8 byte-order mark a stream may open with.
const BYTE_ORDER_MARK: [u8; 3] = [0xEF, 0xBB, 0xBF];

/// The line framing of an event stream.
///
/// Consumed lines are not drained one by one: `start` moves past them and the
/// buffer compacts once per [`push`](Self::push), so a read carrying many
/// lines costs one move. `scanned` remembers how far the unconsumed bytes are
/// known to hold no terminator, so a long line arriving over many reads is
/// scanned once, not once per read (the old reader rescanned from the start
/// on every read, quadratic in the line length).
struct SseLines {
    /// Bytes received; those before `start` are consumed.
    bytes: Vec<u8>,
    /// The first unconsumed byte.
    start: usize,
    /// The bytes before this index are known to hold no line terminator.
    scanned: usize,
    /// The last line ended at a `\r` that was the last byte held, so a `\n`
    /// opening the next read belongs to that same line ending.
    after_cr: bool,
    /// Whether the leading byte-order mark has been judged.
    started: bool,
    /// The per-line ceiling: the exchange's `max_body_bytes`.
    cap: usize,
    /// Bytes examined for a terminator, for the linear-scan proof.
    #[cfg(test)]
    examined: usize,
}

impl SseLines {
    /// No bytes yet.
    fn new(cap: usize) -> Self {
        Self {
            bytes: Vec::new(),
            start: 0,
            scanned: 0,
            after_cr: false,
            started: false,
            cap,
            #[cfg(test)]
            examined: 0,
        }
    }

    /// Append `input`, compacting what earlier lines consumed first.
    fn push(&mut self, input: &[u8]) {
        if self.start > 0 {
            self.bytes.drain(..self.start);
            self.scanned = self.scanned.saturating_sub(self.start);
            self.start = 0;
        }
        self.bytes.extend_from_slice(input);
        if !self.started {
            self.judge_mark();
        }
    }

    /// Strip one leading byte-order mark once enough bytes say whether the
    /// stream opens with one. Nothing is framed until this is decided.
    fn judge_mark(&mut self) {
        if self.bytes.starts_with(&BYTE_ORDER_MARK) {
            self.start = BYTE_ORDER_MARK.len();
            self.scanned = self.start;
            self.started = true;
        } else if !BYTE_ORDER_MARK.starts_with(&self.bytes) {
            self.started = true;
        }
    }

    /// The next complete line, without its terminator, as a span of `bytes`.
    ///
    /// A line longer than the line cap is refused outright: a line is only
    /// deliverable whole, so previewing it would hand the caller a fragment
    /// shaped like an event.
    fn next_line(&mut self) -> Result<Option<std::ops::Range<usize>>, Error> {
        if !self.started {
            return Ok(None);
        }
        if self.after_cr {
            match self.bytes.get(self.start) {
                None => return Ok(None),
                Some(&b'\n') => self.start = self.start.saturating_add(1),
                Some(_) => {}
            }
            self.after_cr = false;
            self.scanned = self.scanned.max(self.start);
        }
        let from = self.scanned.max(self.start);
        let unscanned = match self.bytes.get(from..) {
            Some(unscanned) => unscanned,
            None => &[],
        };
        #[cfg(test)]
        {
            self.examined = self.examined.saturating_add(unscanned.len());
        }
        let found = unscanned
            .iter()
            .position(|byte| *byte == b'\n' || *byte == b'\r');
        let Some(offset) = found else {
            self.scanned = self.bytes.len();
            self.within_cap(self.bytes.len())?;
            return Ok(None);
        };
        let end = from.saturating_add(offset);
        self.within_cap(end)?;
        let mut next = end.saturating_add(1);
        if self.bytes.get(end) == Some(&b'\r') {
            match self.bytes.get(next) {
                Some(&b'\n') => next = next.saturating_add(1),
                Some(_) => {}
                None => self.after_cr = true,
            }
        }
        let line = self.start..end;
        self.start = next;
        self.scanned = next;
        Ok(Some(line))
    }

    /// Refuse when the line from `start` to `end` is longer than the cap.
    fn within_cap(&self, end: usize) -> Result<(), Error> {
        let length = end.saturating_sub(self.start);
        if length > self.cap {
            let refusal = Err(Error::BodyTooLarge { limit: self.cap });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                length,
                error = ?refusal.as_ref().err(),
                "next_line: a line passed its ceiling"
            );
            return refusal;
        }
        Ok(())
    }
}

/// The fields of the event being accumulated, and the last-seen ID.
#[derive(Default)]
struct SseFields {
    /// The event type being accumulated, if named.
    event: Option<String>,
    /// The data lines accumulated so far, joined with `\n` as they arrive.
    data: String,
    /// Whether a `data` field has been seen since the last dispatch.
    has_data: bool,
    /// The last-seen ID, which persists across events.
    id: Option<String>,
    /// A reconnection time not yet carried by a dispatched event.
    retry: Option<u64>,
}

impl SseFields {
    /// Feed one line into the pending event, dispatching on a blank line.
    fn feed(&mut self, line: &[u8]) -> Result<Option<SseEvent>, Error> {
        if line.is_empty() {
            return Ok(self.dispatch());
        }
        if line.starts_with(b":") {
            return Ok(None);
        }
        let text = match std::str::from_utf8(line) {
            Ok(text) => text,
            Err(utf8_error) => {
                let refusal = Err(Error::Failure {
                    stage: FailureStage::TextDecode,
                    kind: FailureKind::InvalidUtf8,
                    cause: Some(FailureCause::sanitized(format!(
                        "valid UTF-8 ends at byte {}",
                        utf8_error.valid_up_to()
                    ))),
                });
                #[cfg(feature = "trace")]
                crate::trace::debug!(
                    error = ?refusal.as_ref().err(),
                    "feed: an event line is not UTF-8"
                );
                return refusal;
            }
        };
        let (field, value) = match text.split_once(':') {
            Some((field, rest)) => match rest.strip_prefix(' ') {
                Some(value) => (field, value),
                None => (field, rest),
            },
            None => (text, ""),
        };
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => {
                if self.has_data {
                    self.data.push('\n');
                }
                self.data.push_str(value);
                self.has_data = true;
            }
            "id" => {
                if !value.contains('\0') {
                    self.id = Some(value.to_owned());
                }
            }
            "retry" if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) => {
                if let Ok(millis) = value.parse::<u64>() {
                    self.retry = Some(millis);
                }
            }
            _ => {}
        }
        Ok(None)
    }

    /// Dispatch the pending event, or reset the event type when no `data`
    /// field arrived: only data fires an event. The last-seen ID persists, and
    /// a reconnection time with no event to ride on waits for the next one.
    fn dispatch(&mut self) -> Option<SseEvent> {
        if !self.has_data {
            self.event = None;
            return None;
        }
        self.has_data = false;
        let event = match self.event.take() {
            Some(event) if !event.is_empty() => event,
            Some(_) | None => "message".to_owned(),
        };
        Some(SseEvent {
            event,
            data: std::mem::take(&mut self.data),
            id: self.id.clone(),
            retry: self.retry.take(),
        })
    }
}

#[cfg(test)]
mod sim_sse;
