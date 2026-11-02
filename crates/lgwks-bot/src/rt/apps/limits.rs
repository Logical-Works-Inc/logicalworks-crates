//! The ceilings one plan admission is measured against.

// ── Limits ───────────────────────────────────────────────────────────────────

/// The ceilings one plan admission is measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanLimits {
    /// The most bytes one plan body may occupy.
    max_bytes: usize,
    /// The deepest nesting of steps admitted.
    max_depth: usize,
    /// The most steps admitted, at every level together.
    max_steps: usize,
    /// The most children of one fan-out step (`each`, `graph`, `together`).
    max_fan_out: usize,
}

impl PlanLimits {
    /// Limits of exactly `max_bytes`, `max_depth`, `max_steps` and
    /// `max_fan_out`.
    #[must_use]
    pub const fn new(
        max_bytes: usize,
        max_depth: usize,
        max_steps: usize,
        max_fan_out: usize,
    ) -> Self {
        Self {
            max_bytes,
            max_depth,
            max_steps,
            max_fan_out,
        }
    }

    /// These limits with the body ceiling replaced.
    #[must_use]
    pub const fn with_max_bytes(mut self, max_bytes: usize) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// These limits with the nesting ceiling replaced.
    #[must_use]
    pub const fn with_max_depth(mut self, max_depth: usize) -> Self {
        self.max_depth = max_depth;
        self
    }

    /// These limits with the step-count ceiling replaced.
    #[must_use]
    pub const fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps;
        self
    }

    /// These limits with the fan-out ceiling replaced.
    #[must_use]
    pub const fn with_max_fan_out(mut self, max_fan_out: usize) -> Self {
        self.max_fan_out = max_fan_out;
        self
    }

    /// The most bytes one plan body may occupy.
    #[must_use]
    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    /// The deepest nesting of steps admitted.
    #[must_use]
    pub const fn max_depth(&self) -> usize {
        self.max_depth
    }

    /// The most steps admitted, at every level together.
    #[must_use]
    pub const fn max_steps(&self) -> usize {
        self.max_steps
    }

    /// The most children of one fan-out step.
    #[must_use]
    pub const fn max_fan_out(&self) -> usize {
        self.max_fan_out
    }
}

impl Default for PlanLimits {
    /// 64 KiB bodies, 32 levels, 256 steps and a fan-out of 64.
    fn default() -> Self {
        Self {
            max_bytes: 64 * 1024,
            max_depth: 32,
            max_steps: 256,
            max_fan_out: 64,
        }
    }
}
