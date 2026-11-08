//! The broker's freshness half: what an authority source reports, what was withdrawn
//! by name, how many reaches may happen offline, and taking over a consumer's history.

use super::{Authority, Broker, BrokerError, Environment, MAX_REVOKED_OPERATIONS, OfflineExposure};
use crate::effect::{ActionId, EnvironmentEpoch, EnvironmentId};
use crate::journal::JournalError;
#[cfg(feature = "wire")]
use crate::journal::{Durable, RecordJournal};

impl Broker {
    /// [`Self::adopt`] over a consumer's own record journal.
    ///
    /// `epoch_of` reads the environment and generation a record names, and `None`
    /// for a record about something else. Only the live generation of the journal
    /// is read, so a consumer that continues its journal carries the highest epoch
    /// in the checkpoint: a checkpoint that drops it would move this takeover
    /// backwards, which is the failure adoption exists to prevent.
    ///
    /// # Errors
    ///
    /// As [`Self::adopt`], and [`BrokerError::NothingToAdopt`] when no record names
    /// a generation.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use core::num::NonZeroU64;
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    /// use lgwks_bot::journal::RecordJournal;
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let path = std::env::temp_dir().join(format!("broker-doc-{}", std::process::id()));
    /// # drop(std::fs::remove_file(&path));
    /// let mut journal = RecordJournal::<u64>::open(&path)?;
    /// journal.compare_and_append(journal.tail(), &4)?;
    ///
    /// let mut broker = Broker::new();
    /// let adopted = broker.adopt_records(id, &journal, |generation| {
    ///     NonZeroU64::new(*generation).map(|at| (id, EnvironmentEpoch::new(at)))
    /// })?;
    /// assert_eq!(adopted.get(), 5, "the new owner moves past what it read");
    /// # drop(journal);
    /// # drop(std::fs::remove_file(&path));
    /// # Ok(())
    /// # }
    /// ```
    #[cfg(feature = "wire")]
    pub fn adopt_records<R: Durable>(
        &mut self,
        id: EnvironmentId,
        journal: &RecordJournal<R>,
        epoch_of: impl Fn(&R) -> Option<(EnvironmentId, EnvironmentEpoch)>,
    ) -> Result<EnvironmentEpoch, BrokerError> {
        self.adopt_from(id, || {
            Ok(journal
                .replay()?
                .map(|entry| entry.map(|read| epoch_of(read.record()))))
        })
    }

    /// Establish freshness: the authority source was reached and reports the
    /// generation it stands at.
    ///
    /// The broker moves to the later of the two and never back, in one step however
    /// far apart they are, so a replayed older snapshot is absorbed rather than
    /// obeyed. The spent count clears, because this call is the source being
    /// reached and nothing else clears it. Returns the generation it now holds.
    ///
    /// # Errors
    ///
    /// [`BrokerError::UnknownEnvironment`] or [`BrokerError::Closed`].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// let reported = EnvironmentEpoch::from_decimal("9")?;
    /// assert_eq!(broker.authenticate(id, reported)?, reported);
    /// let older = EnvironmentEpoch::from_decimal("2")?;
    /// assert_eq!(broker.authenticate(id, older)?, reported, "never back");
    /// # Ok(())
    /// # }
    /// ```
    pub fn authenticate(
        &mut self,
        id: EnvironmentId,
        reported: EnvironmentEpoch,
    ) -> Result<EnvironmentEpoch, BrokerError> {
        let environment = self.known_mut(id)?;
        if !environment.open {
            let refusal = Err(BrokerError::Closed { id });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "authenticate: returning an error to the caller");
            return refusal;
        }
        if reported > environment.epoch {
            environment.epoch = reported;
        }
        if let Some(ref mut offline) = environment.offline {
            offline.spent = 0;
        }
        Ok(environment.epoch)
    }

    /// Withdraw one operation's authority by name, for the life of this broker.
    ///
    /// Returns whether this call was the one that withdrew it. There is no way back,
    /// because restoring authority is a new authorization of its own, and erasing a
    /// withdrawal would let a replay of the earlier state bring the effect back. A
    /// closed environment still accepts one, since a withdrawal should never fail
    /// for being late.
    ///
    /// # Errors
    ///
    /// [`BrokerError::UnknownEnvironment`], or [`BrokerError::RevocationsFull`] when
    /// the operation is new and the environment holds all it can.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// use lgwks_bot::effect::ActionId;
    ///
    /// let action = ActionId::from_hex("1112131415161718191a1b1c1d1e1f20")?;
    /// assert!(broker.revoke_operation(id, action)?);
    /// assert!(!broker.revoke_operation(id, action)?, "the second is a no-op");
    /// # Ok(())
    /// # }
    /// ```
    pub fn revoke_operation(
        &mut self,
        id: EnvironmentId,
        action: ActionId,
    ) -> Result<bool, BrokerError> {
        let environment = self.known_mut(id)?;
        if environment.revoked.contains(&action) {
            return Ok(false);
        }
        if environment.revoked.len() >= MAX_REVOKED_OPERATIONS {
            let refusal = Err(BrokerError::RevocationsFull {
                environment: id,
                limit: MAX_REVOKED_OPERATIONS,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "revoke_operation: returning an error to the caller");
            return refusal;
        }
        Ok(environment.revoked.insert(action))
    }

    /// Whether `action` was withdrawn by name on `id`.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// use lgwks_bot::effect::ActionId;
    ///
    /// let action = ActionId::from_hex("1112131415161718191a1b1c1d1e1f20")?;
    /// assert!(!broker.is_revoked(id, action));
    /// broker.revoke_operation(id, action)?;
    /// assert!(broker.is_revoked(id, action));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn is_revoked(&self, id: EnvironmentId, action: ActionId) -> bool {
        self.environments
            .get(&id)
            .is_some_and(|environment| environment.revoked.contains(&action))
    }

    /// Cap the mutating reaches allowed between contacts with the authority source.
    ///
    /// A bound of zero is a kill switch rather than a mistake: every reach is
    /// refused, a fresh authentication included, because the bound is a ceiling and
    /// not a credit. What was already spent stays spent, so a host cannot recharge
    /// by setting the bound again. Until this is called nothing is counted.
    ///
    /// # Errors
    ///
    /// [`BrokerError::UnknownEnvironment`].
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// broker.set_offline_bound(id, 4)?;
    /// assert_eq!(broker.offline_bound(id), Some(4));
    /// # Ok(())
    /// # }
    /// ```
    pub fn set_offline_bound(&mut self, id: EnvironmentId, bound: u32) -> Result<(), BrokerError> {
        let environment = self.known_mut(id)?;
        let spent = match environment.offline {
            Some(offline) => offline.spent,
            None => 0,
        };
        environment.offline = Some(OfflineExposure { bound, spent });
        Ok(())
    }

    /// The ceiling on `id`, or `None` when nothing is counted there.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// assert_eq!(broker.offline_bound(id), None);
    /// broker.set_offline_bound(id, 2)?;
    /// assert_eq!(broker.offline_bound(id), Some(2));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn offline_bound(&self, id: EnvironmentId) -> Option<u32> {
        let offline = self.environments.get(&id)?.offline?;
        Some(offline.bound)
    }

    /// The reaches spent on `id` since the last authentication, or `None` when
    /// nothing is counted there.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::Broker;
    /// use lgwks_bot::effect::{EnvironmentEpoch, EnvironmentId};
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let mut broker = Broker::new();
    /// broker.register(id)?;
    /// broker.set_offline_bound(id, 2)?;
    /// assert_eq!(broker.offline_spent(id), Some(0));
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn offline_spent(&self, id: EnvironmentId) -> Option<u32> {
        let offline = self.environments.get(&id)?.offline?;
        Some(offline.spent)
    }

    /// Check a warrant one last time and count the mutating reach it is about to make.
    ///
    /// The check and the count are one call, so nothing can slip past the ceiling
    /// between them. Call it immediately before the reach, after any revalidation,
    /// and never for a status query: observing what already happened spends nothing.
    /// With no bound set it checks and counts nothing further.
    ///
    /// # Errors
    ///
    /// As [`Self::authorize`]. A refusal counts nothing.
    ///
    /// # Examples
    ///
    /// ```
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// use lgwks_bot::broker::{Broker, BrokerError};
    /// use lgwks_bot::effect::{
    ///     ActionDigest, ActionId, AttemptId, EffectIdentity, EnvironmentId, FlowRevision, RunId,
    /// };
    ///
    /// let id = EnvironmentId::from_hex("2122232425262728292a2b2c2d2e2f30")?;
    /// let zeroes = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    /// let mut broker = Broker::new();
    /// let generation = broker.register(id)?;
    /// broker.set_offline_bound(id, 1)?;
    /// let key = EffectIdentity::new(
    ///     RunId::from_hex("0102030405060708090a0b0c0d0e0f10")?,
    ///     id,
    ///     FlowRevision::from_tagged("blake3_256", zeroes)?,
    /// )
    /// .key(
    ///     ActionId::from_hex("1112131415161718191a1b1c1d1e1f20")?,
    ///     AttemptId::from_decimal("1")?,
    ///     ActionDigest::from_tagged("blake3_256", zeroes)?,
    ///     generation,
    /// );
    /// let warrant = broker.authorize(key)?;
    /// broker.spend(&warrant)?;
    /// assert!(matches!(
    ///     broker.spend(&warrant),
    ///     Err(BrokerError::OfflineExposureExhausted { bound: 1, .. })
    /// ));
    /// # Ok(())
    /// # }
    /// ```
    pub fn spend(&mut self, authority: &Authority) -> Result<(), BrokerError> {
        let id = authority.environment();
        self.check_warrant(id, authority.action(), authority.epoch())?;
        let environment = self.known_mut(id)?;
        if let Some(ref mut offline) = environment.offline {
            offline.spent = offline.spent.saturating_add(1);
        }
        Ok(())
    }

    /// The one takeover: read what a history names, then claim the generation after
    /// the highest. Every adoption is this function, so none can fence differently.
    pub(super) fn adopt_from<I>(
        &mut self,
        id: EnvironmentId,
        read: impl FnOnce() -> Result<I, JournalError>,
    ) -> Result<EnvironmentEpoch, BrokerError>
    where
        I: Iterator<Item = Result<Option<(EnvironmentId, EnvironmentEpoch)>, JournalError>>,
    {
        if self.environments.contains_key(&id) {
            let refusal = Err(BrokerError::AlreadyRegistered { id });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "adopt: returning an error to the caller");
            return refusal;
        }
        let mut highest = None;
        for named in read().map_err(BrokerError::Journal)? {
            let Some((environment, epoch)) = named.map_err(BrokerError::Journal)? else {
                continue;
            };
            if environment != id {
                let refusal = Err(BrokerError::ForeignEnvironment {
                    asked: id,
                    named: environment,
                });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "adopt: returning an error to the caller");
                return refusal;
            }
            highest = Some(match highest {
                None => epoch,
                Some(current) if epoch > current => epoch,
                Some(current) => current,
            });
        }
        let Some(highest) = highest else {
            let refusal = Err(BrokerError::NothingToAdopt { id });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "adopt: returning an error to the caller");
            return refusal;
        };
        let next = highest
            .checked_next()
            .ok_or(BrokerError::Exhausted { id })?;
        self.environments.insert(id, Environment::at(next));
        Ok(next)
    }

    /// The generation check, run both when a warrant is minted and when it is
    /// presented again.
    ///
    /// One function because the two callers must agree exactly. A second copy
    /// of this comparison that drifted would mean a warrant the broker would
    /// refuse to mint and would accept at the boundary, which is the failure
    /// fencing exists to prevent. The order is fixed, so an attempt that is revoked
    /// and stale and out of allowance always reports the same one reason.
    pub(super) fn check_warrant(
        &self,
        id: EnvironmentId,
        action: ActionId,
        presented: EnvironmentEpoch,
    ) -> Result<EnvironmentEpoch, BrokerError> {
        let environment = self
            .environments
            .get(&id)
            .ok_or(BrokerError::UnknownEnvironment { id })?;
        if !environment.open {
            let refusal = Err(BrokerError::Closed { id });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_warrant: returning an error to the caller");
            return refusal;
        }
        if environment.revoked.contains(&action) {
            let refusal = Err(BrokerError::Revoked {
                environment: id,
                action,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_warrant: returning an error to the caller");
            return refusal;
        }
        let current = environment.epoch;
        if presented.get() > current.get() {
            let refusal = Err(BrokerError::NeverIssued {
                environment: id,
                presented,
                current,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_warrant: returning an error to the caller");
            return refusal;
        }
        if presented != current {
            let refusal = Err(BrokerError::Superseded {
                environment: id,
                presented,
                current,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_warrant: returning an error to the caller");
            return refusal;
        }
        if let Some(offline) = environment.offline
            && offline.spent >= offline.bound
        {
            let refusal = Err(BrokerError::OfflineExposureExhausted {
                environment: id,
                bound: offline.bound,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "check_warrant: returning an error to the caller");
            return refusal;
        }
        Ok(current)
    }
}
