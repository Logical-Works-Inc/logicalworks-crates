//! The plans the store holds, and the bounds that keep one tenant from crowding out another (#889).
//!
//! Every tenant can always hold `TENANT_FLOOR` plans, whatever its neighbours
//! hold. Plans past the floor draw on one shared pool, and a tenant holds at
//! most `MAX_APPS_PER_TENANT` in all. The floors are carved out of the total
//! before the pool is sized, so the pool can never eat a floor.

use std::collections::HashMap;

use super::store::StoredPlan;

/// The most plans one tenant may hold.
const MAX_APPS_PER_TENANT: usize = 256;

/// The most distinct tenants the store holds plans for.
const MAX_TENANTS: usize = 1_024;

/// The plans each tenant can always hold: no neighbour can take them.
const TENANT_FLOOR: usize = 4;

/// The most plans the store holds for every tenant together.
const MAX_APPS_TOTAL: usize = 8_192;

/// The plans every tenant shares past its own floor.
const SHARED_POOL: usize = MAX_APPS_TOTAL - MAX_TENANTS * TENANT_FLOOR;

const _: () = assert!(
    MAX_TENANTS * TENANT_FLOOR < MAX_APPS_TOTAL,
    "the floors must leave the shared pool room"
);

/// Why the store would not take a plan it did not already hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Denied {
    /// The tenant holds its ceiling.
    Tenant {
        /// The tenant.
        tenant: String,
        /// The plans it holds.
        held: usize,
    },
    /// The store holds plans for as many tenants as it may, and this is a new one.
    Tenants {
        /// The tenants holding plans.
        count: usize,
    },
    /// The tenant is past its floor and the shared pool is spent.
    Pool {
        /// The plans held past every tenant's floor.
        shared: usize,
    },
}

impl Denied {
    /// The refusal text the edge answers.
    pub(super) fn message(&self) -> String {
        match *self {
            Self::Tenant { ref tenant, held } => format!(
                "tenant {tenant:?} holds {held} plans, past the ceiling of {MAX_APPS_PER_TENANT}"
            ),
            Self::Tenants { count } => format!(
                "the store holds plans for {count} tenants, past the ceiling of {MAX_TENANTS}"
            ),
            Self::Pool { shared } => format!(
                "the shared pool holds {shared} plans past each tenant's floor, past its ceiling of {SHARED_POOL}"
            ),
        }
    }
}

/// The plans by `(tenant, id)`, with the counts that answer admission in O(1).
#[derive(Default)]
pub(super) struct Held {
    /// The plans by `(tenant, id)`.
    plans: HashMap<(String, String), StoredPlan>,
    /// How many plans each tenant holds; a tenant with none has no entry.
    per_tenant: HashMap<String, usize>,
    /// The plans held above each tenant's floor, summed.
    shared: usize,
}

impl Held {
    /// The plan held under `(tenant, id)`.
    pub(super) fn plan(&self, tenant: &str, id: &str) -> Option<&StoredPlan> {
        self.plans.get(&(tenant.to_owned(), id.to_owned()))
    }

    /// Hold the plan `make` builds under `key`, unless the bounds refuse it.
    ///
    /// A key already held is admitted again unchanged and `make` never runs.
    pub(super) fn admit(
        &mut self,
        key: (String, String),
        make: impl FnOnce() -> StoredPlan,
    ) -> Result<(), Denied> {
        if self.plans.contains_key(&key) {
            return Ok(());
        }
        let held = match self.per_tenant.get(key.0.as_str()) {
            Some(&count) => count,
            None => 0,
        };
        if let Some(denied) = self.refusal(&key.0, held) {
            lgwks_std::trace::debug!(
                tenant = key.0.as_str(),
                denied = ?denied,
                "apps: the store refuses the plan"
            );
            return Err(denied);
        }
        if held >= TENANT_FLOOR {
            self.shared = self.shared.saturating_add(1);
        }
        let count = self.per_tenant.entry(key.0.clone()).or_insert(0);
        *count = count.saturating_add(1);
        self.plans.insert(key, make());
        Ok(())
    }

    /// The bound a new plan for `tenant`, which holds `held`, would break.
    fn refusal(&self, tenant: &str, held: usize) -> Option<Denied> {
        if held >= MAX_APPS_PER_TENANT {
            return Some(Denied::Tenant {
                tenant: tenant.to_owned(),
                held,
            });
        }
        if held == 0 && self.per_tenant.len() >= MAX_TENANTS {
            return Some(Denied::Tenants {
                count: self.per_tenant.len(),
            });
        }
        if held >= TENANT_FLOOR && self.shared >= SHARED_POOL {
            return Some(Denied::Pool {
                shared: self.shared,
            });
        }
        None
    }
}
