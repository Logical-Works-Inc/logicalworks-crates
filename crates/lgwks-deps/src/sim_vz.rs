//! Deterministic simulation testing for Apple Virtualization configuration (#854).

use crate::vz::{
    LinuxVmSpec, MAX_VM_CPUS, MAX_VM_MEMORY_BYTES, MIN_VM_CPUS, MIN_VM_MEMORY_BYTES,
    PAGE_ALIGN_BYTES, VzError,
};
use lgwks_std::seeded::Seeded;
use std::fs;
use std::path::PathBuf;

const SEED: u64 = 0x0085_4077_1ca1;

fn draw_u32(stream: &mut Seeded) -> u32 {
    let bytes = stream.next_u64().to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

#[test]
fn sim_vz_hostile_actor_config_sweep() -> Result<(), String> {
    let mut rng = Seeded::from_seed(SEED);

    // Create a real temp directory with dummy files for kernel and initrd tests
    let temp_dir = std::env::temp_dir().join(format!("sim_vz_{}", rng.next_u64()));
    fs::create_dir_all(&temp_dir).map_err(|err| format!("failed to create temp dir: {err}"))?;
    let dummy_kernel = temp_dir.join("vmlinuz");
    let dummy_initrd = temp_dir.join("initrd.img");
    fs::write(&dummy_kernel, b"ELF_TEST_KERNEL")
        .map_err(|err| format!("failed to write dummy kernel: {err}"))?;
    fs::write(&dummy_initrd, b"INITRD_TEST_DATA")
        .map_err(|err| format!("failed to write dummy initrd: {err}"))?;

    for step_idx in 0..1_000 {
        let cpus = draw_u32(&mut rng) % 128;
        // Test memory sizes spanning unaligned, under-min, over-max, and aligned valid ranges
        let mem_choice = rng.next_u64() % 4;
        let mem = match mem_choice {
            0 => rng.next_u64() % MIN_VM_MEMORY_BYTES, // under min
            1 => {
                // aligned in [MIN, MAX]
                let mib = 64 + (rng.next_u64() % (64 * 1024 - 64));
                mib * PAGE_ALIGN_BYTES
            }
            2 => {
                // unaligned in [MIN, MAX]
                let base = 64 * PAGE_ALIGN_BYTES;
                base + (rng.next_u64() % (PAGE_ALIGN_BYTES - 1)) + 1
            }
            _ => MAX_VM_MEMORY_BYTES + (rng.next_u64() % 10_000_000_000), // over max
        };

        let use_real_kernel = (rng.next_u64() & 1) != 0;
        let kernel_path = if use_real_kernel {
            dummy_kernel.clone()
        } else {
            PathBuf::from(format!("/nonexistent_kernel_{}", rng.next_u64()))
        };

        let use_initrd = (rng.next_u64() & 1) != 0;
        let initrd_path = if use_initrd {
            let use_real_initrd = (rng.next_u64() & 1) != 0;
            if use_real_initrd {
                Some(dummy_initrd.clone())
            } else {
                Some(PathBuf::from(format!(
                    "/nonexistent_initrd_{}",
                    rng.next_u64()
                )))
            }
        } else {
            None
        };

        let spec = LinuxVmSpec::new(cpus, mem, &kernel_path, initrd_path.clone(), "console=hvc0");

        let validated = spec.validate();

        // Hostile / fool oracle:
        let cpu_valid = (MIN_VM_CPUS..=MAX_VM_CPUS).contains(&cpus);
        let mem_aligned = (mem & (PAGE_ALIGN_BYTES - 1)) == 0;
        let mem_valid = (MIN_VM_MEMORY_BYTES..=MAX_VM_MEMORY_BYTES).contains(&mem) && mem_aligned;
        let kernel_valid = use_real_kernel;
        let initrd_valid = match initrd_path {
            Some(ref target_path) => target_path == &dummy_initrd,
            None => true,
        };

        let all_valid = cpu_valid && mem_valid && kernel_valid && initrd_valid;

        if all_valid {
            if let Err(err_val) = validated {
                drop(fs::remove_dir_all(&temp_dir));
                return Err(format!(
                    "step {step_idx}: expected valid spec for cpus={cpus}, mem={mem}, err={err_val}"
                ));
            }
        } else {
            match validated {
                Ok(()) => {
                    drop(fs::remove_dir_all(&temp_dir));
                    return Err(format!(
                        "step {step_idx}: expected refusal for cpus={cpus}, mem={mem}, use_real_kernel={use_real_kernel}"
                    ));
                }
                Err(VzError::InvalidCpuCount { count }) => {
                    if cpu_valid || count != cpus {
                        drop(fs::remove_dir_all(&temp_dir));
                        return Err(format!(
                            "step {step_idx}: unexpected InvalidCpuCount: count={count}"
                        ));
                    }
                }
                Err(VzError::InvalidMemorySize { bytes }) => {
                    if !cpu_valid || mem_valid || bytes != mem {
                        drop(fs::remove_dir_all(&temp_dir));
                        return Err(format!(
                            "step {step_idx}: unexpected InvalidMemorySize: bytes={bytes}"
                        ));
                    }
                }
                Err(VzError::MissingKernelPath { .. }) => {
                    if !cpu_valid || !mem_valid || kernel_valid {
                        drop(fs::remove_dir_all(&temp_dir));
                        return Err(format!("step {step_idx}: unexpected MissingKernelPath"));
                    }
                }
                Err(VzError::MissingInitrdPath { .. }) => {
                    if !cpu_valid || !mem_valid || !kernel_valid || initrd_valid {
                        drop(fs::remove_dir_all(&temp_dir));
                        return Err(format!("step {step_idx}: unexpected MissingInitrdPath"));
                    }
                }
                Err(other) => {
                    drop(fs::remove_dir_all(&temp_dir));
                    return Err(format!(
                        "step {step_idx}: unexpected error variant: {other:?}"
                    ));
                }
            }
        }
    }

    drop(fs::remove_dir_all(&temp_dir));
    Ok(())
}

#[test]
fn sim_vz_saboteur_accessors_and_cloning() -> Result<(), String> {
    let mut rng = Seeded::from_seed(SEED ^ 0x050c_ca11_dead_beef);

    for step_idx in 0..500 {
        let cpus = (draw_u32(&mut rng) % 64) + 1;
        let mem = (64 + (rng.next_u64() % 1024)) * PAGE_ALIGN_BYTES;
        let kernel = PathBuf::from(format!("/boot/vmlinuz-{}", rng.next_u64()));
        let has_initrd = (rng.next_u64() & 1) != 0;
        let initrd = if has_initrd {
            Some(PathBuf::from(format!("/boot/initrd-{}", rng.next_u64())))
        } else {
            None
        };
        let cmdline = format!("console=hvc0 test_id={}", rng.next_u64());

        let spec = LinuxVmSpec::new(cpus, mem, &kernel, initrd.clone(), &cmdline);

        if spec.cpus() != cpus {
            return Err(format!("step {step_idx}: cpu count mismatch"));
        }
        if spec.memory_bytes() != mem {
            return Err(format!("step {step_idx}: memory bytes mismatch"));
        }
        if spec.kernel_path() != kernel.as_path() {
            return Err(format!("step {step_idx}: kernel path mismatch"));
        }
        if spec.initrd_path() != initrd.as_deref() {
            return Err(format!("step {step_idx}: initrd path mismatch"));
        }
        if spec.cmdline() != cmdline {
            return Err(format!("step {step_idx}: cmdline mismatch"));
        }

        let cloned = spec.clone();
        if cloned.cpus() != cpus
            || cloned.memory_bytes() != mem
            || cloned.kernel_path() != kernel.as_path()
            || cloned.initrd_path() != initrd.as_deref()
            || cloned.cmdline() != cmdline
        {
            return Err(format!(
                "step {step_idx}: cloned spec diverged from original"
            ));
        }
    }
    Ok(())
}
