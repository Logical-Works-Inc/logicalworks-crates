//! The directory of failed-job logs: which files are logs, and how much of one is
//! read.
//!
//! Two bounds hold the memory. A directory with more than [`MAX_LOGS`] logs is
//! refused, so the list of names is short; and a log is read one at a time, at
//! most [`MAX_LOG_BYTES`] of it, from the end: the failure is at the end of a
//! job's log, and what is dropped is the start.

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::error::ReceiptError;

/// The most logs one run reads.
pub(crate) const MAX_LOGS: usize = 512;

/// The most bytes of one log read: 32 MiB.
pub(crate) const MAX_LOG_BYTES: u64 = 33_554_432;

/// One log file of the directory.
#[derive(Debug)]
pub(crate) struct LogFile {
    /// The file's stem, which names the job.
    pub(crate) job: String,
    /// The file.
    pub(crate) path: PathBuf,
}

/// The text read from one log.
#[derive(Debug)]
pub(crate) struct LogText {
    /// The lines read.
    pub(crate) text: String,
    /// Whether the start of the log lay past the bound and was dropped.
    pub(crate) clipped: bool,
}

/// The `.log` files of `dir`, by job name. A link, a directory or any other file
/// is not a log; the order is the name's, so a listing is the same on every host.
pub(crate) fn list(dir: &Path) -> Result<Vec<LogFile>, ReceiptError> {
    let unlisted = |source: io::Error| ReceiptError::LogsDir {
        path: dir.to_path_buf(),
        source,
    };
    let mut files: Vec<LogFile> = Vec::new();
    for entry in fs::read_dir(dir).map_err(unlisted)? {
        let entry = entry.map_err(unlisted)?;
        let name = entry.file_name();
        let Some(job) = name
            .to_str()
            .and_then(|text| text.strip_suffix(".log"))
            .filter(|stem| !stem.is_empty())
        else {
            continue;
        };
        if !entry.metadata().map_err(unlisted)?.is_file() {
            continue;
        }
        if files.len() >= MAX_LOGS {
            lgwks_std::trace::debug!(
                dir = %dir.display(),
                limit = MAX_LOGS,
                "the logs directory holds too many logs"
            );
            return Err(ReceiptError::TooManyLogs {
                path: dir.to_path_buf(),
                limit: MAX_LOGS,
            });
        }
        files.push(LogFile {
            job: job.to_owned(),
            path: entry.path(),
        });
    }
    files.sort_by(|left, right| {
        left.job
            .cmp(&right.job)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(files)
}

/// The last [`MAX_LOG_BYTES`] of the log at `path`.
pub(crate) fn read(path: &Path) -> Result<LogText, ReceiptError> {
    read_within(path, MAX_LOG_BYTES)
}

/// The last `limit` bytes of the log at `path`, from its first whole line when
/// the start was dropped. Bytes that are not UTF-8 read as U+FFFD.
pub(crate) fn read_within(path: &Path, limit: u64) -> Result<LogText, ReceiptError> {
    let unreadable = |source: io::Error| ReceiptError::ReadLog {
        path: path.to_path_buf(),
        source,
    };
    let mut file = File::open(path).map_err(unreadable)?;
    let skip = file
        .metadata()
        .map_err(unreadable)?
        .len()
        .saturating_sub(limit);
    if skip > 0 {
        file.seek(SeekFrom::Start(skip)).map_err(unreadable)?;
    }
    let mut bytes = Vec::new();
    Read::take(file, limit)
        .read_to_end(&mut bytes)
        .map_err(unreadable)?;
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(invalid) => String::from_utf8_lossy(invalid.as_bytes()).into_owned(),
    };
    if skip == 0 {
        return Ok(LogText {
            text,
            clipped: false,
        });
    }
    let whole = match text.split_once('\n') {
        Some((_, rest)) => rest.to_owned(),
        None => String::new(),
    };
    Ok(LogText {
        text: whole,
        clipped: true,
    })
}
