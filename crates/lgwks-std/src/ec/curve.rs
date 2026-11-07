//! Curve parameters, Jacobian points, and scalar multiplication for NIST curves (#840).

use super::Curve;
use super::EcError;
use super::bignum::U576;
use super::fail;

/// Domain parameters for a NIST prime curve.
pub(super) struct CurveParams {
    /// Prime field modulus $p$.
    pub(super) prime_p: U576,
    /// Scalar subgroup order $n$.
    pub(super) order_n: U576,
    /// Curve coefficient $b$ in $y^2 = x^3 - 3x + b$.
    pub(super) coeff_b: U576,
    /// Base point $G_x$.
    pub(super) gen_x: U576,
    /// Base point $G_y$.
    pub(super) gen_y: U576,
    /// Bit width of field elements and scalars.
    pub(super) bits: usize,
}

/// Decodes a known-valid constant hex literal into `U576`.
fn hex_u576(hex_str: &str) -> U576 {
    let mut bytes = Vec::new();
    if let Ok(raw) = crate::hex::decode(hex_str) {
        bytes = raw;
    }
    U576::from_be_bytes(&bytes)
}

/// Loads curve parameters for `curve`.
pub(super) fn params_for(curve: Curve) -> CurveParams {
    match curve {
        Curve::P256 => CurveParams {
            prime_p: hex_u576("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff"),
            order_n: hex_u576("ffffffff00000000ffffffffffffffffbce6faada7179e84f3b9cac2fc632551"),
            coeff_b: hex_u576("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b"),
            gen_x: hex_u576("6b17d1f2e12c4247f8bce6e563a440f277037d812deb33a0f4a13945d898c296"),
            gen_y: hex_u576("4fe342e2fe1a7f9b8ee7eb4a7c0f9e162bce33576b315ececbb6406837bf51f5"),
            bits: 256,
        },
        Curve::P384 => CurveParams {
            prime_p: hex_u576(
                "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
            ),
            order_n: hex_u576(
                "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
            ),
            coeff_b: hex_u576(
                "b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef",
            ),
            gen_x: hex_u576(
                "aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7",
            ),
            gen_y: hex_u576(
                "3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f",
            ),
            bits: 384,
        },
        Curve::P521 => CurveParams {
            prime_p: hex_u576(
                "01ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            ),
            order_n: hex_u576(
                "01fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffa51868783bf2f966b7fcc0148f709a5d03bb5c9b8899c47aebb6fb71e91386409",
            ),
            coeff_b: hex_u576(
                "0051953eb9618e1c9a1f929a21a0b68540eea2da725b99b315f3b8b489918ef109e156193951ec7e937b1652c0bd3bb1bf073573df883d2c34f1ef451fd46b503f00",
            ),
            gen_x: hex_u576(
                "00c6858e06b70404e9cd9e3ecb662395b4429c648139053fb521f828af606b4d3dbaa14b5e77efe75928fe1dc127a2ffa8de3348b3c1856a429bf97e7e31c2e5bd66",
            ),
            gen_y: hex_u576(
                "011839296a789a3bc0045c8a5fb42c7d1bd998f54449579b446817afbd17273e662c97ee72995ef42640c550b9013fad0761353c7086a272c24088be94769fd16650",
            ),
            bits: 521,
        },
    }
}

/// Jacobian point $(X : Y : Z)$ representing affine $(X/Z^2, Y/Z^3)$ when $Z \neq 0$.
#[derive(Clone, Copy, Debug)]
pub(super) struct JacPoint {
    /// $X$ coordinate.
    pub(super) px: U576,
    /// $Y$ coordinate.
    pub(super) py: U576,
    /// $Z$ coordinate (`0` for point at infinity).
    pub(super) pz: U576,
}

impl JacPoint {
    /// Point at infinity $\mathcal{O}$.
    pub(super) const INFINITY: Self = Self {
        px: U576::ONE,
        py: U576::ONE,
        pz: U576::ZERO,
    };

    /// Constructs a Jacobian point from affine coordinates.
    pub(super) fn from_affine(ax: U576, ay: U576) -> Self {
        Self {
            px: ax,
            py: ay,
            pz: U576::ONE,
        }
    }

    /// Constant-time conditional selection between two Jacobian points.
    pub(super) fn ct_select(lhs: &Self, rhs: &Self, choice: u64) -> Self {
        Self {
            px: U576::ct_select(&lhs.px, &rhs.px, choice),
            py: U576::ct_select(&lhs.py, &rhs.py, choice),
            pz: U576::ct_select(&lhs.pz, &rhs.pz, choice),
        }
    }

    /// Doubles a Jacobian point on $y^2 = x^3 - 3x + b$.
    pub(super) fn double(&self, params: &CurveParams) -> Self {
        let mod_p = &params.prime_p;
        let bits = params.bits;
        let z_is_zero = self.pz.ct_is_zero();

        let z_sq = self.pz.mul_mod(&self.pz, mod_p, bits);
        let x_sub_z2 = self.px.sub_mod(&z_sq, mod_p);
        let x_add_z2 = self.px.add_mod(&z_sq, mod_p);
        let diff_prod = x_sub_z2.mul_mod(&x_add_z2, mod_p, bits);
        // m = 3 * (X - Z^2) * (X + Z^2)
        let slope_m = diff_prod
            .add_mod(&diff_prod, mod_p)
            .add_mod(&diff_prod, mod_p);

        let y_sq = self.py.mul_mod(&self.py, mod_p, bits);
        let xy_sq = self.px.mul_mod(&y_sq, mod_p, bits);
        let xy_sq2 = xy_sq.add_mod(&xy_sq, mod_p);
        // s = 4 * X * Y^2
        let term_s = xy_sq2.add_mod(&xy_sq2, mod_p);

        let m_sq = slope_m.mul_mod(&slope_m, mod_p, bits);
        let two_s = term_s.add_mod(&term_s, mod_p);
        let out_x = m_sq.sub_mod(&two_s, mod_p);

        let y_fourth = y_sq.mul_mod(&y_sq, mod_p, bits);
        let y4_2 = y_fourth.add_mod(&y_fourth, mod_p);
        let y4_4 = y4_2.add_mod(&y4_2, mod_p);
        let eight_y4 = y4_4.add_mod(&y4_4, mod_p);

        let s_minus_x = term_s.sub_mod(&out_x, mod_p);
        let m_times = slope_m.mul_mod(&s_minus_x, mod_p, bits);
        let out_y = m_times.sub_mod(&eight_y4, mod_p);

        let y_z = self.py.mul_mod(&self.pz, mod_p, bits);
        let out_z = y_z.add_mod(&y_z, mod_p);

        let doubled = Self {
            px: out_x,
            py: out_y,
            pz: out_z,
        };
        Self::ct_select(&doubled, &Self::INFINITY, z_is_zero)
    }

    /// Adds two Jacobian points in constant time, handling identity and doubling cases.
    pub(super) fn add(&self, rhs: &Self, params: &CurveParams) -> Self {
        let mod_p = &params.prime_p;
        let bits = params.bits;
        let lhs_inf = self.pz.ct_is_zero();
        let rhs_inf = rhs.pz.ct_is_zero();

        let z1_sq = self.pz.mul_mod(&self.pz, mod_p, bits);
        let z2_sq = rhs.pz.mul_mod(&rhs.pz, mod_p, bits);
        let u1 = self.px.mul_mod(&z2_sq, mod_p, bits);
        let u2 = rhs.px.mul_mod(&z1_sq, mod_p, bits);

        let z1_cu = z1_sq.mul_mod(&self.pz, mod_p, bits);
        let z2_cu = z2_sq.mul_mod(&rhs.pz, mod_p, bits);
        let s1 = self.py.mul_mod(&z2_cu, mod_p, bits);
        let s2 = rhs.py.mul_mod(&z1_cu, mod_p, bits);

        let diff_h = u2.sub_mod(&u1, mod_p);
        let diff_r = s2.sub_mod(&s1, mod_p);

        let h_zero = diff_h.ct_is_zero();
        let r_zero = diff_r.ct_is_zero();

        let h_sq = diff_h.mul_mod(&diff_h, mod_p, bits);
        let h_cu = h_sq.mul_mod(&diff_h, mod_p, bits);
        let u1_h2 = u1.mul_mod(&h_sq, mod_p, bits);

        let r_sq = diff_r.mul_mod(&diff_r, mod_p, bits);
        let two_u1_h2 = u1_h2.add_mod(&u1_h2, mod_p);
        let out_x = r_sq.sub_mod(&h_cu, mod_p).sub_mod(&two_u1_h2, mod_p);

        let u1_h2_sub_x = u1_h2.sub_mod(&out_x, mod_p);
        let r_times = diff_r.mul_mod(&u1_h2_sub_x, mod_p, bits);
        let s1_h3 = s1.mul_mod(&h_cu, mod_p, bits);
        let out_y = r_times.sub_mod(&s1_h3, mod_p);

        let z1_z2 = self.pz.mul_mod(&rhs.pz, mod_p, bits);
        let out_z = z1_z2.mul_mod(&diff_h, mod_p, bits);

        let generic = Self {
            px: out_x,
            py: out_y,
            pz: out_z,
        };
        let doubled = self.double(params);
        let same_x_res = Self::ct_select(&Self::INFINITY, &doubled, r_zero);
        let non_inf_res = Self::ct_select(&generic, &same_x_res, h_zero);
        let after_rhs = Self::ct_select(&non_inf_res, self, rhs_inf);
        Self::ct_select(&after_rhs, rhs, lhs_inf)
    }

    /// Constant-time scalar multiplication $[k]P$.
    pub(super) fn scalar_mul(&self, scalar: &U576, params: &CurveParams) -> Self {
        let mut acc = Self::INFINITY;
        let mut bit_idx = params.bits;
        while bit_idx > 0 {
            bit_idx = bit_idx.saturating_sub(1);
            acc = acc.double(params);
            let limb_idx = bit_idx >> 6;
            let shift = bit_idx & 63;
            let limb_val = match scalar.0.get(limb_idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let bit = (limb_val >> shift) & 1;
            let added = acc.add(self, params);
            acc = Self::ct_select(&acc, &added, bit);
        }
        acc
    }

    /// Converts Jacobian coordinates to affine `(x, y)`, or refuses if at infinity.
    pub(super) fn to_affine(self, params: &CurveParams) -> Result<(U576, U576), EcError> {
        if self.pz.is_zero() {
            return fail(EcError::PointAtInfinity);
        }
        let mod_p = &params.prime_p;
        let bits = params.bits;
        let z_inv = self.pz.inv_mod(mod_p, bits);
        let z_inv2 = z_inv.mul_mod(&z_inv, mod_p, bits);
        let z_inv3 = z_inv2.mul_mod(&z_inv, mod_p, bits);
        let ax = self.px.mul_mod(&z_inv2, mod_p, bits);
        let ay = self.py.mul_mod(&z_inv3, mod_p, bits);
        Ok((ax, ay))
    }
}

/// Checks whether affine `(ax, ay)` lies on the curve $y^2 \equiv x^3 - 3x + b \pmod p$.
pub(super) fn is_on_curve(ax: &U576, ay: &U576, params: &CurveParams) -> bool {
    let mod_p = &params.prime_p;
    if ax.ge(mod_p) || ay.ge(mod_p) {
        return false;
    }
    let bits = params.bits;
    let lhs = ay.mul_mod(ay, mod_p, bits);
    let x_sq = ax.mul_mod(ax, mod_p, bits);
    let x_cu = x_sq.mul_mod(ax, mod_p, bits);
    let three_x = U576::THREE.mul_mod(ax, mod_p, bits);
    let rhs = x_cu
        .sub_mod(&three_x, mod_p)
        .add_mod(&params.coeff_b, mod_p);
    lhs.ct_eq(&rhs) == 1
}
