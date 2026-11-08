//! Reading committed entries back: one frame at a time, from its own descriptor.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::marker::PhantomData;
use std::path::Path;

use super::super::file::verify_frame;
use super::super::frame::{
    HEAD_BYTES, LENGTH_BYTES, SaturatingFrom, declared_length, decodable, framed_len,
};
use super::super::{CorruptionKind, JournalError, JournalPosition};
use super::scan::{Body, parse_body};
use super::{Durable, refuse};

/// One committed record and where the chain holds it.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
/// use lgwks_bot::journal::{RecordEntry, RecordJournal};
///
/// let path = std::env::temp_dir().join(format!("record-doc-entry-{}", std::process::id()));
/// # drop(std::fs::remove_file(&path));
/// let mut journal = RecordJournal::<u64>::open(&path)?;
/// let ack = journal.compare_and_append(journal.tail(), &9)?;
/// let page: Vec<RecordEntry<u64>> = journal.page(0, 1)?;
/// assert_eq!(page.first().map(RecordEntry::position), Some(ack.position()));
/// # drop(journal);
/// # drop(std::fs::remove_file(&path));
/// # Ok(())
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordEntry<R> {
    /// Where the chain holds it.
    position: JournalPosition,
    /// The record.
    record: R,
    /// Whether this is the checkpoint a predecessor sealed with.
    checkpoint: bool,
}

impl<R> RecordEntry<R> {
    /// Where the chain holds this record.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-at-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append(journal.tail(), &9)?;
    /// let page = journal.page(0, 1)?;
    /// assert_eq!(page.first().map(|entry| entry.position().sequence()), Some(1));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn position(&self) -> JournalPosition {
        self.position
    }

    /// The record itself.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-body-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append(journal.tail(), &9)?;
    /// let page = journal.page(0, 1)?;
    /// assert_eq!(page.first().map(|entry| *entry.record()), Some(9));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn record(&self) -> &R {
        &self.record
    }

    /// Whether this is the checkpoint a predecessor sealed with, which stands for
    /// everything before it rather than one more fact.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let path = std::env::temp_dir().join(format!("record-doc-mark2-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append(journal.tail(), &9)?;
    /// let page = journal.page(0, 1)?;
    /// assert_eq!(page.first().map(|entry| entry.is_checkpoint()), Some(false));
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub const fn is_checkpoint(&self) -> bool {
        self.checkpoint
    }
}

/// A verifying stream of one generation's entries, retaining one at a time.
///
/// It stops at the acknowledged end the journal named when it was made, so bytes
/// an append is still moving are never read. A short read or a head that does not
/// follow refuses the stream and ends it: the file changed after it was opened.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), lgwks_bot::journal::JournalError> {
/// use lgwks_bot::journal::{RecordJournal, RecordReplay};
///
/// let path = std::env::temp_dir().join(format!("record-doc-stream-{}", std::process::id()));
/// # drop(std::fs::remove_file(&path));
/// let mut journal = RecordJournal::<u64>::open(&path)?;
/// journal.compare_and_append(journal.tail(), &9)?;
/// let replay: RecordReplay<u64> = journal.replay()?;
/// assert_eq!(replay.count(), 1);
/// # drop(journal);
/// # drop(std::fs::remove_file(&path));
/// # Ok(())
/// # }
/// ```
pub struct RecordReplay<R> {
    /// The frame reader.
    reader: BufReader<File>,
    /// What the next frame chains from.
    position: JournalPosition,
    /// Whether the next frame is the carried seal.
    checkpoint: bool,
    /// The next frame's index in the file, which locates a refusal.
    index: usize,
    /// Where the next frame begins.
    offset: u64,
    /// Where the acknowledged entries end.
    end: u64,
    /// Whether the stream has ended.
    done: bool,
    /// The record type.
    record: PhantomData<fn() -> R>,
}

impl<R: Durable> RecordReplay<R> {
    /// A stream of the frames in `offset..end`, the first chaining from `from`.
    pub(super) fn open(
        path: &Path,
        from: JournalPosition,
        checkpoint: bool,
        index: usize,
        offset: u64,
        end: u64,
    ) -> Result<Self, JournalError> {
        let mut file = File::open(path).map_err(JournalError::Storage)?;
        file.seek(SeekFrom::Start(offset))
            .map_err(JournalError::Storage)?;
        Ok(Self {
            reader: BufReader::new(file),
            position: from,
            checkpoint,
            index,
            offset,
            end,
            done: false,
            record: PhantomData,
        })
    }

    /// The position the frame at `offset` chains from: the head stored just before
    /// it, at the sequence the caller counted.
    pub(super) fn position_before(
        path: &Path,
        offset: u64,
        sequence: u64,
    ) -> Result<JournalPosition, JournalError> {
        let mut file = File::open(path).map_err(JournalError::Storage)?;
        file.seek(SeekFrom::Start(
            offset.saturating_sub(u64::saturating_from(HEAD_BYTES)),
        ))
        .map_err(JournalError::Storage)?;
        let mut head = [0u8; HEAD_BYTES];
        file.read_exact(&mut head).map_err(JournalError::Storage)?;
        Ok(JournalPosition {
            sequence,
            head: lgwks_std::hash::Digest::from_bytes(head),
        })
    }

    /// Read and verify the next frame.
    fn read_next(&mut self) -> Result<RecordEntry<R>, JournalError> {
        let index = u64::saturating_from(self.index);
        let mut prefix = [0u8; LENGTH_BYTES];
        self.reader
            .read_exact(&mut prefix)
            .map_err(JournalError::Storage)?;
        let declared = declared_length(&prefix);
        if self.offset.saturating_add(framed_len(declared)) > self.end {
            return refuse(
                index,
                CorruptionKind::Framed,
                "a frame runs past the acknowledged end",
            );
        }
        let mut payload = vec![0u8; declared];
        self.reader
            .read_exact(&mut payload)
            .map_err(JournalError::Storage)?;
        let mut head = [0u8; HEAD_BYTES];
        self.reader
            .read_exact(&mut head)
            .map_err(JournalError::Storage)?;
        let bytes = match (parse_body(&payload), self.checkpoint) {
            (Some(Body::Record(bytes)), false) => bytes,
            (
                Some(Body::Seal {
                    predecessor,
                    record,
                }),
                true,
            ) if predecessor == self.position => record,
            (Some(_) | None, _) => {
                return refuse(
                    index,
                    CorruptionKind::Undecodable,
                    "the payload is no entry here",
                );
            }
        };
        let Ok(record) = R::from_record(decodable(bytes).as_slice()) else {
            return refuse(
                index,
                CorruptionKind::Undecodable,
                "the record does not decode",
            );
        };
        let position = verify_frame(&payload, &head, self.position, index)?;
        let entry = RecordEntry {
            position,
            record,
            checkpoint: self.checkpoint,
        };
        self.position = position;
        self.checkpoint = false;
        self.index = self.index.saturating_add(1);
        self.offset = self.offset.saturating_add(framed_len(declared));
        Ok(entry)
    }
}

impl<R: Durable> Iterator for RecordReplay<R> {
    type Item = Result<RecordEntry<R>, JournalError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.offset >= self.end {
            return None;
        }
        let item = self.read_next();
        self.done = item.is_err();
        Some(item)
    }
}
