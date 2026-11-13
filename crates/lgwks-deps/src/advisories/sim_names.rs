//! The advisory file-name rule (#942), on names no filesystem in CI can hold:
//! APFS refuses a name that is not UTF-8, so the rule is checked on the name.

use std::ffi::{OsStr, OsString};

use super::AdvisoryError;
use super::db::advisory_name;

/// `RUSTSEC-2099-?.md` with the `?` not UTF-8 on this target.
#[cfg(unix)]
fn not_utf8() -> OsString {
    use std::os::unix::ffi::OsStringExt as _;
    OsString::from_vec(b"RUSTSEC-2099-\xff.md".to_vec())
}

/// `RUSTSEC-2099-?.md` with the `?` an unpaired surrogate.
#[cfg(windows)]
fn not_utf8() -> OsString {
    use std::os::windows::ffi::OsStringExt as _;
    let mut wide: Vec<u16> = "RUSTSEC-2099-".encode_utf16().collect();
    wide.push(0xD800);
    wide.extend(".md".encode_utf16());
    OsString::from_wide(&wide)
}

/// Catches: a name that is not UTF-8 skipped (an advisory silently unread) or
/// a dotfile, the template or a non-Markdown name read as an advisory.
#[cfg(any(unix, windows))]
#[test]
fn sim_advisories_a_name_is_read_skipped_or_refused_never_guessed() -> Result<(), String> {
    let cases: [(&str, Option<&str>); 6] = [
        ("RUSTSEC-2099-0001.md", Some("RUSTSEC-2099-0001.md")),
        (".RUSTSEC-2099-0001.md", None),
        ("RUSTSEC-0000-0000.md", None),
        ("README.txt", None),
        ("RUSTSEC-2099-0001.md.orig", None),
        ("RUSTSEC-2099-0001.MD", None),
    ];
    for (name, expected) in cases {
        let decided = advisory_name(OsStr::new(name)).map_err(|error| error.to_string())?;
        if decided != expected {
            return Err(format!("{name:?} read as {decided:?}"));
        }
    }
    let name = not_utf8();
    match advisory_name(&name) {
        Err(AdvisoryError::NotUtf8 { .. }) => Ok(()),
        other => Err(format!("a name that is not UTF-8 gave {other:?}")),
    }
}
