//! Deterministic simulation testing for warden resource limits (#846).

use crate::warden::{MAX_PERMITTED_MEMORY_BYTES, WasmLimits};
use lgwks_std::seeded::Seeded;

const SEED: u64 = 0x0084_65a4_db07;

fn draw_u32(stream: &mut Seeded) -> u32 {
    let bytes = stream.next_u64().to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn draw_usize(stream: &mut Seeded) -> usize {
    let bytes = stream.next_u64().to_le_bytes();
    usize::from_le_bytes(bytes)
}

#[test]
fn sim_warden_hostile_actor_limits_sweep() -> Result<(), String> {
    let mut rng = Seeded::from_seed(SEED);

    for step_idx in 0..1_000 {
        // Draw memory size across full range including zero and above 4 GiB
        let raw_mem = draw_usize(&mut rng);
        let mem = raw_mem % 8_589_934_592;
        let tables = draw_u32(&mut rng);
        let fuel = rng.next_u64() % 100_000_000;
        let epoch = rng.next_u64() % 100;

        let limits = WasmLimits::default()
            .with_max_memory_bytes(mem)
            .with_max_table_elements(tables)
            .with_fuel_budget(fuel)
            .with_epoch_deadline(epoch);

        let validated = limits.validate();

        // Hostile / fool oracle:
        // Memory of 0 or > 4 GiB (MAX_PERMITTED_MEMORY_BYTES) must be refused.
        // Fuel budget of 0 must be refused.
        let is_valid = mem > 0 && mem <= MAX_PERMITTED_MEMORY_BYTES && fuel > 0;

        if is_valid {
            if validated.is_err() {
                return Err(format!(
                    "step {step_idx}: expected valid limits for mem={mem}, tables={tables}, fuel={fuel}, epoch={epoch}"
                ));
            }
        } else if validated.is_ok() {
            return Err(format!(
                "step {step_idx}: expected refusal for mem={mem}, tables={tables}, fuel={fuel}, epoch={epoch}"
            ));
        }
    }
    Ok(())
}

#[test]
fn sim_warden_saboteur_cloning_and_accessors() -> Result<(), String> {
    let mut rng = Seeded::from_seed(SEED ^ 0x0bad_f00d_dead_beef);

    for step_idx in 0..500 {
        let mem = (draw_usize(&mut rng) % 1_000_000) + 1;
        let tables = (draw_u32(&mut rng) % 50_000) + 1;
        let fuel = (rng.next_u64() % 50_000_000) + 1;
        let epoch = (rng.next_u64() % 20) + 1;

        let limits = WasmLimits::new(mem, tables, fuel, epoch);
        if limits.max_memory_bytes() != mem {
            return Err(format!(
                "step {step_idx}: max memory bytes accessor mismatch"
            ));
        }
        if limits.max_table_elements() != tables {
            return Err(format!(
                "step {step_idx}: max table elements accessor mismatch"
            ));
        }
        if limits.fuel_budget() != fuel {
            return Err(format!("step {step_idx}: fuel budget accessor mismatch"));
        }
        if limits.epoch_deadline() != epoch {
            return Err(format!("step {step_idx}: epoch deadline accessor mismatch"));
        }

        let cloned = limits;
        if cloned.max_memory_bytes() != mem
            || cloned.max_table_elements() != tables
            || cloned.fuel_budget() != fuel
            || cloned.epoch_deadline() != epoch
        {
            return Err(format!(
                "step {step_idx}: cloned limits diverged from original"
            ));
        }
    }
    Ok(())
}
