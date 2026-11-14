//! The per-tenant budget ledger of the dispatcher.
//!
//! The runner spends one unit of a tenant's budget for each effect it takes to
//! serve, an `Ask` or a `Command` alike, whether or not the provider answers: a
//! client that retries a failing effect pays for each try. An effect past the
//! budget is answered [`EffectResult::Refused`](crate::cell::EffectResult) naming
//! [`BUDGET_CAP`], and never reaches the provider or the command pool.
//!
//! A tenant owns a fixed window on the injected [`Clock`]: `allowance` units
//! per `window`, counted from the tenant's first charge, then from the first
//! charge after the window ends. A fixed window is exact integer state with no
//! fractions to round, so a replay gives the same verdicts. The cost is that a
//! tenant may spend its allowance at the end of one window and again at the
//! start of the next. A clock that reads earlier than a window's start never
//! refunds: the window stays open.
//!
//! The table holds at most `tenants` windows. When it is full, windows that have
//! ended are dropped, which loses nothing because an ended window equals a
//! fresh one. If it is still full the new tenant is refused: a flood of names
//! cannot evict a live charge, and the host's own tenant ceiling bounds the
//! names that reach here at all.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use crate::clock::Clock;
use crate::ecs::{MAX_TENANTS, valid_tenant};

/// The cap an effect past its tenant's budget is refused for lacking.
pub(super) const BUDGET_CAP: &str = "app.budget";

/// A unit allowance per window, and the most tenants the ledger tracks.
///
/// # Examples
///
/// ```
/// use std::time::Duration;
///
/// use lgwks_bot::app::{App, BudgetLimits};
/// use lgwks_bot::clock::Clock;
///
/// let limits = BudgetLimits::new(100, Duration::from_secs(60), 1_024);
/// let app = App::new("metered").budget(limits, Clock::wall());
/// assert!(app.check().is_ok());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetLimits {
    /// Units one tenant may spend per window.
    allowance: u32,
    /// How long one window lasts.
    window: Duration,
    /// The most tenants tracked at once.
    tenants: usize,
}

impl BudgetLimits {
    /// `allowance` units per tenant per `window`, over at most `tenants` tenants.
    ///
    /// [`App::check`](super::App::check) refuses a zero window or zero tenants.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    ///
    /// use lgwks_bot::app::BudgetLimits;
    ///
    /// let per_minute = BudgetLimits::new(600, Duration::from_secs(60), 4_096);
    /// assert_eq!(per_minute, BudgetLimits::new(600, Duration::from_secs(60), 4_096));
    /// ```
    #[must_use]
    pub fn new(allowance: u32, window: Duration, tenants: usize) -> Self {
        Self {
            allowance,
            window,
            tenants,
        }
    }

    /// Whether the ledger can run under these limits.
    pub(super) fn usable(&self) -> bool {
        !self.window.is_zero() && self.tenants > 0
    }
}

impl Default for BudgetLimits {
    /// A thousand units a minute for each of the host's tenants.
    fn default() -> Self {
        Self::new(1_000, Duration::from_secs(60), MAX_TENANTS)
    }
}

/// What the ledger decided for one effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Charge {
    /// One unit was spent; the effect may run.
    Granted,
    /// The tenant spent its allowance for this window.
    Spent,
    /// The ledger tracks its most tenants and this one is new.
    TableFull,
    /// The tenant is not a valid name, so it keys nothing.
    BadTenant,
}

/// One tenant's window.
#[derive(Debug, Clone, Copy)]
struct Window {
    /// When the window opened, on the ledger's clock.
    start: Duration,
    /// Units spent in it.
    used: u32,
}

impl Window {
    /// Whether the window had ended by `now`.
    fn ended(&self, now: Duration, window: Duration) -> bool {
        now.saturating_sub(self.start) >= window
    }

    /// Spend a unit at `now`, opening a new window first if the last one ended.
    fn take(&mut self, now: Duration, limits: &BudgetLimits) -> Charge {
        if self.ended(now, limits.window) {
            self.start = now;
            self.used = 0;
        }
        if self.used < limits.allowance {
            self.used = self.used.saturating_add(1);
            Charge::Granted
        } else {
            Charge::Spent
        }
    }
}

/// The windows, and a lower bound on when the first of them ends.
#[derive(Debug)]
struct Table {
    /// One window per tenant.
    windows: HashMap<Box<str>, Window>,
    /// No window ends before this, so a sweep before it finds nothing.
    next_end: Duration,
}

impl Table {
    /// Drop every window that ended by `now` and recompute `next_end` exactly.
    fn sweep(&mut self, now: Duration, window: Duration) {
        self.windows.retain(|_, held| !held.ended(now, window));
        let first = self
            .windows
            .values()
            .map(|held| held.start.saturating_add(window))
            .min();
        self.next_end = match first {
            Some(end) => end,
            None => Duration::MAX,
        };
    }

    /// Charge `tenant` one unit at `now`.
    fn charge(&mut self, tenant: &str, now: Duration, limits: &BudgetLimits) -> Charge {
        if let Some(held) = self.windows.get_mut(tenant) {
            return held.take(now, limits);
        }
        if self.windows.len() >= limits.tenants && now >= self.next_end {
            self.sweep(now, limits.window);
        }
        if self.windows.len() >= limits.tenants {
            return Charge::TableFull;
        }
        let mut fresh = Window {
            start: now,
            used: 0,
        };
        let charge = fresh.take(now, limits);
        self.next_end = self.next_end.min(now.saturating_add(limits.window));
        self.windows.insert(tenant.into(), fresh);
        charge
    }
}

/// The ledger: every tenant's window under one lock, read against one clock.
///
/// The lock is held for a map lookup and a few integer steps, never across
/// provider work, so the workers that share it wait nanoseconds.
#[derive(Debug)]
pub(super) struct Ledger {
    /// The bounds in force.
    limits: BudgetLimits,
    /// The clock windows are read on.
    clock: Clock,
    /// The windows.
    table: Mutex<Table>,
}

impl Ledger {
    /// An empty ledger over `limits`, read on `clock`.
    pub(super) fn new(limits: BudgetLimits, clock: Clock) -> Self {
        Self {
            limits,
            clock,
            table: Mutex::new(Table {
                windows: HashMap::new(),
                next_end: Duration::MAX,
            }),
        }
    }

    /// The table under its lock. A poisoned lock still holds whole integers,
    /// since no step under it can panic part-way, so its table is used as it stands.
    fn held(&self) -> MutexGuard<'_, Table> {
        match self.table.lock() {
            Ok(table) => table,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Spend one unit of `tenant`'s budget.
    pub(super) fn charge(&self, tenant: &str) -> Charge {
        if !valid_tenant(tenant) {
            return Charge::BadTenant;
        }
        let now = self.clock.now();
        self.held().charge(tenant, now, &self.limits)
    }

    /// How many tenants hold a window. Used by the simulation to check the bound.
    #[cfg(test)]
    pub(super) fn tracked(&self) -> usize {
        self.held().windows.len()
    }
}
