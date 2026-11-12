//! Seeded simulations of the event-stream decoder against a model.
//!
//! Source: the WHATWG HTML standard, section 9.2.6 "Interpreting an event
//! stream", and #913 item 2 (a `\r\n` split across two reads dispatched an
//! event early; an `id`-only block dispatched an empty `message`). The model
//! below is written from that section, not from the decoder: it parses the
//! whole stream in one pass. Every family feeds the decoder the same bytes cut
//! at every point (or at seeded points) and requires the model's events, so a
//! framing that depends on the network's segmentation fails here by seed.

use super::SseDecoder;
use crate::http::Error;
use crate::seeded::Seeded;

/// One dispatched event as compared: type, data, last-seen ID, retry.
type Seen = (String, String, Option<String>, Option<u64>);

/// What a test returns: any failure, named.
type Outcome = Result<(), Box<dyn std::error::Error>>;

/// The per-line cap every family but the cap family runs under.
const WIDE_CAP: usize = 1 << 20;

/// Seeds per seeded family.
const SEEDS: u64 = 256;

/// Decode `pieces` as successive reads, then end the stream.
fn decode(pieces: &[&[u8]], cap: usize) -> Result<Vec<Seen>, Error> {
    let mut decoder = SseDecoder::new(cap);
    let mut seen = Vec::new();
    for piece in pieces {
        decoder.push(piece);
        while let Some(event) = decoder.next_event()? {
            seen.push(view(&event));
        }
    }
    if let Some(event) = decoder.finish() {
        seen.push(view(&event));
    }
    Ok(seen)
}

/// An event's fields as one comparable value.
fn view(event: &super::SseEvent) -> Seen {
    (
        event.event().to_owned(),
        event.data().to_owned(),
        event.id().map(str::to_owned),
        event.retry(),
    )
}

/// The specification's reading of a whole stream, in one pass.
///
/// Lines end at CRLF, LF or CR; a final unterminated line is dropped; one
/// leading byte-order mark is stripped. A pending block with data at the end
/// of the stream dispatches, which is this crate's documented contract
/// (`sse_dispatches_a_pending_event_at_eof`) where the standard discards it.
fn model(input: &[u8]) -> Vec<Seen> {
    let body = match input.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        Some(rest) => rest,
        None => input,
    };
    let mut lines: Vec<&[u8]> = Vec::new();
    let mut from = 0_usize;
    let mut at = 0_usize;
    while let Some(&byte) = body.get(at) {
        if byte == b'\n' || byte == b'\r' {
            if let Some(line) = body.get(from..at) {
                lines.push(line);
            }
            at = at.saturating_add(1);
            if byte == b'\r' && body.get(at) == Some(&b'\n') {
                at = at.saturating_add(1);
            }
            from = at;
        } else {
            at = at.saturating_add(1);
        }
    }
    let mut state = ModelState::default();
    for line in lines {
        state.line(&String::from_utf8_lossy(line));
    }
    state.line("");
    state.events
}

/// The model's buffers, named as the standard names them.
#[derive(Default)]
struct ModelState {
    /// The event type buffer.
    event_type: String,
    /// The data buffer, each value followed by a line feed.
    data: String,
    /// The last event ID buffer.
    last_id: Option<String>,
    /// A reconnection time not yet carried by an event.
    retry: Option<u64>,
    /// Events dispatched so far.
    events: Vec<Seen>,
}

impl ModelState {
    /// Process one line.
    fn line(&mut self, line: &str) {
        if line.is_empty() {
            self.dispatch();
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((field, rest)) => match rest.strip_prefix(' ') {
                Some(value) => (field, value),
                None => (field, rest),
            },
            None => (line, ""),
        };
        match field {
            "event" => value.clone_into(&mut self.event_type),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
            }
            "id" if !value.contains('\0') => self.last_id = Some(value.to_owned()),
            "retry" if !value.is_empty() && value.bytes().all(|digit| digit.is_ascii_digit()) => {
                if let Ok(millis) = value.parse() {
                    self.retry = Some(millis);
                }
            }
            _ => {}
        }
    }

    /// The standard's dispatch steps, with the retry riding on the event.
    fn dispatch(&mut self) {
        if self.data.is_empty() {
            self.event_type.clear();
            return;
        }
        if self.data.ends_with('\n') {
            self.data.pop();
        }
        let event = if self.event_type.is_empty() {
            "message".to_owned()
        } else {
            std::mem::take(&mut self.event_type)
        };
        let data = std::mem::take(&mut self.data);
        self.events
            .push((event, data, self.last_id.clone(), self.retry.take()));
    }
}

/// A uniform pick from `items`, `None` only for an empty slice.
fn pick<'items, T>(rng: &mut Seeded, items: &'items [T]) -> Option<&'items T> {
    let bound = u64::try_from(items.len()).ok()?;
    let index = usize::try_from(rng.below(bound).ok()?).ok()?;
    items.get(index)
}

/// A seeded stream: lines from the grammar's whole vocabulary, every line
/// ending, an optional byte-order mark and an optional unterminated tail.
fn stream(seed: u64) -> Option<Vec<u8>> {
    const LINES: [&str; 16] = [
        "data: a",
        "data:b",
        "data",
        "data:  two spaces",
        "data: x:y",
        "event: tick",
        "event:",
        "id: 7",
        "id",
        "retry: 25",
        "retry: +5",
        "retry: 9x",
        ": comment",
        "bogus: field",
        "",
        "data: é",
    ];
    const ENDINGS: [&str; 3] = ["\n", "\r\n", "\r"];
    let mut rng = Seeded::from_seed(seed);
    let mut out = Vec::new();
    if *pick(&mut rng, &[false, false, true])? {
        out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
    }
    let count = usize::try_from(rng.below(24).ok()?).ok()?;
    for _ in 0..count {
        out.extend_from_slice(pick(&mut rng, &LINES)?.as_bytes());
        out.extend_from_slice(pick(&mut rng, &ENDINGS)?.as_bytes());
    }
    if *pick(&mut rng, &[false, true])? {
        out.extend_from_slice(b"data: unterminated");
    }
    Some(out)
}

/// Cut `input` at `points` (sorted, in range) into successive reads.
fn cut<'input>(input: &'input [u8], points: &[usize]) -> Vec<&'input [u8]> {
    let mut pieces = Vec::with_capacity(points.len().saturating_add(1));
    let mut from = 0_usize;
    for &point in points {
        if let Some(piece) = input.get(from..point) {
            pieces.push(piece);
        }
        from = point;
    }
    if let Some(piece) = input.get(from..) {
        pieces.push(piece);
    }
    pieces
}

/// The issue's own case: a CRLF whose two bytes arrive in two reads ends one
/// line, so two data lines and a blank line are one event, not two.
#[test]
fn a_crlf_split_across_two_reads_is_one_line_ending() -> Outcome {
    let input = b"data: a\r\ndata: b\r\n\r\n";
    let split = decode(&cut(input, &[8]), WIDE_CAP)?;
    assert_eq!(
        split,
        vec![("message".to_owned(), "a\nb".to_owned(), None, None)],
        "a CR at the end of one read and its LF in the next are one line ending"
    );
    Ok(())
}

/// Every pair of split points of a stream holding every line ending decodes
/// exactly as the whole stream does, and as the model does.
#[test]
fn every_split_of_every_line_ending_decodes_like_the_whole_stream() -> Outcome {
    let input = b"event: e\rdata: 1\r\ndata: 2\n\r\nid: 9\r\rdata: 3\r\n\n: c\r\ndata: 4\r";
    let expected = model(input);
    assert_eq!(expected.len(), 3, "the model reads three events");
    assert_eq!(decode(&[input], WIDE_CAP)?, expected, "whole stream");
    for first in 0..=input.len() {
        for second in first..=input.len() {
            let got = decode(&cut(input, &[first, second]), WIDE_CAP)?;
            assert_eq!(got, expected, "cut at {first} and {second}");
        }
    }
    Ok(())
}

/// Every seeded stream, cut at every single point, at seeded points and one
/// byte at a time, decodes as the model reads it.
#[test]
fn sim_every_seeded_stream_at_every_split_matches_the_model() -> Outcome {
    for seed in 0..SEEDS {
        let input = stream(seed).ok_or("the generator draws from non-empty tables")?;
        let expected = model(&input);
        for point in 0..=input.len() {
            let got = decode(&cut(&input, &[point]), WIDE_CAP)?;
            assert_eq!(got, expected, "seed {seed}: cut at {point}");
        }
        let bytes: Vec<&[u8]> = input.chunks(1).collect();
        assert_eq!(
            decode(&bytes, WIDE_CAP)?,
            expected,
            "seed {seed}: byte by byte"
        );
        let mut rng = Seeded::from_seed(seed ^ 0x5EED);
        let bound = u64::try_from(input.len().saturating_add(1))?;
        let mut points = Vec::new();
        for _ in 0..8 {
            points.push(usize::try_from(rng.below(bound)?)?);
        }
        points.sort_unstable();
        let got = decode(&cut(&input, &points), WIDE_CAP)?;
        assert_eq!(got, expected, "seed {seed}: cut at {points:?}");
    }
    Ok(())
}

/// One seed replays to one trace and two seeds diverge.
#[test]
fn sim_the_same_seed_replays_and_seeds_diverge() -> Outcome {
    let trace = |seed: u64| -> Result<(Vec<u8>, Vec<Seen>), Box<dyn std::error::Error>> {
        let input = stream(seed).ok_or("the generator draws from non-empty tables")?;
        let seen = decode(&input.chunks(3).collect::<Vec<_>>(), WIDE_CAP)?;
        Ok((input, seen))
    };
    let reference = trace(11)?;
    assert_eq!(trace(11)?, reference, "one seed, one trace");
    let mut distinct = 0_usize;
    for seed in 0..SEEDS {
        if trace(seed)? != reference {
            distinct = distinct.saturating_add(1);
        }
    }
    assert!(distinct > 0, "seeds draw different streams");
    Ok(())
}

/// A block with only an `id` dispatches nothing, and the ID it named is the
/// last-seen ID of the next event (the standard's dispatch step 2).
#[test]
fn an_id_only_block_dispatches_nothing_and_the_id_persists() -> Outcome {
    let got = decode(&[b"id: 7\n\ndata: x\n\n"], WIDE_CAP)?;
    assert_eq!(
        got,
        vec![(
            "message".to_owned(),
            "x".to_owned(),
            Some("7".to_owned()),
            None
        )],
        "the id-only block fires nothing; its id rides on the next event"
    );
    assert_eq!(
        decode(&[b"id: 7\n\n"], WIDE_CAP)?,
        Vec::new(),
        "nothing fires"
    );
    Ok(())
}

/// A data-less block resets the event type, so a later data block is a
/// `message`; an empty type is a `message` too.
#[test]
fn a_data_less_block_resets_the_type_and_an_empty_type_is_message() -> Outcome {
    let got = decode(
        &[b"event: tick\n\ndata: x\n\nevent:\ndata: y\n\n"],
        WIDE_CAP,
    )?;
    let types: Vec<&str> = got.iter().map(|seen| seen.0.as_str()).collect();
    assert_eq!(
        types,
        vec!["message", "message"],
        "type reset, empty is message"
    );
    Ok(())
}

/// A retry with no data rides on the next event; a retry that is not all
/// ASCII digits (`+5`, `9x`) is ignored, as the standard says.
#[test]
fn a_retry_only_block_rides_on_the_next_event() -> Outcome {
    let got = decode(
        &[b"retry: 40\n\nretry: +5\nretry: 9x\ndata: x\n\n"],
        WIDE_CAP,
    )?;
    assert_eq!(
        got.first().and_then(|seen| seen.3),
        Some(40),
        "carried forward"
    );
    Ok(())
}

/// A byte-order mark split across reads is stripped once; a second one is
/// field text, not a mark.
#[test]
fn a_byte_order_mark_split_across_reads_is_stripped_once() -> Outcome {
    let got = decode(&[b"\xEF", b"\xBB", b"\xBFdata: a\n\n"], WIDE_CAP)?;
    assert_eq!(
        got.first().map(|seen| seen.1.as_str()),
        Some("a"),
        "stripped"
    );
    let twice = decode(&[b"\xEF\xBB\xBF\xEF\xBB\xBFdata: a\n\n"], WIDE_CAP)?;
    assert_eq!(twice, Vec::new(), "a second mark makes the field unknown");
    Ok(())
}

/// A 256 KiB line arriving one byte per read is examined once per byte: the
/// old reader rescanned the whole pending line on every read, about 2^35
/// byte visits here.
#[test]
fn a_long_line_over_many_reads_is_scanned_once() -> Outcome {
    const LINE: usize = 256 * 1024;
    const DATA: usize = LINE.saturating_sub(b"data: ".len());
    let mut input = b"data: ".to_vec();
    input.resize(LINE, b'z');
    input.extend_from_slice(b"\n\n");
    let mut decoder = SseDecoder::new(WIDE_CAP);
    let mut events = Vec::new();
    for byte in input.chunks(1) {
        decoder.push(byte);
        while let Some(event) = decoder.next_event()? {
            events.push(view(&event));
        }
    }
    assert!(
        decoder.lines.examined <= input.len(),
        "{} byte visits for {} bytes",
        decoder.lines.examined,
        input.len()
    );
    assert_eq!(events.len(), 1, "one event");
    assert_eq!(
        events.first().map(|seen| seen.1.len()),
        Some(DATA),
        "the whole line is the data"
    );
    Ok(())
}

/// A line one byte past the cap is refused at every split point, buffered
/// or terminated; a line exactly at the cap is accepted.
#[test]
fn a_line_past_the_cap_is_refused_at_every_split() -> Outcome {
    let at_cap = b"data: 0123456789\n\n";
    let past = b"data: 01234567890\n\n";
    for point in 0..=past.len() {
        let refused = decode(&cut(past, &[point]), 16);
        assert_eq!(
            refused,
            Err(Error::BodyTooLarge { limit: 16 }),
            "cut at {point}: seventeen bytes past a sixteen-byte cap"
        );
    }
    for point in 0..=at_cap.len() {
        let got = decode(&cut(at_cap, &[point]), 16)?;
        assert_eq!(got.len(), 1, "cut at {point}: exactly at the cap is a line");
    }
    Ok(())
}
