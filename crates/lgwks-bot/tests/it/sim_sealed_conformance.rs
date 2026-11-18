//! Hostile providers against the conformance kit (#1060).
//!
//! Source: issue #1060. The kit is what a provider crate trusts to say it is
//! fit for the sealed store, so a kit that passes a broken provider ships that
//! provider's breakage to every host. Each simulation here wraps the real
//! `SoftwareProvider` in one hostile actor and runs the kit over many seeds.
//! The oracle is the rule the actor breaks, never the kit's own output: a liar
//! must be named as a round-trip mismatch, and a saboteur whose `destroy` lies
//! must be named as a destroyed key that still opens.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lgwks_bot::sealed::conformance::{self, Conformance, Violation};
use lgwks_bot::sealed::envelope::{DataKey, KeyMint};
use lgwks_bot::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// How many seeds each hostile actor faces.
const SEEDS: u64 = 128;

/// Which lie the hostile provider tells.
#[derive(Clone, Copy)]
enum Lie {
    /// The liar upstream: every unwrap hands back a key of fixed bytes, not
    /// the key the record was sealed under.
    WrongKey,
    /// The saboteur: `destroy` says yes and destroys nothing.
    FakeDestroy,
    /// The flaky upstream: the first provision is refused as transient, and
    /// every call after it is honest.
    RefuseFirst,
    /// The non-idempotent destroy: the first `destroy` of a key works, and a
    /// second one of the same key is refused instead of being a no-op.
    DestroyOnce,
    /// The dead upstream: every unwrap is refused as transient.
    RefuseUnwrap,
    /// The sealing upstream that will not wrap: every `wrap_key` is refused.
    RefuseWrap,
}

/// The real provider underneath, with one lie on top.
struct Hostile {
    /// Real keys, real files, real restarts.
    inner: SoftwareProvider,
    /// The lie this provider tells.
    lie: Lie,
    /// Provision calls this provider was asked, across its restart.
    asked: Arc<AtomicUsize>,
}

impl KeyProvider for Hostile {
    fn provision_with(&self, tenant: &str, mint: KeyMint) -> Result<(KeyId, DataKey), KeyError> {
        let call = self.asked.fetch_add(1, Ordering::SeqCst);
        if matches!(self.lie, Lie::RefuseFirst) && call == 0 {
            lgwks_std::trace::debug!(tenant, "hostile: the first provision is refused");
            return Err(KeyError::Transient {
                reason: "the upstream refused the first provision",
            });
        }
        self.inner.provision_with(tenant, mint)
    }

    fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError> {
        if matches!(self.lie, Lie::RefuseWrap) {
            lgwks_std::trace::debug!(tenant = id.tenant(), "hostile: every wrap is refused");
            return Err(KeyError::Transient {
                reason: "the upstream refused every wrap",
            });
        }
        self.inner.wrap_key(id)
    }

    fn unwrap_with(&self, id: &KeyId, wrapped: &[u8], mint: KeyMint) -> Result<DataKey, KeyError> {
        match self.lie {
            Lie::WrongKey => Ok(mint.key([0x5a; 32])),
            Lie::RefuseUnwrap => {
                lgwks_std::trace::debug!(tenant = id.tenant(), "hostile: every unwrap is refused");
                Err(KeyError::Transient {
                    reason: "the upstream refused every unwrap",
                })
            }
            Lie::FakeDestroy | Lie::RefuseFirst | Lie::DestroyOnce | Lie::RefuseWrap => {
                self.inner.unwrap_with(id, wrapped, mint)
            }
        }
    }

    fn destroy(&self, id: &KeyId) -> Result<(), KeyError> {
        match self.lie {
            Lie::WrongKey | Lie::RefuseFirst | Lie::RefuseUnwrap | Lie::RefuseWrap => {
                self.inner.destroy(id)
            }
            Lie::FakeDestroy => Ok(()),
            // A key that no longer wraps was destroyed already: refuse again.
            Lie::DestroyOnce => match self.inner.wrap_key(id) {
                Ok(_live) => self.inner.destroy(id),
                Err(gone) => {
                    lgwks_std::trace::debug!(error = %gone, "hostile: a second destroy is refused");
                    Err(gone)
                }
            },
        }
    }

    fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
        self.inner.note_wrapped(id, wrapped)
    }
}

/// Run the kit against `lie` for one seed and return what it found.
fn run(lie: Lie, seed: u64) -> Result<Conformance, Box<dyn std::error::Error>> {
    Ok(run_counted(lie, seed)?.0)
}

/// Run the kit against `lie` for one seed: what it found, and how many
/// provisions the provider was asked.
fn run_counted(lie: Lie, seed: u64) -> Result<(Conformance, usize), Box<dyn std::error::Error>> {
    let scratch = Scratch::new("sim-kit-hostile")?;
    let dir = scratch.path().to_path_buf();
    let asked = Arc::new(AtomicUsize::new(0));
    let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> {
        Ok(Arc::new(Hostile {
            inner: SoftwareProvider::open(&dir)?,
            lie,
            asked: Arc::clone(&asked),
        }))
    };
    let found = conformance::check(&mut open, seed);
    Ok((found, asked.load(Ordering::SeqCst)))
}

/// A destroy that is not idempotent is named on every seed whose coin
/// destroys a key, and nothing else is named: crypto-shredding a key twice
/// (a retried erasure request) must not fail the second time.
#[test]
fn sim_conformance_a_second_destroy_refused_is_named() -> TestResult {
    let mut caught = 0_u64;
    for seed in 0..SEEDS {
        let found = run(Lie::DestroyOnce, seed)?;
        let named = found.violations().contains(&Violation::DestroyRefused);
        if named {
            caught = caught.saturating_add(1);
        }
        assert!(
            found
                .violations()
                .iter()
                .all(|violation| *violation == Violation::DestroyRefused),
            "seed {seed}: only the refused second destroy is named: {:?}",
            found.violations()
        );
    }
    assert!(
        caught.saturating_mul(8) > SEEDS.saturating_mul(7),
        "the refused second destroy was named on {caught} of {SEEDS} seeds"
    );
    Ok(())
}

/// An upstream that refuses every unwrap is never passed: a key the store
/// cannot get back is a round trip that failed, on every seed.
#[test]
fn sim_conformance_an_upstream_that_refuses_every_unwrap_is_never_passed() -> TestResult {
    for seed in 0..SEEDS {
        let found = run(Lie::RefuseUnwrap, seed)?;
        assert!(
            found.violations().contains(&Violation::RoundTripMismatch),
            "seed {seed}: a refused unwrap is a round-trip mismatch: {:?}",
            found.violations()
        );
    }
    Ok(())
}

/// An upstream that will not wrap fails every record's round trip, and the
/// kit names each one and keeps going: the provider is asked for every record
/// the seed plans, at least two, never only the first.
#[test]
fn sim_conformance_a_refused_wrap_is_named_for_every_record() -> TestResult {
    for seed in 0..SEEDS {
        let (found, asked) = run_counted(Lie::RefuseWrap, seed)?;
        assert!(
            found
                .violations()
                .iter()
                .all(|violation| *violation == Violation::RoundTripMismatch),
            "seed {seed}: only the round trip is named: {:?}",
            found.violations()
        );
        assert_eq!(
            found.violations().len(),
            asked,
            "seed {seed}: one mismatch per record the provider was asked for"
        );
        assert!(
            asked >= 2,
            "seed {seed}: the kit kept asking, asked {asked}"
        );
    }
    Ok(())
}

/// Every rule the kit can name reads as its own plain sentence, so a person
/// reading `store-conformance` or a provider's CI log learns which rule broke
/// without knowing the enum: no two read alike and none is a Rust name.
#[test]
fn sim_conformance_each_violation_reads_as_a_sentence() -> TestResult {
    let all = [
        Violation::OpenRefused,
        Violation::ProvisionRefused,
        Violation::DuplicateIdentity,
        Violation::WrongTenant,
        Violation::RoundTripMismatch,
        Violation::ForeignTenantOpens,
        Violation::DestroyRefused,
        Violation::DestroyedStillOpens,
        Violation::NoteRefused,
        Violation::RestartLostKey,
    ];
    let mut seen = std::collections::HashSet::new();
    for violation in &all {
        let sentence = violation.to_string();
        assert!(
            sentence.contains(' ') && !sentence.contains("::"),
            "{violation:?} reads as a sentence, not a name: {sentence:?}"
        );
        assert_ne!(
            sentence,
            format!("{violation:?}"),
            "{violation:?} is not its Debug name"
        );
        assert!(
            seen.insert(sentence),
            "{violation:?} reads unlike every other rule"
        );
    }
    Ok(())
}

/// `mint_ids` asks the provider for up to 4,096 keys and refuses past that
/// before the provider is asked at all. The provider refuses its first call,
/// so the oracle is its own count of calls, never the kit's answer.
#[test]
fn sim_conformance_mint_ids_asks_up_to_its_bound_and_never_past() -> TestResult {
    let scratch = Scratch::new("sim-kit-mint-bound")?;
    for (count, expected) in [(4096_usize, 1_usize), (4097, 0)] {
        let asked = Arc::new(AtomicUsize::new(0));
        let provider = Hostile {
            inner: SoftwareProvider::open(scratch.path())?,
            lie: Lie::RefuseFirst,
            asked: Arc::clone(&asked),
        };
        let refused = conformance::mint_ids(&provider, "kit-bound", count);
        assert!(refused.is_err(), "count {count}: the batch is refused");
        assert_eq!(
            asked.load(Ordering::SeqCst),
            expected,
            "count {count}: provision calls before the refusal"
        );
    }
    Ok(())
}

/// A flaky upstream that refuses one provision is named for it once, and the
/// kit keeps asking: a seed provisions at least two records, so the provider
/// is asked again and every later record is still checked and passes.
#[test]
fn sim_conformance_one_refused_provision_does_not_stop_the_kit() -> TestResult {
    for seed in 0..SEEDS {
        let (found, asked) = run_counted(Lie::RefuseFirst, seed)?;
        assert_eq!(
            found.violations(),
            [Violation::ProvisionRefused].as_slice(),
            "seed {seed}: only the one refusal is named"
        );
        assert!(
            asked >= 2,
            "seed {seed}: the kit asked again after the refusal, asked {asked}"
        );
    }
    Ok(())
}

/// The liar upstream is caught on every seed: a key that does not open its
/// own seal is a round-trip mismatch, never a pass, and the liar's honest
/// destroys are never blamed.
#[test]
fn sim_conformance_liar_upstream_wrong_key_is_never_passed() -> TestResult {
    for seed in 0..SEEDS {
        let found = run(Lie::WrongKey, seed)?;
        assert!(
            found.violations().contains(&Violation::RoundTripMismatch),
            "seed {seed}: the wrong key is named as a round-trip mismatch: {:?}",
            found.violations()
        );
        assert!(
            !found.violations().contains(&Violation::DestroyRefused),
            "seed {seed}: the liar's real destroys are not blamed: {:?}",
            found.violations()
        );
    }
    Ok(())
}

/// The saboteur is named on every seed that destroys a key, and only the
/// broken rule is named: a seed whose coin destroys nothing passes, because
/// the saboteur told no lie the kit could meet. A seed spares each record
/// past the first on its own coin, so with 2 to 31 records nearly every seed
/// destroys one: more than 7 in 8 must name the lie.
#[test]
fn sim_conformance_saboteur_fake_destroy_is_named_when_a_key_is_destroyed() -> TestResult {
    let mut caught = 0_u64;
    for seed in 0..SEEDS {
        let found = run(Lie::FakeDestroy, seed)?;
        let named = found.violations().contains(&Violation::DestroyedStillOpens);
        if named {
            caught = caught.saturating_add(1);
        }
        assert!(
            found
                .violations()
                .iter()
                .all(|violation| *violation == Violation::DestroyedStillOpens),
            "seed {seed}: only the fake destroy is named: {:?}",
            found.violations()
        );
        assert_eq!(
            found.passed(),
            !named,
            "seed {seed}: a pass means no lie was met"
        );
    }
    assert!(
        caught.saturating_mul(8) > SEEDS.saturating_mul(7),
        "the saboteur was named on {caught} of {SEEDS} seeds"
    );
    Ok(())
}
