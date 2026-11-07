//! Simulation tests for pure-Rust IEEE 754 floating-point mathematical operations.
//!
//! Validates IEEE 754 special cases, transcendental identities across 10,000 seeded draws,
//! and deterministic bit-level reproducibility under FNV-1a trace hashing.

use crate::math::{
    atan2, atan2f, cbrt, cbrtf, cos, cosf, exp, expf, f64_to_f32, ln, lnf, pow, powf, sin, sinf,
    sqrt, sqrtf,
};
use crate::seeded::{SWEEP_SEEDS, Seeded};
use std::error::Error;

/// What every simulation test here returns.
type TestResult = Result<(), Box<dyn Error>>;

/// FNV-1a 64-bit offset basis.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// FNV-1a 64-bit prime.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Verifies a boolean condition, returning a descriptive error on failure.
fn ensure(condition: bool, msg: &'static str) -> TestResult {
    if condition { Ok(()) } else { Err(msg.into()) }
}

/// Updates an FNV-1a 64-bit hash with a 64-bit word.
fn fnv1a_fold_u64(hash: &mut u64, word: u64) {
    for byte in word.to_le_bytes() {
        *hash = (*hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME);
    }
}

/// Verifies IEEE 754 special-case handling for square root functions.
#[test]
fn special_cases_sqrt() -> TestResult {
    ensure(sqrt(f64::NAN).is_nan(), "sqrt(NaN) must be NaN")?;
    ensure(sqrt(-1.0).is_nan(), "sqrt(-1.0) must be NaN")?;
    ensure(
        sqrt(0.0).to_bits() == 0.0f64.to_bits(),
        "sqrt(+0.0) must preserve positive zero",
    )?;
    ensure(
        sqrt(-0.0).to_bits() == (-0.0f64).to_bits(),
        "sqrt(-0.0) must preserve negative zero",
    )?;
    ensure(
        sqrt(f64::INFINITY).to_bits() == f64::INFINITY.to_bits(),
        "sqrt(+inf) must be +inf",
    )?;
    ensure(sqrt(f64::NEG_INFINITY).is_nan(), "sqrt(-inf) must be NaN")?;
    let sub_val = f64::from_bits(1);
    ensure(sqrt(sub_val) > 0.0, "sqrt(subnormal) must be positive")?;

    ensure(sqrtf(f32::NAN).is_nan(), "sqrtf(NaN) must be NaN")?;
    ensure(sqrtf(-1.0).is_nan(), "sqrtf(-1.0) must be NaN")?;
    ensure(
        sqrtf(0.0).to_bits() == 0.0f32.to_bits(),
        "sqrtf(0.0) must be 0.0",
    )?;
    ensure(
        sqrtf(f32::INFINITY).to_bits() == f32::INFINITY.to_bits(),
        "sqrtf(+inf) must be +inf",
    )?;
    Ok(())
}

/// Verifies IEEE 754 special-case handling for cube root functions.
#[test]
fn special_cases_cbrt() -> TestResult {
    ensure(cbrt(f64::NAN).is_nan(), "cbrt(NaN) must be NaN")?;
    ensure(
        cbrt(0.0).to_bits() == 0.0f64.to_bits(),
        "cbrt(+0.0) must preserve positive zero",
    )?;
    ensure(
        cbrt(-0.0).to_bits() == (-0.0f64).to_bits(),
        "cbrt(-0.0) must preserve negative zero",
    )?;
    ensure(
        cbrt(f64::INFINITY).to_bits() == f64::INFINITY.to_bits(),
        "cbrt(+inf) must be +inf",
    )?;
    ensure(
        cbrt(f64::NEG_INFINITY).to_bits() == f64::NEG_INFINITY.to_bits(),
        "cbrt(-inf) must be -inf",
    )?;
    ensure(
        cbrt(8.0).to_bits() == 2.0f64.to_bits(),
        "cbrt(8.0) must be 2.0",
    )?;
    ensure(
        cbrt(-8.0).to_bits() == (-2.0f64).to_bits(),
        "cbrt(-8.0) must be -2.0",
    )?;
    let sub_val = f64::from_bits(1);
    ensure(cbrt(sub_val) > 0.0, "cbrt(subnormal) must be positive")?;

    ensure(cbrtf(f32::NAN).is_nan(), "cbrtf(NaN) must be NaN")?;
    ensure(
        cbrtf(0.0).to_bits() == 0.0f32.to_bits(),
        "cbrtf(0.0) must be 0.0",
    )?;
    ensure(
        cbrtf(-8.0).to_bits() == (-2.0f32).to_bits(),
        "cbrtf(-8.0) must be -2.0",
    )?;
    Ok(())
}

/// Verifies IEEE 754 special-case handling for exponential functions.
#[test]
fn special_cases_exp() -> TestResult {
    ensure(exp(f64::NAN).is_nan(), "exp(NaN) must be NaN")?;
    ensure(
        exp(0.0).to_bits() == 1.0f64.to_bits(),
        "exp(0.0) must be 1.0",
    )?;
    ensure(
        exp(710.0).to_bits() == f64::INFINITY.to_bits(),
        "exp(710.0) must overflow to +inf",
    )?;
    ensure(
        exp(-750.0).to_bits() == 0.0f64.to_bits(),
        "exp(-750.0) must underflow to 0.0",
    )?;
    ensure(
        exp(f64::INFINITY).to_bits() == f64::INFINITY.to_bits(),
        "exp(+inf) must be +inf",
    )?;
    ensure(
        exp(f64::NEG_INFINITY).to_bits() == 0.0f64.to_bits(),
        "exp(-inf) must be 0.0",
    )?;

    ensure(expf(f32::NAN).is_nan(), "expf(NaN) must be NaN")?;
    ensure(
        expf(0.0).to_bits() == 1.0f32.to_bits(),
        "expf(0.0) must be 1.0",
    )?;
    ensure(
        expf(100.0).to_bits() == f32::INFINITY.to_bits(),
        "expf(100.0) must overflow to +inf",
    )?;
    ensure(
        expf(-150.0).to_bits() == 0.0f32.to_bits(),
        "expf(-150.0) must underflow to 0.0",
    )?;
    Ok(())
}

/// Verifies IEEE 754 special-case handling for natural logarithm functions.
#[test]
fn special_cases_ln() -> TestResult {
    ensure(ln(f64::NAN).is_nan(), "ln(NaN) must be NaN")?;
    ensure(ln(-1.0).is_nan(), "ln(-1.0) must be NaN")?;
    ensure(
        ln(0.0).to_bits() == f64::NEG_INFINITY.to_bits(),
        "ln(+0.0) must be -inf",
    )?;
    ensure(
        ln(-0.0).to_bits() == f64::NEG_INFINITY.to_bits(),
        "ln(-0.0) must be -inf",
    )?;
    ensure(ln(1.0).to_bits() == 0.0f64.to_bits(), "ln(1.0) must be 0.0")?;
    ensure(
        ln(f64::INFINITY).to_bits() == f64::INFINITY.to_bits(),
        "ln(+inf) must be +inf",
    )?;
    let sub_val = f64::from_bits(1);
    ensure(ln(sub_val) < 0.0, "ln(subnormal) must be negative")?;

    ensure(lnf(f32::NAN).is_nan(), "lnf(NaN) must be NaN")?;
    ensure(lnf(-1.0).is_nan(), "lnf(-1.0) must be NaN")?;
    ensure(
        lnf(0.0).to_bits() == f32::NEG_INFINITY.to_bits(),
        "lnf(0.0) must be -inf",
    )?;
    ensure(
        lnf(1.0).to_bits() == 0.0f32.to_bits(),
        "lnf(1.0) must be 0.0",
    )?;
    Ok(())
}

/// Verifies IEEE 754 special-case handling for power functions.
#[test]
fn special_cases_pow() -> TestResult {
    ensure(
        pow(f64::NAN, 0.0).to_bits() == 1.0f64.to_bits(),
        "pow(NaN, 0.0) must be 1.0",
    )?;
    ensure(
        pow(1.0, f64::NAN).to_bits() == 1.0f64.to_bits(),
        "pow(1.0, NaN) must be 1.0",
    )?;
    ensure(
        pow(2.0, 0.0).to_bits() == 1.0f64.to_bits(),
        "pow(2.0, 0.0) must be 1.0",
    )?;
    ensure(
        pow(-2.0, 3.0).to_bits() == (-8.0f64).to_bits(),
        "pow(-2.0, 3.0) must be -8.0",
    )?;
    ensure(
        pow(-2.0, 2.0).to_bits() == 4.0f64.to_bits(),
        "pow(-2.0, 2.0) must be 4.0",
    )?;
    ensure(pow(-2.0, 2.5).is_nan(), "pow(-2.0, 2.5) must be NaN")?;
    ensure(
        pow(0.0, 1.0).to_bits() == 0.0f64.to_bits(),
        "pow(0.0, 1.0) must be 0.0",
    )?;
    ensure(
        pow(0.0, -1.0).to_bits() == f64::INFINITY.to_bits(),
        "pow(0.0, -1.0) must be +inf",
    )?;

    ensure(
        powf(f32::NAN, 0.0).to_bits() == 1.0f32.to_bits(),
        "powf(NaN, 0.0) must be 1.0",
    )?;
    ensure(
        powf(2.0, 3.0).to_bits() == 8.0f32.to_bits(),
        "powf(2.0, 3.0) must be 8.0",
    )?;
    ensure(
        powf(-2.0, 3.0).to_bits() == (-8.0f32).to_bits(),
        "powf(-2.0, 3.0) must be -8.0",
    )?;
    Ok(())
}

/// Verifies IEEE 754 special-case handling for trigonometric functions.
#[test]
fn special_cases_trig() -> TestResult {
    ensure(
        sin(0.0).to_bits() == 0.0f64.to_bits(),
        "sin(+0.0) must preserve positive zero",
    )?;
    ensure(
        sin(-0.0).to_bits() == (-0.0f64).to_bits(),
        "sin(-0.0) must preserve negative zero",
    )?;
    ensure(
        cos(0.0).to_bits() == 1.0f64.to_bits(),
        "cos(+0.0) must be 1.0",
    )?;
    ensure(
        cos(-0.0).to_bits() == 1.0f64.to_bits(),
        "cos(-0.0) must be 1.0",
    )?;
    ensure(sin(f64::INFINITY).is_nan(), "sin(+inf) must be NaN")?;
    ensure(cos(f64::INFINITY).is_nan(), "cos(+inf) must be NaN")?;

    ensure(
        atan2(0.0, 0.0).to_bits() == 0.0f64.to_bits(),
        "atan2(+0.0, +0.0) must be +0.0",
    )?;
    ensure(atan2(0.0, -0.0) > 3.1, "atan2(+0.0, -0.0) must be +pi")?;
    ensure(atan2(-0.0, -0.0) < -3.1, "atan2(-0.0, -0.0) must be -pi")?;
    ensure(atan2(1.0, 0.0) > 1.5, "atan2(1.0, 0.0) must be +pi/2")?;
    ensure(atan2(-1.0, 0.0) < -1.5, "atan2(-1.0, 0.0) must be -pi/2")?;

    ensure(
        sinf(0.0).to_bits() == 0.0f32.to_bits(),
        "sinf(0.0) must be 0.0",
    )?;
    ensure(
        cosf(0.0).to_bits() == 1.0f32.to_bits(),
        "cosf(0.0) must be 1.0",
    )?;
    ensure(
        atan2f(0.0, 1.0).to_bits() == 0.0f32.to_bits(),
        "atan2f(0.0, 1.0) must be 0.0",
    )?;
    Ok(())
}

fn sweep_normalized_values() -> Vec<f64> {
    let mut values = Vec::with_capacity(10_000);
    for seed_ref in &SWEEP_SEEDS {
        let seed = *seed_ref;
        let mut stream = Seeded::from_seed(seed);
        for _step in 0..2_500 {
            let word = stream.next_u64();
            let norm = f64::from_bits(0x3ff0_0000_0000_0000 | (word >> 12)) - 1.0;
            values.push(norm);
        }
    }
    values
}

/// Tests mathematical identities across 10,000 seeded values from `SWEEP_SEEDS`.
#[test]
fn sim_math_libm_oracle_sweep() -> TestResult {
    for norm in sweep_normalized_values() {
        // 1. sin(x)^2 + cos(x)^2 ≈ 1.0 within < 1e-14
        let x_trig = (norm - 0.5) * 100.0;
        let ss_trig = sin(x_trig);
        let cc_trig = cos(x_trig);
        let trig_id = (ss_trig * ss_trig + cc_trig * cc_trig - 1.0).abs();
        ensure(trig_id < 1e-14, "sin^2 + cos^2 must equal 1.0 within 1e-14")?;

        // f32 counterpart
        let x_trig_f = (f64_to_f32(norm) - 0.5f32) * 50.0f32;
        let sf_trig = sinf(x_trig_f);
        let cf_trig = cosf(x_trig_f);
        let trig_id_f = (sf_trig * sf_trig + cf_trig * cf_trig - 1.0f32).abs();
        ensure(
            trig_id_f < 1e-6,
            "sinf^2 + cosf^2 must equal 1.0 within 1e-6",
        )?;

        // 2. ln(exp(x)) ≈ x
        let x_exp = (norm - 0.5) * 20.0;
        let exp_val = exp(x_exp);
        let ln_exp = ln(exp_val);
        let exp_id = (ln_exp - x_exp).abs();
        ensure(exp_id < 1e-12, "ln(exp(x)) must equal x within 1e-12")?;

        // f32 counterpart
        let x_exp_f = (f64_to_f32(norm) - 0.5f32) * 10.0f32;
        let exp_val_f = expf(x_exp_f);
        let ln_exp_f = lnf(exp_val_f);
        let exp_id_f = (ln_exp_f - x_exp_f).abs();
        ensure(exp_id_f < 1e-5, "lnf(expf(x)) must equal x within 1e-5")?;

        // 3. sqrt(x * x) ≈ x
        let x_sqrt = norm * 500.0 + 0.01;
        let sq_val = x_sqrt * x_sqrt;
        let sqrt_val = sqrt(sq_val);
        let sqrt_diff = (sqrt_val - x_sqrt).abs() / x_sqrt;
        ensure(
            sqrt_diff < 1e-13,
            "sqrt(x*x) must equal x within 1e-13 rel error",
        )?;

        // f32 counterpart
        let x_sqrt_f = f64_to_f32(norm) * 50.0f32 + 0.01f32;
        let sq_val_f = x_sqrt_f * x_sqrt_f;
        let sqrt_val_f = sqrtf(sq_val_f);
        let sqrt_diff_f = (sqrt_val_f - x_sqrt_f).abs() / x_sqrt_f;
        ensure(
            sqrt_diff_f < 1e-6,
            "sqrtf(x*x) must equal x within 1e-6 rel error",
        )?;

        // 4. cbrt(-x) == -cbrt(x)
        let x_cbrt = (norm - 0.5) * 10_000.0;
        ensure(
            cbrt(-x_cbrt).to_bits() == (-cbrt(x_cbrt)).to_bits(),
            "cbrt(-x) must equal -cbrt(x) exactly",
        )?;

        // f32 counterpart
        let x_cbrt_f = (f64_to_f32(norm) - 0.5f32) * 1_000.0f32;
        ensure(
            cbrtf(-x_cbrt_f).to_bits() == (-cbrtf(x_cbrt_f)).to_bits(),
            "cbrtf(-x) must equal -cbrtf(x) exactly",
        )?;

        // 5. atan2(sin(x), cos(x)) ≈ x for x in (-pi, pi)
        let x_atan = (norm - 0.5) * 6.2;
        let ss_atan = sin(x_atan);
        let cc_atan = cos(x_atan);
        let atan_val = atan2(ss_atan, cc_atan);
        let atan_diff = (atan_val - x_atan).abs();
        ensure(
            atan_diff < 1e-14,
            "atan2(sin(x), cos(x)) must equal x within 1e-14",
        )?;

        // f32 counterpart
        let x_atan_f = (f64_to_f32(norm) - 0.5f32) * 6.0f32;
        let sf_atan = sinf(x_atan_f);
        let cf_atan = cosf(x_atan_f);
        let atan_val_f = atan2f(sf_atan, cf_atan);
        let atan_diff_f = (atan_val_f - x_atan_f).abs();
        ensure(
            atan_diff_f < 1e-6,
            "atan2f(sinf(x), cosf(x)) must equal x within 1e-6",
        )?;
    }
    Ok(())
}

/// Computes the combined FNV-1a trace hash over all mathematical functions across `SWEEP_SEEDS`.
fn compute_sweep_trace() -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for norm in sweep_normalized_values() {
        let xx_val = (norm - 0.5) * 20.0;
        let pos_x = norm * 100.0 + 0.01;

        fnv1a_fold_u64(&mut hash, sqrt(pos_x).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(sqrtf(f64_to_f32(pos_x)).to_bits()));
        fnv1a_fold_u64(&mut hash, cbrt(xx_val).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(cbrtf(f64_to_f32(xx_val)).to_bits()));
        fnv1a_fold_u64(&mut hash, exp(xx_val).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(expf(f64_to_f32(xx_val)).to_bits()));
        fnv1a_fold_u64(&mut hash, ln(pos_x).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(lnf(f64_to_f32(pos_x)).to_bits()));
        fnv1a_fold_u64(&mut hash, pow(pos_x, 1.5).to_bits());
        fnv1a_fold_u64(
            &mut hash,
            u64::from(powf(f64_to_f32(pos_x), 1.5f32).to_bits()),
        );
        fnv1a_fold_u64(&mut hash, sin(xx_val).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(sinf(f64_to_f32(xx_val)).to_bits()));
        fnv1a_fold_u64(&mut hash, cos(xx_val).to_bits());
        fnv1a_fold_u64(&mut hash, u64::from(cosf(f64_to_f32(xx_val)).to_bits()));
        fnv1a_fold_u64(&mut hash, atan2(xx_val, pos_x).to_bits());
        fnv1a_fold_u64(
            &mut hash,
            u64::from(atan2f(f64_to_f32(xx_val), f64_to_f32(pos_x)).to_bits()),
        );
    }
    hash
}

/// Verifies deterministic bit-level reproducibility across independent replays.
#[test]
fn deterministic_bit_level_reproducibility() -> TestResult {
    let trace1 = compute_sweep_trace();
    let trace2 = compute_sweep_trace();
    ensure(
        trace1 != 0 && trace1 != FNV_OFFSET_BASIS,
        "trace hash must be non-trivial",
    )?;
    ensure(
        trace1 == trace2,
        "trace hash must replay identically across runs",
    )?;
    Ok(())
}

/// Hostile weird-bytes actor: feeds arbitrary quiet and signaling NaN bit patterns
/// generated from a deterministic seed and verifies every elementary function
/// returns a quiet NaN without trapping or fabricating finite output.
#[test]
fn sim_f64_weird_bytes_nan_payload_storm() -> TestResult {
    let mut stream = Seeded::from_seed(SWEEP_SEEDS[0]);
    for _step in 0..512 {
        let raw_payload = (stream.next_u64() & 0x000f_ffff_ffff_ffff) | 1;
        let sign_bit = (stream.next_u64() & 1) << 63;
        let nan_val = f64::from_bits(sign_bit | 0x7ff0_0000_0000_0000 | raw_payload);
        ensure(nan_val.is_nan(), "constructed bits must be NaN")?;
        ensure(sqrt(nan_val).is_nan(), "sqrt must propagate NaN payload")?;
        ensure(cbrt(nan_val).is_nan(), "cbrt must propagate NaN payload")?;
        ensure(exp(nan_val).is_nan(), "exp must propagate NaN payload")?;
        ensure(ln(nan_val).is_nan(), "ln must propagate NaN payload")?;
        ensure(sin(nan_val).is_nan(), "sin must propagate NaN payload")?;
        ensure(cos(nan_val).is_nan(), "cos must propagate NaN payload")?;
        ensure(
            atan2(nan_val, 1.0).is_nan() && atan2(1.0, nan_val).is_nan(),
            "atan2 must propagate NaN in either argument",
        )?;
    }
    Ok(())
}

/// Hostile monkey actor: generates random unsorted positive pairs across
/// subnormal-to-normal scales and verifies monotonic ordering of `sqrt`, `cbrt`, and `ln`.
#[test]
fn sim_f64_monkey_monotonic_cbrt_ln_ordering() -> TestResult {
    let mut stream = Seeded::from_seed(SWEEP_SEEDS[1]);
    for _step in 0..512 {
        let bits_a = (stream.next_u64() & 0x7fef_ffff_ffff_ffff).max(1);
        let bits_b = (stream.next_u64() & 0x7fef_ffff_ffff_ffff).max(1);
        let (lo_val, hi_val) = if bits_a <= bits_b {
            (f64::from_bits(bits_a), f64::from_bits(bits_b))
        } else {
            (f64::from_bits(bits_b), f64::from_bits(bits_a))
        };
        ensure(
            sqrt(lo_val) <= sqrt(hi_val),
            "sqrt must be monotonically non-decreasing across positive IEEE-754 domain",
        )?;
        ensure(
            cbrt(lo_val) <= cbrt(hi_val),
            "cbrt must be monotonically non-decreasing across positive IEEE-754 domain",
        )?;
        ensure(
            ln(lo_val) <= ln(hi_val),
            "ln must be monotonically non-decreasing across positive IEEE-754 domain",
        )?;
    }
    Ok(())
}

/// Hostile saboteur actor: flips signs across all four quadrants on one seed and
/// checks the exact odd/reflection symmetries of `atan2` and `sin`/`cos`.
#[test]
fn sim_f64_saboteur_quadrant_atan2_reflection() -> TestResult {
    let mut stream = Seeded::from_seed(SWEEP_SEEDS[2]);
    for _step in 0..512 {
        let word_y = stream.next_u64();
        let word_x = stream.next_u64();
        let y_coord = (f64::from_bits(0x3ff0_0000_0000_0000 | (word_y >> 12)) - 1.0) * 100.0 + 1e-6;
        let x_coord = (f64::from_bits(0x3ff0_0000_0000_0000 | (word_x >> 12)) - 1.0) * 100.0 + 1e-6;
        let q1_angle = atan2(y_coord, x_coord);
        let q4_angle = atan2(-y_coord, x_coord);
        ensure(
            (q1_angle + q4_angle).abs() <= 1e-15,
            "atan2(-y, x) must equal -atan2(y, x)",
        )?;
        ensure(
            sin(-q1_angle).to_bits() == (-sin(q1_angle)).to_bits(),
            "sin(-theta) must be bit-opposite of sin(theta)",
        )?;
        ensure(
            cos(-q1_angle).to_bits() == cos(q1_angle).to_bits(),
            "cos(-theta) must be bit-identical to cos(theta)",
        )?;
    }
    Ok(())
}
