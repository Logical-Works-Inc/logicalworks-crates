//! The private syscall helpers [`super::Dir`]'s `*at` calls share: the
//! one-component check, the mode words, the kind mapping and the errno bridge.

use std::io;

use super::FileKind;

/// Reject anything that is not one plain component of a name.
///
/// This is the check that makes the `*at` calls safe to reason about. `..` is
/// the important one: it is the spelling that moves *up* out of the directory
/// the descriptor names, so a caller walking a tree cannot be walked out of it
/// by an entry whose name is `..`.
pub(super) fn single_component(name: &str) -> io::Result<std::ffi::CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        let refusal = Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name:?} is not a single path component"),
        ));
        #[cfg(feature = "trace")]
        crate::trace::debug!(error = ?refusal.as_ref().err(), "single_component: returning an error to the caller");
        return refusal;
    }
    std::ffi::CString::new(name).map_err(|cause| {
        // The offset is read off the cause before the cause is moved into the
        // error: `CString::new` already located the NUL, and restating a second
        // copy of that number could only disagree with it.
        let at = cause.nul_position();
        io::Error::new(io::ErrorKind::InvalidInput, InteriorNul { cause, at })
    })
}

/// Why a path component could not become a `CString`.
///
/// A named type rather than a `String`: the [`std::ffi::NulError`] travels
/// inside it and is what makes the refusal observable, while the rendered text
/// stays the module's own phrase and never quotes the rejected name.
#[derive(Debug)]
struct InteriorNul {
    /// The interior-NUL failure `CString::new` reported, kept as the cause so
    /// `Error::source` reaches it rather than losing it at the translation.
    cause: std::ffi::NulError,
    /// Offset of the NUL byte within the rejected component.
    at: usize,
}

impl std::fmt::Display for InteriorNul {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Deliberately no `{:?}` of the rejected name: this path is reached
        // with caller-supplied text, and a name carrying CR/LF would forge a
        // second line in whatever log interpolates the `io::Error`.
        write!(
            formatter,
            "path component contains an interior NUL at offset {}",
            self.at
        )
    }
}

impl std::error::Error for InteriorNul {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.cause)
    }
}

/// Map a raw `st_mode` word to the kind this crate reports.
///
/// Delegates to rustix's `FileType::from_raw_mode` rather than decoding the
/// `S_IFMT` bits here, and that is not a style preference. `Stat::st_mode` has
/// type `rustix::fs::RawMode`, which is `c_uint` on rustix's Linux backend and
/// `mode_t` — a `u16` — on its libc backend. A hand-written `u16` parameter
/// compiles on macOS and fails on Linux, which is where this repository's CI
/// runs. Decoding the bits locally also restated seven `S_IF*` constants that
/// rustix already maps, with nothing to catch a transcription error.
pub(super) fn classify(raw: rustix::fs::RawMode) -> FileKind {
    use rustix::fs::FileType as Ft;
    let file_type = Ft::from_raw_mode(raw);
    if file_type == Ft::RegularFile {
        FileKind::File
    } else if file_type == Ft::Directory {
        FileKind::Directory
    } else if file_type == Ft::Symlink {
        FileKind::Symlink
    } else if file_type == Ft::Fifo {
        FileKind::Fifo
    } else if file_type == Ft::CharacterDevice {
        FileKind::CharDevice
    } else if file_type == Ft::BlockDevice {
        FileKind::BlockDevice
    } else if file_type == Ft::Socket {
        FileKind::Socket
    } else {
        FileKind::Unknown
    }
}

/// Mode for a file this process owns: readable and writable by the user only.
///
/// `RUSR | WUSR` is 0o600, and nothing else may be set.
///
/// The previous value was `RUSR | RWXG | RWXO` — rustix's names for *all* the
/// group bits and *all* the world bits, which read like "user read, and then
/// some group and world bits" rather than "owner-only". It is 0o477: the
/// setuid bit, plus group and world rwx. A umask does not rescue that, because
/// it can only clear bits. On the host that caught it, with the usual 0o022
/// umask, `create_new` produced a file at 0o455 — world-readable and
/// world-executable. Anything this process wrote into a shared tree was
/// readable by every other user on the machine.
///
/// Written as the two named user bits rather than as a literal 0o600 because a
/// bare literal is exactly the thing that reads as obviously correct while
/// being wrong, and `create_new` is the path a caller reaches for when it
/// intends the file to be private.
pub(super) fn user_writable_mode() -> rustix::fs::Mode {
    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR
}

/// Mode for a directory this process owns: user `rwx`, nobody else's.
pub(super) fn user_directory_mode() -> rustix::fs::Mode {
    rustix::fs::Mode::RWXU
}

/// Convert a rustix errno into the std error surface.
pub(super) fn errno_to_io(errno: rustix::io::Errno) -> io::Error {
    io::Error::from_raw_os_error(errno.raw_os_error())
}
