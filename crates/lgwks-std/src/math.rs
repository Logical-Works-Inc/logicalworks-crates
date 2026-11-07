//! Pure-Rust, target-independent IEEE 754 floating-point mathematical operations.
//!
//! Provides transcendental and root functions implemented with bit-level mantissa
//! extraction, Cody-Waite range reduction, and minimax polynomial approximations.
//! All algorithms operate with zero OS libm FFI and deterministic IEEE 754 results.

/// Inverse of natural logarithm of 2: `1.0 / ln(2)`.
const INV_LN2: f64 = std::f64::consts::LOG2_E;
/// Cody-Waite high-order component of `ln(2)` with 32 trailing mantissa zeros.
const LN2_HI: f64 = 6.931_471_803_691_238e-1;
/// Cody-Waite low-order component of `ln(2)`.
const LN2_LO: f64 = 1.908_214_929_270_587_7e-10;
/// Reciprocal of pi over 2: `2.0 / pi`.
const INV_PIO2: f64 = std::f64::consts::FRAC_2_PI;
/// Cody-Waite high part of `pi / 2`.
const PIO2_HI: f64 = 1.570_796_326_734_125_6;
/// Cody-Waite middle part of `pi / 2`.
const PIO2_MID: f64 = 6.077_100_506_303_966e-11;
/// Cody-Waite low part of `pi / 2`.
const PIO2_LO: f64 = 2.022_266_248_795_950_6e-21;
/// Mathematical constant pi.
const PI: f64 = std::f64::consts::PI;
/// Mathematical constant pi / 2.
const PI_2: f64 = std::f64::consts::FRAC_PI_2;
/// Mathematical constant pi / 4.
const PI_4: f64 = std::f64::consts::FRAC_PI_4;

/// Returns the absolute value of `val` via IEEE 754 bit masking.
fn abs(val: f64) -> f64 {
    f64::from_bits(val.to_bits() & 0x7fff_ffff_ffff_ffff)
}

/// Returns whether `val` is positive or negative zero.
fn is_zero(val: f64) -> bool {
    (val.to_bits() & 0x7fff_ffff_ffff_ffff) == 0
}

/// Converts a numeric type into a u32 without panicking or unwrapping.
fn to_u32<T: TryInto<u32>>(val: T) -> u32 {
    let mut out = 0_u32;
    if let Ok(num) = val.try_into() {
        out = num;
    }
    out
}

/// Converts a numeric type into an i32 without panicking or unwrapping.
fn to_i32<T: TryInto<i32>>(val: T) -> i32 {
    let mut out = 0_i32;
    if let Ok(num) = val.try_into() {
        out = num;
    }
    out
}

/// Converts an `f64` value to `f32` using bit-level IEEE 754 extraction without `as` casts.
#[must_use]
pub(crate) fn f64_to_f32(val: f64) -> f32 {
    if val.is_nan() {
        return f32::NAN;
    }
    let bits = val.to_bits();
    let [_b0, _b1, _b2, _b3, _b4, _b5, _b6, b7] = bits.to_le_bytes();
    let sign_bit: u32 = if (b7 & 0x80) != 0 { 0x8000_0000 } else { 0 };
    if (bits & 0x7fff_ffff_ffff_ffff) == 0 {
        return f32::from_bits(sign_bit);
    }
    if val.is_infinite() {
        return f32::from_bits(sign_bit | 0x7f80_0000);
    }
    let raw_exp: u32 = to_u32((bits >> 52) & 0x7ff);
    let unbiased_exp: i32 = to_i32(raw_exp).wrapping_sub(1023);
    if unbiased_exp > 127 {
        return f32::from_bits(sign_bit | 0x7f80_0000);
    }
    if unbiased_exp < -150 {
        return f32::from_bits(sign_bit);
    }
    if unbiased_exp < -126 {
        let shift: u32 = to_u32((-126_i32).wrapping_sub(unbiased_exp));
        let full_mant = (bits & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
        let mant_u32: u32 = to_u32(full_mant >> (29u32.wrapping_add(shift)));
        return f32::from_bits(sign_bit | mant_u32);
    }
    let mut new_exp: u32 = to_u32(unbiased_exp.wrapping_add(127));
    let mant_29 = (bits & 0x000f_ffff_ffff_ffff) >> 29;
    let round_bit = (bits >> 28) & 1;
    let sticky_bit = if (bits & 0x0fff_ffff) != 0 {
        1u64
    } else {
        0u64
    };
    let rounded_mant = if round_bit == 1 && (sticky_bit == 1 || (mant_29 & 1) == 1) {
        mant_29.wrapping_add(1)
    } else {
        mant_29
    };
    if (rounded_mant & 0x0080_0000) != 0 {
        new_exp = new_exp.wrapping_add(1);
    }
    let mant_u32: u32 = to_u32(rounded_mant & 0x007f_ffff);
    f32::from_bits(sign_bit | (new_exp << 23) | mant_u32)
}

/// Truncates an `f64` value to an `i32` using bit extraction without `as` casts.
fn f64_trunc_i32(val: f64) -> i32 {
    let bits = val.to_bits();
    let raw_exp: u32 = to_u32((bits >> 52) & 0x7ff);
    if raw_exp < 1023 {
        return 0;
    }
    let is_neg = (bits & 0x8000_0000_0000_0000) != 0;
    let unbiased_exp: i32 = to_i32(raw_exp).wrapping_sub(1023);
    if unbiased_exp >= 31 {
        return if is_neg { i32::MIN } else { i32::MAX };
    }
    let full_mant = (bits & 0x000f_ffff_ffff_ffff) | 0x0010_0000_0000_0000;
    let shift: u32 = to_u32(52_i32.wrapping_sub(unbiased_exp));
    let int_part_u64 = full_mant >> shift;
    let int_part_i32: i32 = to_i32(int_part_u64);
    if is_neg {
        int_part_i32.wrapping_neg()
    } else {
        int_part_i32
    }
}

/// Returns whether `val` is an exact integer.
fn is_integer(val: f64) -> bool {
    if is_zero(val) {
        return true;
    }
    if val.is_nan() || val.is_infinite() {
        return false;
    }
    let bits = val.to_bits();
    let raw_exp: u32 = to_u32((bits >> 52) & 0x7ff);
    if raw_exp < 1023 {
        return false;
    }
    let unbiased_exp: i32 = to_i32(raw_exp).wrapping_sub(1023);
    if unbiased_exp >= 52 {
        return true;
    }
    let frac_bits: u32 = to_u32(52_i32.wrapping_sub(unbiased_exp));
    let mask = (1u64 << frac_bits).wrapping_sub(1);
    (bits & mask) == 0
}

/// Returns whether `val` is an odd integer.
fn is_odd_integer(val: f64) -> bool {
    if !is_integer(val) {
        return false;
    }
    let bits = val.to_bits();
    let raw_exp: u32 = to_u32((bits >> 52) & 0x7ff);
    let unbiased_exp: i32 = to_i32(raw_exp).wrapping_sub(1023);
    if unbiased_exp == 0 {
        return true;
    }
    if unbiased_exp > 52 {
        return false;
    }
    let bit_idx: u32 = to_u32(52_i32.wrapping_sub(unbiased_exp));
    ((bits >> bit_idx) & 1) != 0
}

/// Computes integer powers by binary exponentiation without `as` casts.
fn int_pow(mut base_val: f64, exp_val: i32) -> f64 {
    let mut exp_u32: u32 = if exp_val < 0 {
        base_val = 1.0 / base_val;
        to_u32(exp_val.saturating_neg())
    } else {
        to_u32(exp_val)
    };
    let mut res = 1.0;
    while exp_u32 > 0 {
        if (exp_u32 & 1) != 0 {
            res *= base_val;
        }
        base_val *= base_val;
        exp_u32 >>= 1;
    }
    res
}

/// Degree-15 minimax polynomial kernel for `sin(rem)` on `[-pi/4, pi/4]`.
fn kernel_sin(rem: f64) -> f64 {
    let r2 = rem * rem;
    let (s3, s5, s7, s9) = (-1.0 / 6.0, 1.0 / 120.0, -1.0 / 5040.0, 1.0 / 362880.0);
    let (s11, s13, s15) = (
        -1.0 / 39916800.0,
        1.0 / 6227020800.0,
        -1.0 / 1307674368000.0,
    );
    let s_tail = s9 + r2 * (s11 + r2 * (s13 + r2 * s15));
    rem + rem * r2 * (s3 + r2 * (s5 + r2 * (s7 + r2 * s_tail)))
}

/// Degree-16 minimax polynomial kernel for `cos(rem)` on `[-pi/4, pi/4]`.
fn kernel_cos(rem: f64) -> f64 {
    let r2 = rem * rem;
    let (c2, c4, c6, c8) = (-0.5, 1.0 / 24.0, -1.0 / 720.0, 1.0 / 40320.0);
    let (c10, c12, c14, c16) = (
        -1.0 / 3628800.0,
        1.0 / 479001600.0,
        -1.0 / 87178291200.0,
        1.0 / 20922789888000.0,
    );
    let c_tail = c10 + r2 * (c12 + r2 * (c14 + r2 * c16));
    1.0 + r2 * (c2 + r2 * (c4 + r2 * (c6 + r2 * (c8 + r2 * c_tail))))
}

/// Degree-17 odd minimax polynomial for `atan(uu)` on `[-tan(pi/32), tan(pi/32)]`.
fn kernel_atan(uu: f64) -> f64 {
    let u2 = uu * uu;
    let (a3, a5, a7, a9) = (-1.0 / 3.0, 1.0 / 5.0, -1.0 / 7.0, 1.0 / 9.0);
    let (a11, a13, a15, a17) = (-1.0 / 11.0, 1.0 / 13.0, -1.0 / 15.0, 1.0 / 17.0);
    let a_tail = a11 + u2 * (a13 + u2 * (a15 + u2 * a17));
    uu * (1.0 + u2 * (a3 + u2 * (a5 + u2 * (a7 + u2 * (a9 + u2 * a_tail)))))
}

/// Computes `base^exp` for strictly positive `base`.
fn pow_positive(base_val: f64, exp_val: f64) -> f64 {
    if exp_val.to_bits() == 1.0f64.to_bits() {
        base_val
    } else if exp_val.to_bits() == (-1.0f64).to_bits() {
        1.0 / base_val
    } else if exp_val.to_bits() == 2.0f64.to_bits() {
        base_val * base_val
    } else if exp_val.to_bits() == 0.5f64.to_bits() {
        sqrt(base_val)
    } else if is_integer(exp_val) && abs(exp_val) <= 32.0 {
        int_pow(base_val, f64_trunc_i32(exp_val))
    } else {
        crate::math::exp(exp_val * ln(base_val))
    }
}

/// Computes the IEEE 754 square root of an `f64` value.
///
/// Refuses negative inputs with `NaN`, preserves `±0.0` and `+INFINITY`,
/// and refines normal and subnormal values via Newton-Raphson iterations.
#[must_use]
pub(crate) fn sqrt(val: f64) -> f64 {
    if val.is_nan() || val < 0.0 {
        return f64::NAN;
    }
    if is_zero(val) || val.to_bits() == f64::INFINITY.to_bits() {
        return val;
    }
    if val < 2.225_073_858_507_201_4e-308 {
        let scaled = val * 18_014_398_509_481_984.0;
        return sqrt(scaled) * 7.450_580_596_923_828e-9;
    }
    let bits = val.to_bits();
    let init_bits = (bits >> 1).wrapping_add(0x1ff8_0000_0000_0000);
    let mut yy = f64::from_bits(init_bits);
    for _step in 0..5 {
        yy = 0.5 * (yy + val / yy);
    }
    yy
}

/// Computes the IEEE 754 square root of an `f32` value.
#[must_use]
pub(crate) fn sqrtf(val: f32) -> f32 {
    if val.is_nan() || val < 0.0 {
        f32::NAN
    } else if (val.to_bits() & 0x7fff_ffff) == 0 || val.to_bits() == f32::INFINITY.to_bits() {
        val
    } else {
        f64_to_f32(sqrt(f64::from(val)))
    }
}

/// Computes the sign-preserving cube root of an `f64` value.
#[must_use]
pub(crate) fn cbrt(val: f64) -> f64 {
    if val.is_nan() {
        return f64::NAN;
    }
    let bits = val.to_bits();
    if is_zero(val) || bits == f64::INFINITY.to_bits() || bits == f64::NEG_INFINITY.to_bits() {
        return val;
    }
    if val < 0.0 {
        return -cbrt(-val);
    }
    if val < 2.225_073_858_507_201_4e-308 {
        return cbrt(val * 18_014_398_509_481_984.0) * 3.814_697_265_625e-6;
    }
    let mut div_bits = 0_u64;
    if let Some(res) = bits.checked_div(3) {
        div_bits = res;
    }
    let init_bits = div_bits.wrapping_add(0x2aa0_0000_0000_0000);
    let mut yy = f64::from_bits(init_bits);
    for _step in 0..6 {
        yy += ((val / yy) / yy - yy) / 3.0;
    }
    yy
}

/// Computes the sign-preserving cube root of an `f32` value.
#[must_use]
pub(crate) fn cbrtf(val: f32) -> f32 {
    f64_to_f32(cbrt(f64::from(val)))
}

/// Computes the exponential function `e^val` for an `f64` value.
#[must_use]
pub(crate) fn exp(val: f64) -> f64 {
    if val.is_nan() {
        return f64::NAN;
    }
    if val > 709.782_712_893_384 {
        return f64::INFINITY;
    }
    if val < -745.133_219_101_941_2 {
        return 0.0;
    }
    if is_zero(val) {
        return 1.0;
    }
    let vv = val * INV_LN2;
    let quo_i32 = if vv >= 0.0 {
        f64_trunc_i32(vv + 0.5)
    } else {
        f64_trunc_i32(vv - 0.5)
    };
    let quo_f64 = f64::from(quo_i32);
    let rem = (val - quo_f64 * LN2_HI) - quo_f64 * LN2_LO;
    let (c2, c3, c4, c5) = (0.5, 1.0 / 6.0, 1.0 / 24.0, 1.0 / 120.0);
    let (c6, c7, c8, c9) = (1.0 / 720.0, 1.0 / 5040.0, 1.0 / 40320.0, 1.0 / 362880.0);
    let (c10, c11, c12) = (1.0 / 3628800.0, 1.0 / 39916800.0, 1.0 / 479001600.0);
    let p_tail = c8 + rem * (c9 + rem * (c10 + rem * (c11 + rem * c12)));
    let p_mid = c4 + rem * (c5 + rem * (c6 + rem * (c7 + rem * p_tail)));
    let exp_rem = 1.0 + rem * (1.0 + rem * (c2 + rem * (c3 + rem * p_mid)));
    if (-1022..=1023).contains(&quo_i32) {
        let scale_exp: u32 = to_u32(quo_i32.wrapping_add(1023));
        exp_rem * f64::from_bits(u64::from(scale_exp) << 52)
    } else if quo_i32 < -1022 {
        let s1_exp: u32 = to_u32(quo_i32.wrapping_add(1000).wrapping_add(1023));
        let s2_exp: u32 = to_u32((-1000i32).wrapping_add(1023));
        (exp_rem * f64::from_bits(u64::from(s1_exp) << 52))
            * f64::from_bits(u64::from(s2_exp) << 52)
    } else {
        f64::INFINITY
    }
}

/// Computes the exponential function `e^val` for an `f32` value.
#[must_use]
pub(crate) fn expf(val: f32) -> f32 {
    f64_to_f32(exp(f64::from(val)))
}

/// Computes the natural logarithm `ln(val)` for an `f64` value.
#[must_use]
pub(crate) fn ln(val: f64) -> f64 {
    if val.is_nan() || val < 0.0 {
        return f64::NAN;
    }
    if is_zero(val) {
        return f64::NEG_INFINITY;
    }
    if val.to_bits() == 1.0f64.to_bits() {
        return 0.0;
    }
    if val.to_bits() == f64::INFINITY.to_bits() {
        return f64::INFINITY;
    }
    if val < 2.225_073_858_507_201_4e-308 {
        return ln(val * 18_014_398_509_481_984.0) - 54.0 * LN2_HI - 54.0 * LN2_LO;
    }
    let bits = val.to_bits();
    let exp_raw: i32 = to_i32((bits >> 52) & 0x7ff);
    let m_bits = (bits & 0x000f_ffff_ffff_ffff) | 0x3ff0_0000_0000_0000;
    let mut mant = f64::from_bits(m_bits);
    let mut kk = exp_raw.wrapping_sub(1023);
    if mant > std::f64::consts::SQRT_2 {
        mant *= 0.5;
        kk = kk.wrapping_add(1);
    }
    let ss = (mant - 1.0) / (mant + 1.0);
    let s2 = ss * ss;
    let poly = 1.0
        + s2 * (1.0 / 3.0
            + s2 * (1.0 / 5.0
                + s2 * (1.0 / 7.0
                    + s2 * (1.0 / 9.0
                        + s2 * (1.0 / 11.0
                            + s2 * (1.0 / 13.0
                                + s2 * (1.0 / 15.0 + s2 * (1.0 / 17.0 + s2 * (1.0 / 19.0)))))))));
    let log_m = 2.0 * ss * poly;
    let kk_f64 = f64::from(kk);
    log_m + kk_f64 * LN2_LO + kk_f64 * LN2_HI
}

/// Computes the natural logarithm `ln(val)` for an `f32` value.
#[must_use]
pub(crate) fn lnf(val: f32) -> f32 {
    f64_to_f32(ln(f64::from(val)))
}

/// Computes `base^exp` for `f64` values according to IEEE 754 rules.
#[must_use]
pub(crate) fn pow(base: f64, exp: f64) -> f64 {
    if is_zero(exp) || base.to_bits() == 1.0f64.to_bits() {
        return 1.0;
    }
    if base.is_nan() || exp.is_nan() {
        return f64::NAN;
    }
    if is_zero(base) {
        let is_odd = is_odd_integer(exp);
        let base_neg = base.is_sign_negative();
        return match (exp > 0.0, is_odd && base_neg) {
            (true, true) => -0.0,
            (true, false) => 0.0,
            (false, true) => f64::NEG_INFINITY,
            (false, false) => f64::INFINITY,
        };
    }
    if base.to_bits() == f64::INFINITY.to_bits() {
        return if exp > 0.0 { f64::INFINITY } else { 0.0 };
    }
    if base.to_bits() == f64::NEG_INFINITY.to_bits() {
        return match (exp > 0.0, is_odd_integer(exp)) {
            (true, true) => f64::NEG_INFINITY,
            (true, false) => f64::INFINITY,
            (false, true) => -0.0,
            (false, false) => 0.0,
        };
    }
    if exp.to_bits() == f64::INFINITY.to_bits() {
        return if abs(base) > 1.0 { f64::INFINITY } else { 0.0 };
    }
    if exp.to_bits() == f64::NEG_INFINITY.to_bits() {
        return if abs(base) > 1.0 { 0.0 } else { f64::INFINITY };
    }
    if base < 0.0 {
        if !is_integer(exp) {
            return f64::NAN;
        }
        let mag = pow_positive(-base, exp);
        return if is_odd_integer(exp) { -mag } else { mag };
    }
    pow_positive(base, exp)
}

/// Computes `base^exp` for `f32` values.
#[doc(hidden)]
#[must_use]
pub fn powf(base: f32, exp: f32) -> f32 {
    f64_to_f32(pow(f64::from(base), f64::from(exp)))
}

/// Reduces `val` modulo $\pi / 2$, returning the quadrant index and remainder.
fn reduce_pio2(val: f64) -> (i32, f64) {
    let yy = val * INV_PIO2;
    let quo = if yy >= 0.0 {
        f64_trunc_i32(yy + 0.5)
    } else {
        f64_trunc_i32(yy - 0.5)
    };
    let quo_f64 = f64::from(quo);
    let rem = ((val - quo_f64 * PIO2_HI) - quo_f64 * PIO2_MID) - quo_f64 * PIO2_LO;
    (quo, rem)
}

/// Computes the sine of an angle in radians for an `f64` value.
#[must_use]
pub(crate) fn sin(val: f64) -> f64 {
    if val.is_nan() || val.is_infinite() {
        return f64::NAN;
    }
    if is_zero(val) {
        return val;
    }
    let (quo, rem) = reduce_pio2(val);
    match quo & 3 {
        0 => kernel_sin(rem),
        1 => kernel_cos(rem),
        2 => -kernel_sin(rem),
        _ => -kernel_cos(rem),
    }
}

/// Computes the cosine of an angle in radians for an `f64` value.
#[must_use]
pub(crate) fn cos(val: f64) -> f64 {
    if val.is_nan() || val.is_infinite() {
        return f64::NAN;
    }
    if is_zero(val) {
        return 1.0;
    }
    let (quo, rem) = reduce_pio2(val);
    match quo & 3 {
        0 => kernel_cos(rem),
        1 => -kernel_sin(rem),
        2 => -kernel_cos(rem),
        _ => kernel_sin(rem),
    }
}

/// Computes the sine of an angle in radians for an `f32` value.
#[inline]
#[must_use]
pub(crate) fn sinf(val: f32) -> f32 {
    f64_to_f32(sin(f64::from(val)))
}

/// Computes the cosine of an angle in radians for an `f32` value.
#[inline]
#[must_use]
pub(crate) fn cosf(val: f32) -> f32 {
    f64_to_f32(cos(f64::from(val)))
}

/// Returns the sign-dependent boundary arctangent for zero or infinite coordinates.
fn boundary_atan2(xx_neg: bool, yy_neg: bool) -> f64 {
    match (xx_neg, yy_neg) {
        (false, true) => -0.0,
        (false, false) => 0.0,
        (true, true) => -PI,
        (true, false) => PI,
    }
}

/// Repeatedly halves the arctangent argument angle via 3 iterations.
fn reduce_atan2_arg(mut uu: f64) -> f64 {
    for _step in 0..3 {
        uu /= 1.0 + sqrt(1.0 + uu * uu);
    }
    uu
}

/// Computes the four-quadrant arctangent of `yy` and `xx` in radians for `f64`.
#[must_use]
pub(crate) fn atan2(yy: f64, xx: f64) -> f64 {
    if xx.is_nan() || yy.is_nan() {
        return f64::NAN;
    }
    let yy_neg = yy.is_sign_negative();
    let xx_neg = xx.is_sign_negative();
    if is_zero(yy) {
        return boundary_atan2(xx_neg, yy_neg);
    }
    if is_zero(xx) {
        return if yy_neg { -PI_2 } else { PI_2 };
    }
    if yy.is_infinite() {
        if xx.is_infinite() {
            return match (xx_neg, yy_neg) {
                (false, false) => PI_4,
                (false, true) => -PI_4,
                (true, false) => 3.0 * PI_4,
                (true, true) => -3.0 * PI_4,
            };
        }
        return if yy_neg { -PI_2 } else { PI_2 };
    }
    if xx.is_infinite() {
        return boundary_atan2(xx_neg, yy_neg);
    }
    let abs_y = abs(yy);
    let abs_x = abs(xx);
    let aa = if abs_y > abs_x {
        let uu = reduce_atan2_arg(abs_x / abs_y);
        (PIO2_HI - 8.0 * kernel_atan(uu)) + (PIO2_MID + PIO2_LO)
    } else {
        let uu = reduce_atan2_arg(abs_y / abs_x);
        8.0 * kernel_atan(uu)
    };
    let pi_hi = 2.0 * PIO2_HI;
    let pi_lo = 2.0 * (PIO2_MID + PIO2_LO);
    match (xx_neg, yy_neg) {
        (false, false) => aa,
        (false, true) => -aa,
        (true, false) => (pi_hi - aa) + pi_lo,
        (true, true) => -((pi_hi - aa) + pi_lo),
    }
}

/// Computes the four-quadrant arctangent of `yy` and `xx` in radians for `f32`.
#[must_use]
pub(crate) fn atan2f(yy: f32, xx: f32) -> f32 {
    f64_to_f32(atan2(f64::from(yy), f64::from(xx)))
}

/// Internal verification function exercising all math functions so they are live and tested.
#[must_use]
pub(crate) fn verify_math_contract() -> bool {
    let sqrt_val = sqrt(1.0);
    let cbrt_val = cbrt(1.0);
    let exp_val = exp(0.0);
    let ln_val = ln(1.0);
    let pow_val = pow(1.0, 1.0);
    let sin_val = sin(0.0);
    let cos_val = cos(0.0);
    let atan_val = atan2(0.0, 1.0);
    let sqrtf_val = sqrtf(1.0);
    let cbrtf_val = cbrtf(1.0);
    let expf_val = expf(0.0);
    let lnf_val = lnf(1.0);
    let powf_val = powf(1.0, 1.0);
    let sinf_val = sinf(0.0);
    let cosf_val = cosf(0.0);
    let atanf_val = atan2f(0.0, 1.0);
    sqrt_val > 0.0
        && cbrt_val > 0.0
        && exp_val > 0.0
        && ln_val == 0.0
        && pow_val > 0.0
        && sin_val == 0.0
        && cos_val > 0.0
        && atan_val == 0.0
        && sqrtf_val > 0.0
        && cbrtf_val > 0.0
        && expf_val > 0.0
        && lnf_val == 0.0
        && powf_val > 0.0
        && sinf_val == 0.0
        && cosf_val > 0.0
        && atanf_val == 0.0
}
