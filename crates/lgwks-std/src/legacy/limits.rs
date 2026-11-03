//! `limits` bounds every legacy reader before it allocates.
//!
//! Each codec in this directory takes a `Limits` and refuses input past it
//! with a typed error, so a corrupt length prefix or a cyclic link cannot grow
//! the heap without bound. The checks run before any allocation, never after a
//! prefix has been written.

use std::error::Error;
use std::fmt;

/// Why a legacy reader refused its input on a bound.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitError {
    /// The input holds more bytes than the reader admits.
    TooManyBytes {
        /// The configured ceiling, in bytes.
        limit: usize,
        /// The observed size, in bytes.
        seen: usize,
    },
    /// The input holds more records than the reader admits.
    TooManyRecords {
        /// The configured ceiling, in records.
        limit: usize,
        /// The observed count.
        seen: usize,
    },
    /// The input holds more pages than the reader admits.
    TooManyPages {
        /// The configured ceiling, in pages.
        limit: usize,
        /// The observed count.
        seen: usize,
    },
    /// The input nests deeper than the reader admits.
    TooDeep {
        /// The configured ceiling, in levels.
        limit: usize,
        /// The observed depth.
        seen: usize,
    },
    /// The decompressed form would exceed its ceiling.
    TooMuchOutput {
        /// The configured ceiling, in bytes.
        limit: usize,
        /// The observed size, in bytes.
        seen: usize,
    },
}

impl fmt::Display for LimitError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TooManyBytes { limit, seen } => write!(
                formatter,
                "legacy input of {seen} bytes exceeds the {limit}-byte ceiling"
            ),
            Self::TooManyRecords { limit, seen } => write!(
                formatter,
                "legacy input of {seen} records exceeds the {limit}-record ceiling"
            ),
            Self::TooManyPages { limit, seen } => write!(
                formatter,
                "legacy input of {seen} pages exceeds the {limit}-page ceiling"
            ),
            Self::TooDeep { limit, seen } => write!(
                formatter,
                "legacy input nests {seen} deep past the {limit}-level ceiling"
            ),
            Self::TooMuchOutput { limit, seen } => write!(
                formatter,
                "legacy output of {seen} bytes exceeds the {limit}-byte ceiling"
            ),
        }
    }
}

impl Error for LimitError {}

/// Implements [`Error`] and `From<LimitError>` for a legacy codec error
/// whose `Limit` variant carries the [`LimitError`] that refused the input:
/// the refusal is its source, carried without rewording. Every codec error
/// in `legacy` takes its plumbing from here.
macro_rules! limit_carrying_error {
    ($error:ident) => {
        impl ::std::error::Error for $error {
            fn source(&self) -> Option<&(dyn ::std::error::Error + 'static)> {
                match *self {
                    Self::Limit(ref cause) => Some(cause),
                    _ => None,
                }
            }
        }

        impl From<$crate::legacy::limits::LimitError> for $error {
            /// Carries a limit refusal into the codec error without rewording it.
            fn from(cause: $crate::legacy::limits::LimitError) -> Self {
                Self::Limit(cause)
            }
        }
    };
}

pub(crate) use limit_carrying_error;

/// The bounds every legacy reader enforces before allocating.
///
/// All fields are private; construction is through [`Limits::new`] and every
/// use is through a `check_*` refusal, so a bound is always named and never a
/// bare number at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Limits {
    /// The most input bytes a reader admits.
    max_bytes: usize,
    /// The most records a reader admits.
    max_records: usize,
    /// The most pages a reader admits.
    max_pages: usize,
    /// The deepest nesting a reader admits.
    max_nesting: usize,
    /// The most decompressed bytes a reader admits.
    max_decompressed_bytes: usize,
}

impl Limits {
    /// The bounds in one place: bytes, records, pages, nesting, output bytes.
    #[must_use]
    pub const fn new(
        max_bytes: usize,
        max_records: usize,
        max_pages: usize,
        max_nesting: usize,
        max_decompressed_bytes: usize,
    ) -> Self {
        Self {
            max_bytes,
            max_records,
            max_pages,
            max_nesting,
            max_decompressed_bytes,
        }
    }

    /// Refuses `seen` bytes past the byte ceiling.
    ///
    /// # Errors
    ///
    /// [`LimitError::TooManyBytes`] when `seen` exceeds the ceiling.
    pub(crate) fn check_bytes(&self, seen: usize) -> Result<(), LimitError> {
        if seen > self.max_bytes {
            let refusal = Err(LimitError::TooManyBytes {
                limit: self.max_bytes,
                seen,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                limit = self.max_bytes,
                seen,
                "limits: refusing oversize input"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` records past the record ceiling.
    ///
    /// # Errors
    ///
    /// [`LimitError::TooManyRecords`] when `seen` exceeds the ceiling.
    pub(crate) fn check_records(&self, seen: usize) -> Result<(), LimitError> {
        if seen > self.max_records {
            let refusal = Err(LimitError::TooManyRecords {
                limit: self.max_records,
                seen,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                limit = self.max_records,
                seen,
                "limits: refusing too many records"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` pages past the page ceiling.
    ///
    /// # Errors
    ///
    /// [`LimitError::TooManyPages`] when `seen` exceeds the ceiling.
    pub(crate) fn check_pages(&self, seen: usize) -> Result<(), LimitError> {
        if seen > self.max_pages {
            let refusal = Err(LimitError::TooManyPages {
                limit: self.max_pages,
                seen,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                limit = self.max_pages,
                seen,
                "limits: refusing too many pages"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` nesting levels past the nesting ceiling.
    ///
    /// # Errors
    ///
    /// [`LimitError::TooDeep`] when `seen` exceeds the ceiling.
    pub(crate) fn check_nesting(&self, seen: usize) -> Result<(), LimitError> {
        if seen > self.max_nesting {
            let refusal = Err(LimitError::TooDeep {
                limit: self.max_nesting,
                seen,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                limit = self.max_nesting,
                seen,
                "limits: refusing too deep a nesting"
            );
            return refusal;
        }
        Ok(())
    }

    /// Refuses `seen` output bytes past the decompressed ceiling.
    ///
    /// # Errors
    ///
    /// [`LimitError::TooMuchOutput`] when `seen` exceeds the ceiling.
    pub(crate) fn check_decompressed(&self, seen: usize) -> Result<(), LimitError> {
        if seen > self.max_decompressed_bytes {
            let refusal = Err(LimitError::TooMuchOutput {
                limit: self.max_decompressed_bytes,
                seen,
            });
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                limit = self.max_decompressed_bytes,
                seen,
                "limits: refusing oversize output"
            );
            return refusal;
        }
        Ok(())
    }
}

impl Default for Limits {
    /// The default ceilings: 8 MiB in, 100k records, 4k pages, 64 deep, 32 MiB out.
    fn default() -> Self {
        Self {
            max_bytes: 8_usize.saturating_mul(1024).saturating_mul(1024),
            max_records: 100_000,
            max_pages: 4_096,
            max_nesting: 64,
            max_decompressed_bytes: 32_usize.saturating_mul(1024).saturating_mul(1024),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LimitError, Limits};

    /// The bounds under test: small enough to trip, large enough to admit.
    const BOUNDS: Limits = Limits::new(8, 2, 2, 1, 16);

    /// What every fallible check here returns.
    type CheckResult = Result<(), LimitError>;

    #[test]
    fn an_input_at_the_ceiling_is_admitted() -> CheckResult {
        BOUNDS.check_bytes(8)?;
        BOUNDS.check_records(2)?;
        BOUNDS.check_pages(2)?;
        BOUNDS.check_nesting(1)?;
        BOUNDS.check_decompressed(16)?;
        Ok(())
    }

    #[test]
    fn an_input_past_the_ceiling_is_refused() {
        assert_eq!(
            BOUNDS.check_bytes(9),
            Err(LimitError::TooManyBytes { limit: 8, seen: 9 }),
            "nine bytes past an eight-byte ceiling must be refused"
        );
        assert_eq!(
            BOUNDS.check_records(3),
            Err(LimitError::TooManyRecords { limit: 2, seen: 3 }),
            "three records past a two-record ceiling must be refused"
        );
        assert_eq!(
            BOUNDS.check_pages(3),
            Err(LimitError::TooManyPages { limit: 2, seen: 3 }),
            "three pages past a two-page ceiling must be refused"
        );
        assert_eq!(
            BOUNDS.check_nesting(2),
            Err(LimitError::TooDeep { limit: 1, seen: 2 }),
            "two levels past a one-level ceiling must be refused"
        );
        assert_eq!(
            BOUNDS.check_decompressed(17),
            Err(LimitError::TooMuchOutput {
                limit: 16,
                seen: 17
            }),
            "seventeen output bytes past a sixteen-byte ceiling must be refused"
        );
    }

    #[test]
    fn the_default_ceiling_admits_a_small_input() -> CheckResult {
        let bounds = Limits::default();
        bounds.check_bytes(1)?;
        bounds.check_records(1)?;
        let mebibytes = 8_usize.saturating_mul(1024).saturating_mul(1024);
        bounds.check_bytes(mebibytes)?;
        assert_eq!(
            bounds.check_bytes(mebibytes.saturating_add(1)),
            Err(LimitError::TooManyBytes {
                limit: mebibytes,
                seen: mebibytes.saturating_add(1)
            }),
            "the default byte ceiling must be eight mebibytes"
        );
        Ok(())
    }
}
