//! The 128-bit role ids of an effect key: one never-zero [`Id128`], the three
//! role newtypes over it, their wire form and the minting of the two a host
//! creates.
//!
//! Split from `effect.rs` because parsing and minting an id is its own job:
//! the key, the counters and the digests use only the public surface here.
//! The tuple fields are `pub(super)` so the parent and its siblings see exactly
//! what they saw when this lived in the parent.

use core::fmt;
use core::num::NonZeroU128;

/// Why an [`Id128`] could not be parsed.
///
/// `#[non_exhaustive]`: a later revision can add a rejection reason without
/// breaking a caller that matches the three below.
///
/// # Examples
///
/// ```
/// use lgwks_bot::effect::{Id128, IdError};
///
/// assert_eq!(Id128::from_hex("00"), Err(IdError::WrongLength { len: 2 }));
/// assert_eq!(Id128::from_hex(&"0".repeat(32)), Err(IdError::Zero));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IdError {
    /// The input was not exactly 32 characters.
    ///
    /// Length is checked in bytes and checked first, so a short or long input
    /// reports its own length rather than a character offset rebased onto a
    /// string that was never the right shape.
    WrongLength {
        /// Observed length in bytes.
        len: usize,
    },
    /// A byte at this offset was outside `[0-9a-f]`.
    ///
    /// Uppercase hex is rejected rather than folded. These ids are
    /// identity-bearing wire data, and the schema that defines them admits
    /// `^[0-9a-f]{32}$`; accepting `A` here would mean the crate writes a
    /// canonical form it also refuses to read back from certain producers.
    NotHex {
        /// Byte offset of the offending character.
        at: usize,
    },
    /// The input was the all-zero id.
    ///
    /// Zero is the schema's explicit exclusion. It is also the value a
    /// zero-initialised or truncated buffer produces, so refusing it turns a
    /// whole class of uninitialised-identity bugs into a parse error.
    Zero,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::WrongLength { len } => {
                write!(f, "identifier must be 32 hex characters, got {len}")
            }
            Self::NotHex { at } => {
                write!(f, "identifier has a non-lowercase-hex byte at offset {at}")
            }
            Self::Zero => f.write_str("the all-zero identifier is not a valid identifier"),
        }
    }
}

/// Why a fresh identifier could not be minted from the operating system.
///
/// Two arms, and neither has a fallback. [`lgwks_std::random`] enforces
/// INV-RANDOM-1, so a clock, a process id, a counter and a userspace
/// PRNG are each explicitly not alternatives — an identifier derived from one of
/// those is the collision this crate's identity model exists to make
/// impossible. A caller that cannot reach entropy must fail.
///
/// `#[non_exhaustive]` for the same reason [`IdError`] is: a later revision can
/// add a rejection reason without breaking a caller that matches these two.
///
/// # Examples
///
/// ```
/// use lgwks_bot::effect::{MintError, RunId};
///
/// fn fresh_run() -> Result<RunId, MintError> {
///     RunId::mint()
/// }
/// let run = fresh_run();
/// assert!(run.is_ok(), "the entropy source refused: {:?}", run.err());
/// ```
#[cfg(feature = "ephemeral")]
#[derive(Debug)]
#[non_exhaustive]
pub enum MintError {
    /// The operating system's entropy source could not be read.
    ///
    /// Carried rather than flattened to a string, because `lgwks_std`'s
    /// `EntropyError` names the backend that failed as a matchable value.
    Entropy(lgwks_std::random::EntropyError),
    /// The entropy source returned 128 zero bits.
    ///
    /// A probability of 2^-128 per call, and an error rather than a retry
    /// because zero is both the id the wire schema excludes and the value a
    /// zeroed or truncated buffer produces. A retry would silently paper over a
    /// source that is returning zeros; this names it.
    Zero,
}

/// A 128-bit identifier that is never zero.
///
/// The non-zero invariant is carried by [`NonZeroU128`] rather than checked at
/// each use, so "is this id real?" has one answer and the compiler enforces it.
/// Equality is the ordinary integer comparison: unlike
/// [`lgwks_std::hash::Digest`], these are public identifiers rather than secret-derived values,
/// and a constant-time comparison would buy nothing.
///
/// # Examples
///
/// ```
/// use lgwks_bot::effect::Id128;
///
/// let id = Id128::from_hex("0102030405060708090a0b0c0d0e0f10")?;
/// assert_eq!(id.to_hex(), "0102030405060708090a0b0c0d0e0f10");
/// # Ok::<(), lgwks_bot::effect::IdError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(
    feature = "wire",
    derive(
        lgwks_std::wire::Archive,
        lgwks_std::wire::Serialize,
        lgwks_std::wire::Deserialize
    )
)]
#[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
pub struct Id128(pub(super) NonZeroU128);

impl Id128 {
    /// Wrap a value already known to be non-zero.
    ///
    /// # Examples
    ///
    /// ```
    /// use core::num::NonZeroU128;
    /// use lgwks_bot::effect::Id128;
    ///
    /// assert_eq!(Id128::from_nonzero(NonZeroU128::MIN).get(), NonZeroU128::MIN);
    /// ```
    #[must_use]
    pub const fn from_nonzero(value: NonZeroU128) -> Self {
        Self(value)
    }

    /// Parse 32 lowercase hex characters, big-endian, rejecting the all-zero id.
    ///
    /// # Errors
    ///
    /// [`IdError::WrongLength`], [`IdError::NotHex`] or [`IdError::Zero`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::effect::{Id128, IdError};
    ///
    /// assert!(Id128::from_hex("0102030405060708090a0b0c0d0e0f10").is_ok());
    /// assert_eq!(
    ///     Id128::from_hex("0102030405060708090A0B0C0D0E0F10"),
    ///     Err(IdError::NotHex { at: 19 })
    /// );
    /// ```
    pub fn from_hex(text: &str) -> Result<Self, IdError> {
        if text.len() != 32 {
            let refusal = Err(IdError::WrongLength { len: text.len() });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "from_hex: returning an error to the caller");
            return refusal;
        }
        if let Some(at) = text
            .bytes()
            .position(|byte| !matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        {
            let refusal = Err(IdError::NotHex { at });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "from_hex: returning an error to the caller");
            return refusal;
        }
        // Unreachable in practice: the two checks above leave exactly 32 hex
        // characters, which is exactly 128 bits, so the value always fits a
        // `u128`. The arm exists because `from_str_radix` has an error channel
        // and this crate has no `unwrap` to spend on a proof it cannot state to
        // the compiler.
        // The length is checked above, so the only way this fails is a
        // non-hexadecimal character. `ParseIntError` would say "invalid digit
        // found in string", which names neither the field nor the width.
        let raw = match u128::from_str_radix(text, 16) {
            Ok(raw) => raw,
            Err(_not_hex) => {
                let refusal = Err(IdError::WrongLength { len: 32 });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "from_hex: returning an error to the caller");
                return refusal;
            }
        };
        match NonZeroU128::new(raw) {
            Some(value) => Ok(Self(value)),
            None => Err(IdError::Zero),
        }
    }

    /// The value as a non-zero integer.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::effect::Id128;
    ///
    /// let id = Id128::from_hex("000000000000000000000000000000ff")?;
    /// assert_eq!(id.get().get(), 255);
    /// # Ok::<(), lgwks_bot::effect::IdError>(())
    /// ```
    #[must_use]
    pub const fn get(self) -> NonZeroU128 {
        self.0
    }

    /// Lowercase hex, 32 characters, big-endian.
    ///
    /// # Examples
    ///
    /// ```
    /// use core::num::NonZeroU128;
    /// use lgwks_bot::effect::Id128;
    ///
    /// let id = Id128::from_nonzero(NonZeroU128::MIN);
    /// assert_eq!(id.to_hex(), "00000000000000000000000000000001");
    /// ```
    #[must_use]
    pub fn to_hex(self) -> String {
        lgwks_std::hex::encode(self.0.get().to_be_bytes())
    }

    /// Mint a fresh identifier from the operating system's entropy source.
    ///
    /// Big-endian, 128 bits, from [`lgwks_std::random`] — the estate's one
    /// CSPRNG backend. Distinctness is the entire property being bought: two
    /// runs of the same flow have to be distinguishable, and a value derived
    /// from a clock, a process id or a counter is exactly the kind that is not.
    ///
    /// # Errors
    ///
    /// [`MintError::Entropy`] when the source could not be read, and
    /// [`MintError::Zero`] when it returned 128 zero bits.
    ///
    /// `pub(crate)` because a caller outside this crate has no reason to hold a
    /// bare [`Id128`]: an identifier is meaningless without its role, and the
    /// two roles that are minted expose their own.
    #[cfg(feature = "ephemeral")]
    pub(crate) fn mint() -> Result<Self, MintError> {
        let raw = lgwks_std::random::bytes::<16>().map_err(MintError::Entropy)?;
        match NonZeroU128::new(u128::from_be_bytes(raw)) {
            Some(value) => Ok(Self(value)),
            None => Err(MintError::Zero),
        }
    }
}

/// Declare a newtype over [`Id128`] for one role in the effect key.
///
/// Three of the seven key fields are 128-bit ids with identical representation
/// and entirely different meanings. One macro rather than three written-out
/// copies, so the three cannot drift: a role that gains a parse rule or loses
/// its `Display` gains or loses it everywhere, in one edit.
macro_rules! id_role {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        #[cfg_attr(feature = "wire", derive(lgwks_std::wire::Archive, lgwks_std::wire::Serialize, lgwks_std::wire::Deserialize))]
        #[cfg_attr(feature = "wire", rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug)))]
        pub struct $name(pub(super) Id128);

        impl $name {
            /// Wrap an identifier already known to be non-zero.
            #[must_use]
            pub const fn new(id: Id128) -> Self {
                Self(id)
            }

            /// Parse the 32-character lowercase hex form.
            ///
            /// # Errors
            ///
            /// As [`Id128::from_hex`].
            pub fn from_hex(text: &str) -> Result<Self, IdError> {
                Ok(Self(Id128::from_hex(text)?))
            }

            /// The underlying identifier.
            #[must_use]
            pub const fn id(self) -> Id128 {
                self.0
            }

        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }
    };
}

id_role! {
    /// Identifies one run: a host-generated value persisted before the first
    /// admission, so that two runs of the same flow are distinguishable.
    RunId
}

id_role! {
    /// Identifies one logical intent.
    ///
    /// Distinct from a content digest on purpose: identical content does not
    /// imply identical intent, so two actions with the same [`ActionDigest`](super::ActionDigest) are
    /// still two actions and must not collapse into one settlement.
    ActionId
}

id_role! {
    /// Identifies the broker-created environment.
    ///
    /// Not a PID and not a display name. Both of those are reused by the
    /// operating system, so a settlement naming one can be delivered to a
    /// different process than the one it was about.
    EnvironmentId
}

/// A fresh run identity, minted from the operating system's entropy source.
///
/// Public, and on [`RunId`] rather than on the shared `id_role` macro,
/// because the three roles do not share this: a run id is the one a host
/// generates and persists before its first admission, and an [`ActionId`] is
/// already derived from a bot's structure by `crate::ecs::derive_action_id`.
/// A minting method on that role would be a second way to make one value, so
/// the macro carries none and each role states its own.
///
/// # Errors
///
/// [`MintError::Entropy`] when the source could not be read, and
/// [`MintError::Zero`] when it returned 128 zero bits.
#[cfg(feature = "ephemeral")]
impl RunId {
    /// Mint a run identity no other run can share.
    ///
    /// # Errors
    ///
    /// [`MintError::Entropy`] when the operating system's entropy source could
    /// not be read, and [`MintError::Zero`] when it returned 128 zero bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::effect::RunId;
    ///
    /// assert_ne!(RunId::mint()?, RunId::mint()?);
    /// # Ok::<(), lgwks_bot::effect::MintError>(())
    /// ```
    pub fn mint() -> Result<Self, MintError> {
        Ok(Self(Id128::mint()?))
    }
}

/// A fresh environment identity, minted from the operating system's entropy.
///
/// The host names the environment a run acts on; the broker is what creates
/// it. A name that is not the process's own pid or display name is what stops a
/// settlement from being delivered to a different process than the one it was
/// about, and entropy is the only source for a name with that property.
///
/// # Errors
///
/// [`MintError::Entropy`] when the source could not be read, and
/// [`MintError::Zero`] when it returned 128 zero bits.
#[cfg(feature = "ephemeral")]
impl EnvironmentId {
    /// Mint an environment identity no other environment can share.
    ///
    /// # Errors
    ///
    /// [`MintError::Entropy`] when the operating system's entropy source could
    /// not be read, and [`MintError::Zero`] when it returned 128 zero bits.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::effect::EnvironmentId;
    ///
    /// assert_ne!(EnvironmentId::mint()?, EnvironmentId::mint()?);
    /// # Ok::<(), lgwks_bot::effect::MintError>(())
    /// ```
    pub fn mint() -> Result<Self, MintError> {
        Ok(Self(Id128::mint()?))
    }
}
