//! The liar-upstream fool against the bounded body reader.
//!
//! A transport that reports more bytes than the window it was handed is the one
//! upstream the ceiling cannot see coming: the count is the only thing the reader
//! trusts. Before the window check, a count in `(wanted, READ_CHUNK_BYTES]` fit
//! inside the chunk buffer and was kept, so a body could grow past its ceiling.
//! The oracle is the contract, not the code: a liar is refused at the body stage,
//! and an honest reader with the same ceiling keeps exactly what it sent.

use super::*;
use crate::seeded::Seeded;

/// Seeds the sweep runs; each one draws its own ceilings and lies.
const SEEDS: [u64; 4] = [0x11A2_0000_0000_0001, 0x11A2_0000_0000_0002, 0xDEAD_BEEF, 1];

/// Draws per seed.
const DRAWS: usize = 64;

/// A reader that fills its window and then claims `overstated` more bytes.
struct Liar {
    /// How far past the window the count is pushed.
    overstated: usize,
    /// Whether it has lied yet; later reads end the body.
    lied: bool,
}

impl Read for Liar {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.lied {
            return Ok(0);
        }
        self.lied = true;
        buffer.fill(b'L');
        Ok(buffer.len().saturating_add(self.overstated))
    }
}

/// A reader that answers honestly for `honest_reads` calls, then lies by `overstated`.
struct LateLiar {
    /// Honest reads left before the lie.
    honest_reads: usize,
    /// How far past the window the lie reaches.
    overstated: usize,
}

impl Read for LateLiar {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        buffer.fill(b'T');
        if self.honest_reads == 0 {
            return Ok(buffer.len().saturating_add(self.overstated));
        }
        self.honest_reads = self.honest_reads.saturating_sub(1);
        Ok(buffer.len())
    }
}

/// A reader that yields `before` bytes, then fails with `kind`.
struct PulledPlug {
    /// Bytes still to yield before the fault.
    before: usize,
    /// The fault the transport raises.
    kind: std::io::ErrorKind,
}

impl Read for PulledPlug {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.before == 0 {
            #[cfg(feature = "trace")]
            crate::trace::debug!("read: returning an error to the caller");
            return Err(std::io::Error::from(self.kind));
        }
        let given = buffer.len().min(self.before);
        buffer.iter_mut().take(given).for_each(|byte| *byte = b'P');
        self.before = self.before.saturating_sub(given);
        Ok(given)
    }
}

/// One ceiling in `1..=3 * READ_CHUNK_BYTES` and one overstatement in
/// `1..=READ_CHUNK_BYTES`, the band the chunk buffer used to absorb.
fn draw(stream: &mut Seeded) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let span = u64::try_from(READ_CHUNK_BYTES.saturating_mul(3))?;
    let ceiling_bytes = usize::try_from(stream.below(span)?)?.saturating_add(1);
    let lie = usize::try_from(stream.below(u64::try_from(READ_CHUNK_BYTES)?)?)?;
    Ok((ceiling_bytes, lie.saturating_add(1)))
}

#[test]
fn sim_lying_reader_an_overstated_count_is_refused_at_the_body_stage()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let (limit, overstated) = draw(&mut stream)?;
            let mut liar = Liar {
                overstated,
                lied: false,
            };
            let outcome = read_bounded(&mut liar, &ceiling(limit));
            assert!(
                matches!(
                    outcome,
                    Err(Error::Failure {
                        stage: FailureStage::Body,
                        kind: FailureKind::Transport,
                        ..
                    })
                ),
                "seed {seed:#x}: a count {overstated} past the window under a {limit}-byte \
                 ceiling is refused, got {:?}",
                outcome.map(|(body, _)| body.len())
            );
        }
    }
    Ok(())
}

#[test]
fn sim_lying_reader_an_honest_reader_at_the_same_ceiling_keeps_its_bytes()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..DRAWS {
            let (limit, _) = draw(&mut stream)?;
            let mut honest = std::io::Cursor::new(vec![b'h'; limit]);
            let (body, truncation) = read_bounded(&mut honest, &ceiling(limit))?;
            assert_eq!(
                body.len(),
                limit,
                "seed {seed:#x}: the whole body at {limit}"
            );
            assert!(
                body.iter().all(|byte| *byte == b'h'),
                "seed {seed:#x}: its own bytes"
            );
            assert_eq!(truncation, Truncation::Complete, "seed {seed:#x}");
        }
    }
    Ok(())
}

/// Whether `outcome` is the refusal a lying count earns.
fn is_window_refusal(outcome: &Result<(Vec<u8>, Truncation), Error>) -> bool {
    matches!(
        outcome,
        Err(Error::Failure {
            stage: FailureStage::Body,
            kind: FailureKind::Transport,
            ..
        })
    )
}

#[test]
fn sim_lying_reader_a_lie_after_honest_chunks_is_still_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.rotate_left(7));
        for _ in 0..DRAWS {
            let honest_reads = stream.index(4)?;
            let (extra, overstated) = draw(&mut stream)?;
            let limit = READ_CHUNK_BYTES
                .saturating_mul(honest_reads.saturating_add(1))
                .saturating_add(extra);
            let mut liar = LateLiar {
                honest_reads,
                overstated,
            };
            let outcome = read_bounded(&mut liar, &ceiling(limit));
            assert!(
                is_window_refusal(&outcome),
                "seed {seed:#x}: a lie after {honest_reads} honest chunks under {limit} bytes"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_lying_reader_a_count_of_usize_max_is_refused_without_allocating() {
    for overstated in [
        usize::MAX,
        usize::MAX.saturating_sub(1),
        usize::MAX >> 1_u32,
    ] {
        let mut liar = Liar {
            overstated,
            lied: false,
        };
        let outcome = read_bounded(&mut liar, &ceiling(READ_CHUNK_BYTES));
        assert!(
            is_window_refusal(&outcome),
            "a count of {overstated} is refused before any reservation"
        );
    }
}

#[test]
fn sim_lying_reader_a_preview_past_its_ceiling_is_cut_not_refused()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed ^ 0x9E37);
        for _ in 0..DRAWS {
            let (limit, extra) = draw(&mut stream)?;
            let mut honest = std::io::Cursor::new(vec![b'v'; limit.saturating_add(extra)]);
            let (body, truncation) = read_bounded(&mut honest, &previewing(limit))?;
            assert_eq!(
                body.len(),
                limit,
                "seed {seed:#x}: the preview holds {limit}"
            );
            assert_eq!(
                truncation,
                Truncation::Cut,
                "seed {seed:#x}: {extra} bytes remained"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_lying_reader_a_whole_body_past_its_ceiling_names_the_limit()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_add(0x51));
        for _ in 0..DRAWS {
            let (limit, extra) = draw(&mut stream)?;
            let mut honest = std::io::Cursor::new(vec![b'w'; limit.saturating_add(extra)]);
            let outcome = read_bounded(&mut honest, &ceiling(limit));
            assert!(
                matches!(outcome, Err(Error::BodyTooLarge { limit: named }) if named == limit),
                "seed {seed:#x}: {extra} bytes past {limit} is refused by its ceiling"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_lying_reader_a_pulled_plug_mid_body_keeps_its_stage_and_class()
-> Result<(), Box<dyn std::error::Error>> {
    let faults = [
        (std::io::ErrorKind::ConnectionReset, FailureKind::Transport),
        (std::io::ErrorKind::BrokenPipe, FailureKind::Transport),
        (std::io::ErrorKind::UnexpectedEof, FailureKind::Transport),
        (std::io::ErrorKind::Interrupted, FailureKind::Interrupted),
        (std::io::ErrorKind::TimedOut, FailureKind::Timeout),
    ];
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.wrapping_mul(5));
        for _ in 0..DRAWS {
            let (limit, _) = draw(&mut stream)?;
            let before = stream.index(limit)?;
            let (kind, class) = *faults.get(stream.index(faults.len())?).ok_or("a fault")?;
            let mut plug = PulledPlug { before, kind };
            let outcome = read_bounded(&mut plug, &ceiling(limit));
            assert!(
                matches!(
                    outcome,
                    Err(Error::Failure { stage: FailureStage::Body, kind: seen, .. }) if seen == class
                ),
                "seed {seed:#x}: {kind:?} after {before} of {limit} bytes is a body {class:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn sim_lying_reader_a_plug_pulled_at_the_ceiling_fails_the_eof_probe()
-> Result<(), Box<dyn std::error::Error>> {
    for seed in SEEDS {
        let mut stream = Seeded::from_seed(seed.swap_bytes());
        for _ in 0..DRAWS {
            let (limit, _) = draw(&mut stream)?;
            let mut plug = PulledPlug {
                before: limit,
                kind: std::io::ErrorKind::ConnectionReset,
            };
            let outcome = read_bounded(&mut plug, &previewing(limit));
            assert!(
                matches!(
                    outcome,
                    Err(Error::Failure {
                        stage: FailureStage::EofProbe,
                        kind: FailureKind::Transport,
                        ..
                    })
                ),
                "seed {seed:#x}: a reset after exactly {limit} bytes fails the probe, not the body"
            );
        }
    }
    Ok(())
}
