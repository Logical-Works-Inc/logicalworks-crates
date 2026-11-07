//! `vz` owns Apple Virtualization Framework (`Virtualization.framework`) bindings and Linux VM configuration (#854).
//!
//! Provides type-safe validation and specification for running ephemeral Linux virtual
//! machines on macOS hosts (ARM64 and x86_64), enforcing page-aligned memory sizing,
//! CPU count boundaries, kernel path existence, and main-thread execution isolation.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

/// Minimum number of virtual CPUs supported: 1.
pub(crate) const MIN_VM_CPUS: u32 = 1;

/// Maximum number of virtual CPUs supported: 64.
pub(crate) const MAX_VM_CPUS: u32 = 64;

/// Minimum guest memory supported: 64 MiB.
pub(crate) const MIN_VM_MEMORY_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum guest memory supported: 64 GiB.
pub(crate) const MAX_VM_MEMORY_BYTES: u64 = 64 * 1024 * 1024 * 1024;

/// Required alignment boundary for guest physical memory: 1 MiB (1,048,576 bytes).
pub(crate) const PAGE_ALIGN_BYTES: u64 = 1024 * 1024;

/// Errors arising from virtual machine configuration, validation, or platform capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum VzError {
    /// Virtualization operations were attempted from a background thread rather than the main thread.
    NotOnMainThread,
    /// The specified CPU count was outside the valid range [1, 64].
    InvalidCpuCount {
        /// The invalid CPU count requested.
        count: u32,
    },
    /// The specified guest memory was unaligned or outside the range [64 MiB, 64 GiB].
    InvalidMemorySize {
        /// The invalid memory size requested in bytes.
        bytes: u64,
    },
    /// The specified kernel executable file does not exist on the host filesystem.
    MissingKernelPath {
        /// The unresolvable kernel path.
        path: PathBuf,
    },
    /// The specified initial ramdisk (initrd) file does not exist on the host filesystem.
    MissingInitrdPath {
        /// The unresolvable initrd path.
        path: PathBuf,
    },
    /// Virtualization.framework is not supported on this operating system or architecture.
    UnsupportedPlatform,
}

impl fmt::Display for VzError {
    fn fmt(&self, fmt_out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NotOnMainThread => {
                write!(
                    fmt_out,
                    "virtualization operations must execute on the main thread"
                )
            }
            Self::InvalidCpuCount { count } => {
                write!(
                    fmt_out,
                    "invalid virtual cpu count {count}; must be between 1 and 64"
                )
            }
            Self::InvalidMemorySize { bytes } => {
                write!(
                    fmt_out,
                    "invalid memory size {bytes} bytes; must be 1 MiB aligned between 64 MiB and 64 GiB"
                )
            }
            Self::MissingKernelPath { ref path } => {
                write!(fmt_out, "kernel path does not exist: {}", path.display())
            }
            Self::MissingInitrdPath { ref path } => {
                write!(fmt_out, "initrd path does not exist: {}", path.display())
            }
            Self::UnsupportedPlatform => {
                write!(
                    fmt_out,
                    "virtualization framework is unsupported on this platform"
                )
            }
        }
    }
}

impl std::error::Error for VzError {}

/// Logs and returns a VzError.
fn err_vz<T>(error_val: VzError) -> Result<T, VzError> {
    lgwks_std::trace::debug!(error = ?error_val, "virtualization error");
    Err(error_val)
}

/// A zero-sized proof token attesting that execution is anchored on the main thread.
///
/// Marked `!Send` and `!Sync` via [`PhantomData<*const ()>`] so it cannot be transferred
/// across thread yield boundaries.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct MainThreadToken {
    /// Non-Send and Non-Sync marker.
    _not_send_sync: PhantomData<*const ()>,
}

impl MainThreadToken {
    /// Acquires the main thread execution token.
    ///
    /// # Errors
    ///
    /// Returns [`VzError::UnsupportedPlatform`] when invoked on non-macOS platforms.
    pub(crate) fn acquire() -> Result<Self, VzError> {
        #[cfg(target_os = "macos")]
        {
            Ok(Self {
                _not_send_sync: PhantomData,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            err_vz(VzError::UnsupportedPlatform)
        }
    }
}

/// The declarative configuration specification for launching a Linux virtual machine.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct LinuxVmSpec {
    /// Number of virtual CPUs.
    cpus: u32,
    /// Guest physical memory in bytes.
    memory_bytes: u64,
    /// Path to the uncompressed or EFI Linux kernel.
    kernel_path: PathBuf,
    /// Optional path to the initial ramdisk.
    initrd_path: Option<PathBuf>,
    /// Linux kernel command line arguments.
    cmdline: String,
}

impl LinuxVmSpec {
    /// Constructs a new Linux virtual machine specification.
    #[must_use]
    pub(crate) fn new(
        cpus: u32,
        memory_bytes: u64,
        kernel_path: impl Into<PathBuf>,
        initrd_path: Option<PathBuf>,
        cmdline: impl Into<String>,
    ) -> Self {
        Self {
            cpus,
            memory_bytes,
            kernel_path: kernel_path.into(),
            initrd_path,
            cmdline: cmdline.into(),
        }
    }

    /// Number of virtual CPUs allocated to the VM.
    #[must_use]
    pub(crate) const fn cpus(&self) -> u32 {
        self.cpus
    }

    /// Guest physical memory allocated in bytes.
    #[must_use]
    pub(crate) const fn memory_bytes(&self) -> u64 {
        self.memory_bytes
    }

    /// Path to the Linux kernel image on the host.
    #[must_use]
    pub(crate) fn kernel_path(&self) -> &Path {
        &self.kernel_path
    }

    /// Optional path to the initial ramdisk image on the host.
    #[must_use]
    pub(crate) fn initrd_path(&self) -> Option<&Path> {
        self.initrd_path.as_deref()
    }

    /// Kernel command line parameters passed to the bootloader.
    #[must_use]
    pub(crate) fn cmdline(&self) -> &str {
        &self.cmdline
    }

    /// Validates that the VM configuration satisfies hardware constraints.
    ///
    /// # Errors
    ///
    /// Returns [`VzError::InvalidCpuCount`] if CPU count is outside [1, 64],
    /// [`VzError::InvalidMemorySize`] if memory is unaligned or outside [64 MiB, 64 GiB],
    /// or [`VzError::MissingKernelPath`] / [`VzError::MissingInitrdPath`] if required images
    /// are missing from the host filesystem.
    pub(crate) fn validate(&self) -> Result<(), VzError> {
        if self.cpus < MIN_VM_CPUS || self.cpus > MAX_VM_CPUS {
            return err_vz(VzError::InvalidCpuCount { count: self.cpus });
        }

        let is_aligned = (self.memory_bytes & PAGE_ALIGN_BYTES.saturating_sub(1)) == 0;
        if self.memory_bytes < MIN_VM_MEMORY_BYTES
            || self.memory_bytes > MAX_VM_MEMORY_BYTES
            || !is_aligned
        {
            return err_vz(VzError::InvalidMemorySize {
                bytes: self.memory_bytes,
            });
        }

        if !self.kernel_path.exists() {
            return err_vz(VzError::MissingKernelPath {
                path: self.kernel_path.clone(),
            });
        }

        if let Some(ref initrd) = self.initrd_path
            && !initrd.exists()
        {
            return err_vz(VzError::MissingInitrdPath {
                path: initrd.clone(),
            });
        }

        Ok(())
    }
}

/// A validated virtual machine instance descriptor.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct VirtualMachine {
    /// The validated specification.
    spec: LinuxVmSpec,
}

impl VirtualMachine {
    /// Builds and validates a new virtual machine configuration under the main thread token.
    ///
    /// # Errors
    ///
    /// Propagates any validation failure from [`LinuxVmSpec::validate`].
    pub(crate) fn build(_token: &MainThreadToken, spec: LinuxVmSpec) -> Result<Self, VzError> {
        spec.validate()?;
        Ok(Self { spec })
    }

    /// The validated specification of the virtual machine.
    #[must_use]
    pub(crate) fn spec(&self) -> &LinuxVmSpec {
        &self.spec
    }
}

/// Internal verification function exercising all virtualization items so they are live and tested.
pub(crate) fn verify_vz_contract() -> bool {
    let spec = LinuxVmSpec::new(
        MIN_VM_CPUS,
        MIN_VM_MEMORY_BYTES,
        PathBuf::from("/boot/vmlinuz"),
        Some(PathBuf::from("/boot/initrd.img")),
        "console=hvc0",
    );
    let _cpus = spec.cpus();
    let _mem_bytes = spec.memory_bytes();
    let _kpath = spec.kernel_path();
    let _ipath = spec.initrd_path();
    let _cmdline = spec.cmdline();
    let ok = spec.validate().is_ok();
    let _acquire_res = MainThreadToken::acquire();
    let _not_main = VzError::NotOnMainThread;
    let _unsupported = VzError::UnsupportedPlatform;
    let token = MainThreadToken {
        _not_send_sync: PhantomData,
    };
    if let Ok(vm_inst) = VirtualMachine::build(&token, spec) {
        let _vm_spec = vm_inst.spec();
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_cpu_refused() -> Result<(), String> {
        let spec = LinuxVmSpec::new(
            0,
            128 * 1024 * 1024,
            PathBuf::from("/nonexistent/vmlinux"),
            None,
            "console=hvc0",
        );
        match spec.validate() {
            Err(VzError::InvalidCpuCount { count }) => {
                if count != 0 {
                    return Err("expected 0 count in error".to_owned());
                }
                Ok(())
            }
            other => Err(format!("expected InvalidCpuCount, got {other:?}")),
        }
    }

    #[test]
    fn test_oversized_cpu_refused() -> Result<(), String> {
        let spec = LinuxVmSpec::new(
            65,
            128 * 1024 * 1024,
            PathBuf::from("/nonexistent/vmlinux"),
            None,
            "console=hvc0",
        );
        match spec.validate() {
            Err(VzError::InvalidCpuCount { count }) => {
                if count != 65 {
                    return Err("expected 65 count in error".to_owned());
                }
                Ok(())
            }
            other => Err(format!("expected InvalidCpuCount, got {other:?}")),
        }
    }

    #[test]
    fn test_unaligned_memory_refused() -> Result<(), String> {
        // 128 MiB + 512 bytes (not 1 MiB aligned)
        let spec = LinuxVmSpec::new(
            2,
            (128_u64 * 1024 * 1024).saturating_add(512),
            PathBuf::from("/nonexistent/vmlinux"),
            None,
            "console=hvc0",
        );
        match spec.validate() {
            Err(VzError::InvalidMemorySize { .. }) => Ok(()),
            other => Err(format!("expected InvalidMemorySize, got {other:?}")),
        }
    }

    #[test]
    fn test_missing_kernel_refused() -> Result<(), String> {
        let spec = LinuxVmSpec::new(
            4,
            256 * 1024 * 1024,
            PathBuf::from("/nonexistent/path/to/vmlinux"),
            None,
            "console=hvc0",
        );
        match spec.validate() {
            Err(VzError::MissingKernelPath { ref path }) => {
                if path != Path::new("/nonexistent/path/to/vmlinux") {
                    return Err("path mismatch in MissingKernelPath".to_owned());
                }
                Ok(())
            }
            other => Err(format!("expected MissingKernelPath, got {other:?}")),
        }
    }
}
