//! The approval record I/O behind `Host::decide` and `Host::pending`: reading a
//! pending approval, archiving and appending a decision or a lapse, and the
//! foreign-tenant audit.

use crate::effect::RunId;
use crate::script::approval::{
    ApprovalRecord, AuditEntry, DecisionError, DecisionRecord, PendingRecord, RecordClass,
    classify_record, decision_key_for,
};
use crate::script::run_store::{Appended, Durable, Records};
use crate::script::{FlowError, StepKey};
use crate::task::StoreError;

use super::Host;

impl Host {
    /// The pending approval recorded under `key`, or why none reads there.
    ///
    /// Another tenant's run is the store's own refusal, audited here rather
    /// than answered.
    pub(super) fn pending_record(
        &self,
        records: &Records,
        run: RunId,
        key: StepKey,
        id: &str,
    ) -> Result<PendingRecord, DecisionError> {
        let tenant = self.inner.tenant.as_str();
        let stored = match records.lookup(tenant, run, key) {
            Ok(Some(stored)) => stored,
            Ok(None) => {
                lgwks_std::trace::debug!(id = %id, "decide: no pending record reads under this id");
                return Err(Self::no_pending(id));
            }
            Err(error) => {
                if let FlowError::Store { ref source, .. } = error
                    && let StoreError::ForeignTenant { ref owner, .. } = **source
                {
                    self.record_audit(AuditEntry::foreign(
                        self.inner.clock.now(),
                        tenant,
                        id,
                        owner,
                    ));
                    lgwks_std::trace::debug!(id = %id, owner = %owner, asked = %tenant, "decide: another tenant's approval is refused and audited");
                    return Err(DecisionError::ForeignTenant {
                        owner: owner.clone(),
                        asked: tenant.to_owned(),
                    });
                }
                lgwks_std::trace::debug!(error = %error, id = %id, "decide: the store refused the pending lookup");
                return Err(Self::store_refused(error));
            }
        };
        match classify_record(stored.bytes()) {
            RecordClass::Pending(pending) => Ok(pending),
            RecordClass::Decision(_) | RecordClass::Other => {
                lgwks_std::trace::debug!(id = %id, "decide: the record under this id is not a pending approval");
                Err(Self::no_pending(id))
            }
        }
    }

    /// Append `bytes` as the decision record of `pending`, under its decision key.
    pub(super) fn append_decision(
        &self,
        records: &Records,
        run: RunId,
        pending: &PendingRecord,
        bytes: Vec<u8>,
    ) -> Result<Appended, DecisionError> {
        let tenant = self.inner.tenant.as_str();
        let definition = self.identity_for(run, &self.definition("approval", 1, None, 1));
        records
            .0
            .append(
                tenant,
                run,
                decision_key_for(tenant, &pending.path),
                &format!("{}/decision", pending.path),
                &definition,
                bytes,
            )
            .map_err(Self::store_refused)
    }

    /// Archive `record` as a decision, the bytes a store frames it in.
    pub(super) fn decision_bytes(
        record: DecisionRecord,
        what: &str,
    ) -> Result<Vec<u8>, DecisionError> {
        ApprovalRecord::Decision(record)
            .to_record()
            .map_err(|cause| {
                Self::store_refused(FlowError::failed(format!(
                    "{what} could not be archived: {cause}"
                )))
            })
    }

    /// The store's refusal, as the decision error that carries it.
    pub(super) fn store_refused(error: FlowError) -> DecisionError {
        DecisionError::Store {
            source: Box::new(error),
        }
    }

    /// No pending approval reads as `id`.
    pub(super) fn no_pending(id: &str) -> DecisionError {
        DecisionError::NoPending { id: id.to_owned() }
    }

    /// Record the lapse of `pending` and report it as expired.
    ///
    /// The expiry decision a resume past the deadline records, so the lapse
    /// is a fact the ceiling and a later decide both read rather than a
    /// clock reading that moves.
    pub(super) fn record_lapse(
        &self,
        records: &Records,
        run: RunId,
        key: StepKey,
        pending: &PendingRecord,
        id: &str,
    ) -> Result<(), DecisionError> {
        let bytes = Self::decision_bytes(
            DecisionRecord {
                pending_key_hex: key.to_hex(),
                approved: false,
                expired: true,
                digest: pending.digest,
                by: String::from("expiry"),
            },
            "this approval's lapse",
        )?;
        match self.append_decision(records, run, pending, bytes)? {
            Appended::Recorded | Appended::AlreadyRecorded => {
                Err(DecisionError::Expired { id: id.to_owned() })
            }
            Appended::Conflicting => Err(DecisionError::AlreadyDecided { id: id.to_owned() }),
        }
    }

    /// Record a refused foreign-tenant decision in this host's audit.
    fn record_audit(&self, entry: AuditEntry) {
        match self.inner.audit.lock() {
            Ok(mut log) => log.record(entry),
            Err(poisoned) => poisoned.into_inner().record(entry),
        }
    }
}
