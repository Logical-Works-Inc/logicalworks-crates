//! `--write-doc`: put the table into a Markdown doc between two marker lines.
//!
//! The doc is the hostile reader here: it is a file a human edits. A marker is a
//! whole line, trimmed, that is exactly [`START`] or [`END`] and sits outside a
//! fenced code block, so a doc may show the markers in an example without the
//! example being taken for the real ones. A doc with no start, no end, either one
//! twice, or the end first is refused with its own [`DocFault`] and not written.
//! Everything outside the two lines is kept byte for byte, and the lines the
//! table fills use the line ending of the start marker's line.
//!
//! The write goes to a temp file beside the doc, which is then renamed over it,
//! so a doc is never half written. The doc is read through a bound of
//! [`MAX_DOC_BYTES`]: a larger file is refused, not read whole.

use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::{BenchError, DocFault};

/// The line that opens the block the table fills.
pub(crate) const START: &str = "<!-- durable-table:start -->";

/// The line that closes it.
pub(crate) const END: &str = "<!-- durable-table:end -->";

/// The most bytes of a doc that is read: a larger one is refused.
pub(crate) const MAX_DOC_BYTES: u64 = 1 << 20;

/// An open fenced code block: the fence character and how many of it opened.
#[derive(Debug, Clone, Copy)]
struct Fence {
    /// A backtick or a tilde.
    glyph: char,
    /// How many of it opened the block; the closing fence has at least as many.
    width: usize,
}

/// Where the block the table fills sits in a doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Span {
    /// The byte just past the start marker's line, line ending included.
    after_start: usize,
    /// The first byte of the end marker's line.
    end: usize,
    /// The line ending the start marker's line has.
    eol: &'static str,
}

/// The fence run a line is made of: its glyph, its width and what follows it. A
/// line indented four or more spaces, or with fewer than three fence characters,
/// is not a fence.
fn fence_run(body: &str) -> Option<(char, usize, &str)> {
    let rest = body.trim_start_matches(' ');
    if body.len() - rest.len() > 3 {
        return None;
    }
    let glyph = rest.chars().next().filter(|found| matches!(found, '`' | '~'))?;
    let tail = rest.trim_start_matches(glyph);
    let width = rest.len() - tail.len();
    (width >= 3).then_some((glyph, width, tail))
}

/// Which of the doc's lines are the markers: the first of each, how many of each,
/// and the line ending after the first start marker.
fn locate(doc: &str) -> Result<Span, DocFault> {
    let (mut starts, mut ends) = (0_usize, 0_usize);
    let mut first_start: Option<(usize, &'static str)> = None;
    let mut first_end: Option<usize> = None;
    let mut open: Option<Fence> = None;
    let mut at = 0_usize;
    for line in doc.split_inclusive('\n') {
        let begin = at;
        at += line.len();
        let body = line.trim_end_matches(['\n', '\r']);
        match (open, fence_run(body)) {
            (Some(fence), Some((glyph, width, tail)))
                if glyph == fence.glyph && width >= fence.width && tail.trim().is_empty() =>
            {
                open = None;
            }
            (Some(_), _) => {}
            (None, Some((glyph, width, _))) => open = Some(Fence { glyph, width }),
            (None, None) if body.trim() == START => {
                starts += 1;
                let eol = if line.ends_with("\r\n") { "\r\n" } else { "\n" };
                first_start.get_or_insert((begin + line.len(), eol));
            }
            (None, None) if body.trim() == END => {
                ends += 1;
                first_end.get_or_insert(begin);
            }
            (None, None) => {}
        }
    }
    let (Some((after_start, eol)), Some(end)) = (first_start, first_end) else {
        let fault = if first_start.is_none() {
            DocFault::StartMissing
        } else {
            DocFault::EndMissing
        };
        lgwks_std::trace::debug!(?fault, "the doc lacks a marker line outside a fence");
        return Err(fault);
    };
    if starts > 1 || ends > 1 {
        let fault = if starts > 1 {
            DocFault::StartRepeated
        } else {
            DocFault::EndRepeated
        };
        lgwks_std::trace::debug!(?fault, starts, ends, "the doc repeats a marker line");
        return Err(fault);
    }
    if end < after_start {
        lgwks_std::trace::debug!(end, after_start, "the end marker comes before the start marker");
        return Err(DocFault::OutOfOrder);
    }
    Ok(Span {
        after_start,
        end,
        eol,
    })
}

/// `doc` with the block between its two marker lines replaced by `table`, one
/// line of the table to a line of the doc.
///
/// # Errors
///
/// The [`DocFault`] that names what is wrong with the markers.
pub(crate) fn splice(doc: &str, table: &str) -> Result<String, DocFault> {
    let span = locate(doc)?;
    let mut out = String::with_capacity(doc.len() + table.len() + table.lines().count() * 2);
    let (mut at, mut filled) = (0_usize, false);
    for line in doc.split_inclusive('\n') {
        if at >= span.after_start && !filled {
            for row in table.lines() {
                out.push_str(row);
                out.push_str(span.eol);
            }
            filled = true;
        }
        if at < span.after_start || at >= span.end {
            out.push_str(line);
        }
        at += line.len();
    }
    Ok(out)
}

/// The refusal of `fault` on `path`.
fn refused(path: &Path, fault: DocFault) -> BenchError {
    BenchError::DocMarkers {
        path: path.to_path_buf(),
        fault,
    }
}

/// The doc at `path`, read through the [`MAX_DOC_BYTES`] bound.
fn read(path: &Path) -> Result<String, BenchError> {
    let file = std::fs::File::open(path).map_err(|source| BenchError::Io {
        context: "opening the doc",
        source,
    })?;
    let mut bytes = Vec::new();
    file.take(MAX_DOC_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|source| BenchError::Io {
            context: "reading the doc",
            source,
        })?;
    match u64::try_from(bytes.len()) {
        Ok(len) if len <= MAX_DOC_BYTES => {}
        Ok(_) | Err(_) => {
            lgwks_std::trace::debug!(path = ?path, "the doc is longer than the bound");
            return Err(refused(path, DocFault::TooLarge));
        }
    }
    String::from_utf8(bytes).map_err(|source| {
        lgwks_std::trace::debug!(path = ?path, %source, "the doc is not UTF-8");
        refused(path, DocFault::NotUtf8)
    })
}

/// Refuse a doc that cannot take the table, before the long run that makes it.
///
/// # Errors
///
/// [`BenchError::DocMarkers`] for a doc the table cannot go into, and
/// [`BenchError::Io`] when it cannot be read.
pub(crate) fn check(path: &Path) -> Result<(), BenchError> {
    locate(&read(path)?)
        .map(|_| ())
        .map_err(|fault| refused(path, fault))
}

/// The temp file beside `path` that the new doc is written to first.
fn temp_beside(path: &Path) -> Result<PathBuf, BenchError> {
    let Some(name) = path.file_name() else {
        lgwks_std::trace::debug!(path = ?path, "the doc path names no file");
        return Err(BenchError::Io {
            context: "naming the doc's temp file",
            source: std::io::Error::from(std::io::ErrorKind::InvalidInput),
        });
    };
    let mut temp = name.to_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    Ok(path.with_file_name(temp))
}

/// Put `table` between the markers of the doc at `path`.
///
/// # Errors
///
/// [`BenchError::DocMarkers`] for a doc the table cannot go into and
/// [`BenchError::Io`] when the doc cannot be read, the temp file written or the
/// rename made; the doc is then as it was.
pub(crate) fn write(path: &Path, table: &str) -> Result<(), BenchError> {
    let text = read(path)?;
    let next = splice(&text, table).map_err(|fault| refused(path, fault))?;
    let permissions = std::fs::metadata(path)
        .map_err(|source| BenchError::Io {
            context: "reading the doc's permissions",
            source,
        })?
        .permissions();
    let temp = temp_beside(path)?;
    std::fs::write(&temp, next).map_err(|source| BenchError::Io {
        context: "writing the doc's temp file",
        source,
    })?;
    let placed =
        std::fs::set_permissions(&temp, permissions).and_then(|()| std::fs::rename(&temp, path));
    if let Err(source) = placed {
        lgwks_std::trace::debug!(path = ?path, %source, "the new doc could not replace the old");
        if let Err(error) = std::fs::remove_file(&temp) {
            lgwks_std::trace::warn!(%error, "the doc's temp file was not removed");
        }
        return Err(BenchError::Io {
            context: "replacing the doc with its temp file",
            source,
        });
    }
    Ok(())
}
