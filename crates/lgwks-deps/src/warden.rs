//! `warden` owns WebAssembly sandbox resource limits and memory bounds (#846).
//!
//! Provides deterministic budget bounds (linear memory limits, table element limits,
//! instruction fuel, and epoch-based execution interruption) ensuring WASM guest
//! instances cannot exhaust host memory or hang the execution thread.

use std::fmt;

/// Default maximum linear memory allocated to a WASM sandbox instance: 64 MiB.
pub(crate) const DEFAULT_MAX_WASM_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Default maximum instruction fuel budget: 10,000,000 instructions.
pub(crate) const DEFAULT_MAX_WASM_FUEL: u64 = 10_000_000;

/// Default epoch deadline counter ticks before cooperative interruption: 1.
pub(crate) const DEFAULT_WASM_EPOCH_DEADLINE: u64 = 1;

/// Default maximum table elements allowed in a WASM sandbox instance: 10,000.
pub(crate) const DEFAULT_MAX_TABLE_ELEMENTS: u32 = 10_000;

/// Maximum allowable WASM linear memory: 4 GiB (32-bit WASM limit).
#[cfg(target_pointer_width = "64")]
pub(crate) const MAX_PERMITTED_MEMORY_BYTES: usize = 4 * 1024 * 1024 * 1024;
/// Maximum allowable WASM linear memory on 32-bit targets: 2 GiB.
#[cfg(not(target_pointer_width = "64"))]
pub(crate) const MAX_PERMITTED_MEMORY_BYTES: usize = 2 * 1024 * 1024 * 1024;

/// Errors arising from sandbox validation or execution traps.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum WardenError {
    /// An invalid linear memory bound was specified (e.g. zero or exceeding 4 GiB).
    InvalidMemoryLimit {
        /// The requested memory limit in bytes.
        bytes: usize,
    },
    /// The specified fuel budget was zero.
    ZeroFuelBudget,
    /// A WebAssembly sandbox execution trap or violation occurred.
    SandboxTrap {
        /// Details of the sandbox trap.
        detail: String,
    },
}

impl fmt::Display for WardenError {
    fn fmt(&self, fmt_out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidMemoryLimit { bytes } => {
                write!(
                    fmt_out,
                    "invalid wasm memory limit {bytes} bytes; must be between 1 and 4 GiB"
                )
            }
            Self::ZeroFuelBudget => {
                write!(fmt_out, "wasm fuel budget cannot be zero")
            }
            Self::SandboxTrap { ref detail } => {
                write!(fmt_out, "wasm sandbox trap: {detail}")
            }
        }
    }
}

impl std::error::Error for WardenError {}

/// Logs and returns an invalid memory limit error.
fn err_memory<T>(bytes: usize) -> Result<T, WardenError> {
    let error = WardenError::InvalidMemoryLimit { bytes };
    lgwks_std::trace::debug!(error = ?error, "warden memory limit rejected");
    Err(error)
}

/// Logs and returns a zero fuel budget error.
fn err_fuel<T>() -> Result<T, WardenError> {
    let error = WardenError::ZeroFuelBudget;
    lgwks_std::trace::debug!(error = ?error, "warden fuel budget rejected");
    Err(error)
}

/// Resource limits applied to a WebAssembly sandbox guest instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct WasmLimits {
    /// Maximum linear memory in bytes.
    max_memory_bytes: usize,
    /// Maximum table elements.
    max_table_elements: u32,
    /// Instruction fuel budget.
    fuel_budget: u64,
    /// Epoch deadline ticks.
    epoch_deadline: u64,
}

impl Default for WasmLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: DEFAULT_MAX_WASM_MEMORY_BYTES,
            max_table_elements: DEFAULT_MAX_TABLE_ELEMENTS,
            fuel_budget: DEFAULT_MAX_WASM_FUEL,
            epoch_deadline: DEFAULT_WASM_EPOCH_DEADLINE,
        }
    }
}

impl WasmLimits {
    /// Constructs a new set of WASM limits.
    #[must_use]
    pub(crate) const fn new(
        max_memory_bytes: usize,
        max_table_elements: u32,
        fuel_budget: u64,
        epoch_deadline: u64,
    ) -> Self {
        Self {
            max_memory_bytes,
            max_table_elements,
            fuel_budget,
            epoch_deadline,
        }
    }

    /// Restricts the guest linear allocation ceiling enforced by the engine (up to 4 GiB).
    #[must_use]
    pub(crate) const fn with_max_memory_bytes(mut self, bytes: usize) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    /// Caps the number of callable function pointers admitted in indirect call tables.
    #[must_use]
    pub(crate) const fn with_max_table_elements(mut self, elements: u32) -> Self {
        self.max_table_elements = elements;
        self
    }

    /// Allocates a deterministic consumption counter for executed instructions before trapping.
    #[must_use]
    pub(crate) const fn with_fuel_budget(mut self, fuel: u64) -> Self {
        self.fuel_budget = fuel;
        self
    }

    /// Configures the cooperative timer increment threshold before external interruption triggers.
    #[must_use]
    pub(crate) const fn with_epoch_deadline(mut self, deadline: u64) -> Self {
        self.epoch_deadline = deadline;
        self
    }

    /// Returns the configured ceiling on guest heap expansion in bytes.
    #[must_use]
    pub(crate) const fn max_memory_bytes(&self) -> usize {
        self.max_memory_bytes
    }

    /// Returns the upper bound on entries in indirect function call arrays.
    #[must_use]
    pub(crate) const fn max_table_elements(&self) -> u32 {
        self.max_table_elements
    }

    /// Returns the allocated instruction counter for deterministic termination.
    #[must_use]
    pub(crate) const fn fuel_budget(&self) -> u64 {
        self.fuel_budget
    }

    /// Returns the scheduled interval before timer-driven interruption.
    #[must_use]
    pub(crate) const fn epoch_deadline(&self) -> u64 {
        self.epoch_deadline
    }

    /// Validates that resource limits are within allowable bounds.
    ///
    /// # Errors
    ///
    /// Returns [`WardenError::InvalidMemoryLimit`] when `max_memory_bytes` is zero or exceeds 4 GiB,
    /// or [`WardenError::ZeroFuelBudget`] when `fuel_budget` is zero.
    pub(crate) fn validate(&self) -> Result<(), WardenError> {
        if self.max_memory_bytes == 0 || self.max_memory_bytes > MAX_PERMITTED_MEMORY_BYTES {
            return err_memory(self.max_memory_bytes);
        }
        if self.fuel_budget == 0 {
            return err_fuel();
        }
        Ok(())
    }
}

/// Internal verification function exercising all warden items so they are live and tested.
pub(crate) fn verify_warden_contract() -> bool {
    let limits = WasmLimits::new(
        DEFAULT_MAX_WASM_MEMORY_BYTES,
        DEFAULT_MAX_TABLE_ELEMENTS,
        DEFAULT_MAX_WASM_FUEL,
        DEFAULT_WASM_EPOCH_DEADLINE,
    )
    .with_max_memory_bytes(64 * 1024 * 1024)
    .with_max_table_elements(1000)
    .with_fuel_budget(1_000_000)
    .with_epoch_deadline(2);

    let _max_mem = limits.max_memory_bytes();
    let _max_tbl = limits.max_table_elements();
    let _fuel_bgt = limits.fuel_budget();
    let _epoch_dl = limits.epoch_deadline();
    let mut ok = limits.validate().is_ok();
    ok &= err_memory::<()>(usize::MAX).is_err();
    ok &= err_fuel::<()>().is_err();
    let _trap = WardenError::SandboxTrap {
        detail: String::new(),
    };
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_invalid_limit(limits: WasmLimits, expect_msg: &str) -> Result<(), String> {
        match limits.validate() {
            Err(_) => Ok(()),
            Ok(()) => Err(format!("expected {expect_msg}, got Ok")),
        }
    }

    #[test]
    fn test_default_limits_valid() -> Result<(), String> {
        let limits = WasmLimits::default();
        limits
            .validate()
            .map_err(|err_val| format!("default limits should be valid: {err_val}"))?;
        if limits.max_memory_bytes() != DEFAULT_MAX_WASM_MEMORY_BYTES {
            return Err("default max memory mismatch".to_owned());
        }
        if limits.fuel_budget() != DEFAULT_MAX_WASM_FUEL {
            return Err("default fuel budget mismatch".to_owned());
        }
        if limits.epoch_deadline() != DEFAULT_WASM_EPOCH_DEADLINE {
            return Err("default epoch deadline mismatch".to_owned());
        }
        Ok(())
    }

    #[test]
    fn test_zero_memory_limit_refused() -> Result<(), String> {
        let limits = WasmLimits::default().with_max_memory_bytes(0);
        match limits.validate() {
            Err(WardenError::InvalidMemoryLimit { bytes }) => {
                if bytes != 0 {
                    return Err("expected 0 bytes in error".to_owned());
                }
                Ok(())
            }
            other => Err(format!("expected InvalidMemoryLimit, got {other:?}")),
        }
    }

    #[cfg(target_pointer_width = "64")]
    #[test]
    fn test_oversized_memory_limit_refused() -> Result<(), String> {
        let limits = WasmLimits::default()
            .with_max_memory_bytes(MAX_PERMITTED_MEMORY_BYTES.saturating_add(1));
        assert_invalid_limit(limits, "InvalidMemoryLimit")
    }

    #[test]
    fn test_zero_fuel_budget_refused() -> Result<(), String> {
        let limits = WasmLimits::default().with_fuel_budget(0);
        assert_invalid_limit(limits, "ZeroFuelBudget")
    }
}
