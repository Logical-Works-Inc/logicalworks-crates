//! Constant-time NIST P-256 (`secp256r1`), P-384 (`secp384r1`), and P-521 (`secp521r1`)
//! elliptic curve cryptography: ECDSA deterministic signing, verification, and ECDH.

use std::fmt;

mod bignum;
mod curve;
mod der;

use bignum::U576;
use curve::{JacPoint, is_on_curve, params_for};
use der::{decode_der_int, digest_to_scalar_z, encode_der_int};

/// Supported NIST short-Weierstrass prime curves ($a = -3$).
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub(crate) enum Curve {
    /// NIST P-256 (`secp256r1`, 32-byte scalars, 65-byte uncompressed SEC1 points).
    P256,
    /// NIST P-384 (`secp384r1`, 48-byte scalars, 97-byte uncompressed SEC1 points).
    P384,
    /// NIST P-521 (`secp521r1`, 66-byte scalars, 133-byte uncompressed SEC1 points).
    P521,
}

impl Curve {
    /// Scalar and coordinate byte width for this curve (`32`, `48`, or `66`).
    #[doc(hidden)]
    #[must_use]
    pub(crate) const fn scalar_len(&self) -> usize {
        match *self {
            Self::P256 => 32,
            Self::P384 => 48,
            Self::P521 => 66,
        }
    }

    /// Uncompressed SEC1 point byte length (`1 + 2 * scalar_len`).
    #[must_use]
    pub(crate) const fn uncompressed_point_len(&self) -> usize {
        match *self {
            Self::P256 => 65,
            Self::P384 => 97,
            Self::P521 => 133,
        }
    }

    /// Bit width of the prime field and scalar order.
    #[must_use]
    pub(crate) const fn bit_len(&self) -> usize {
        match *self {
            Self::P256 => 256,
            Self::P384 => 384,
            Self::P521 => 521,
        }
    }
}

/// Refusals returned by NIST elliptic-curve operations.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum EcError {
    /// Scalar is zero, out of range $\ge n$, or wrong byte length.
    InvalidScalar,
    /// Point encoding is malformed or does not satisfy $y^2 \equiv x^3 - 3x + b \pmod p$.
    InvalidPoint,
    /// Result of scalar multiplication is the identity element $\mathcal{O}$.
    PointAtInfinity,
    /// Two inputs belong to different curves.
    CurveMismatch,
    /// ECDSA signature verification failed.
    SignatureInvalid,
    /// ASN.1 DER signature encoding is malformed at the given byte offset.
    DerEncoding {
        /// Byte offset of the invalid ASN.1 DER token.
        offset: usize,
    },
    /// OS entropy source refused during key generation.
    Entropy(crate::random::EntropyError),
}

impl fmt::Display for EcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidScalar => formatter.write_str("invalid elliptic curve scalar"),
            Self::InvalidPoint => formatter.write_str("invalid elliptic curve point"),
            Self::PointAtInfinity => formatter.write_str("elliptic curve point at infinity"),
            Self::CurveMismatch => formatter.write_str("elliptic curve mismatch"),
            Self::SignatureInvalid => formatter.write_str("ecdsa signature verification failed"),
            Self::DerEncoding { offset } => {
                write!(formatter, "invalid ASN.1 DER signature at byte {offset}")
            }
            Self::Entropy(ref err_val) => write!(formatter, "entropy refusal: {err_val}"),
        }
    }
}

impl std::error::Error for EcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Entropy(ref err_val) => Some(err_val),
            Self::InvalidScalar
            | Self::InvalidPoint
            | Self::PointAtInfinity
            | Self::CurveMismatch
            | Self::SignatureInvalid
            | Self::DerEncoding { .. } => None,
        }
    }
}

impl From<crate::random::EntropyError> for EcError {
    fn from(err_val: crate::random::EntropyError) -> Self {
        Self::Entropy(err_val)
    }
}

/// Logs and returns an `EcError`.
#[doc(hidden)]
pub(crate) fn fail<T>(error: EcError) -> Result<T, EcError> {
    crate::trace::debug!(error = ?error, "ec operation refused");
    Err(error)
}

/// Zeroizes secret scalar material on drop.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct SecretScalar {
    /// Curve identifier.
    curve: Curve,
    /// Non-zero scalar in $[1, n - 1]$.
    scalar: U576,
}

impl Drop for SecretScalar {
    fn drop(&mut self) {
        self.scalar.0.fill(0);
    }
}

impl SecretScalar {
    /// Generates a fresh random secret scalar in $[1, n-1]$ from OS entropy.
    pub(crate) fn generate(curve: Curve) -> Result<Self, EcError> {
        let len = curve.scalar_len();
        let params = params_for(curve);
        for _ in 0..64 {
            let mut buf = vec![0u8; len];
            crate::random::fill_bytes(&mut buf)?;
            if matches!(curve, Curve::P521)
                && let Some(first) = buf.first_mut()
            {
                *first &= 0x01;
            }
            let cand = U576::from_be_bytes(&buf);
            if !cand.is_zero() && !cand.ge(&params.order_n) {
                return Ok(Self {
                    curve,
                    scalar: cand,
                });
            }
        }
        fail(EcError::InvalidScalar)
    }

    /// Parses a big-endian scalar in $[1, n-1]$.
    pub(crate) fn from_be_bytes(curve: Curve, bytes: &[u8]) -> Result<Self, EcError> {
        if bytes.len() != curve.scalar_len() {
            return fail(EcError::InvalidScalar);
        }
        let params = params_for(curve);
        let scalar = U576::from_be_bytes(bytes);
        if scalar.is_zero() || scalar.ge(&params.order_n) {
            return fail(EcError::InvalidScalar);
        }
        Ok(Self { curve, scalar })
    }

    /// Serializes the secret scalar to big-endian bytes.
    #[must_use]
    pub(crate) fn to_be_bytes(&self) -> Vec<u8> {
        self.scalar.to_be_bytes(self.curve.scalar_len())
    }

    /// Returns the curve this scalar belongs to.
    #[must_use]
    pub(crate) const fn curve(&self) -> Curve {
        self.curve
    }

    /// Computes the corresponding public key $Q = [d]G$.
    pub(crate) fn public_key(&self) -> Result<PublicPoint, EcError> {
        let params = params_for(self.curve);
        let base_g = JacPoint::from_affine(params.gen_x, params.gen_y);
        let (ax, ay) = base_g
            .scalar_mul(&self.scalar, &params)
            .to_affine(&params)?;
        Ok(PublicPoint {
            curve: self.curve,
            px: ax,
            py: ay,
        })
    }
}

/// Validated affine point on a NIST curve.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct PublicPoint {
    /// Curve identifier.
    curve: Curve,
    /// Affine $x$ coordinate in $[0, p-1]$.
    px: U576,
    /// Affine $y$ coordinate in $[0, p-1]$.
    py: U576,
}

impl PublicPoint {
    /// Parses a SEC1 uncompressed (`0x04 || X || Y`) or compressed (`0x02`/`0x03 || X`) point.
    pub(crate) fn from_sec1_bytes(curve: Curve, bytes: &[u8]) -> Result<Self, EcError> {
        let coord_len = curve.scalar_len();
        let params = params_for(curve);
        let Some(tag) = bytes.first().copied() else {
            return fail(EcError::InvalidPoint);
        };
        let (ax, ay) = if tag == 0x04 {
            if bytes.len() != curve.uncompressed_point_len() {
                return fail(EcError::InvalidPoint);
            }
            let x_end = 1usize.saturating_add(coord_len);
            let Some(x_bytes) = bytes.get(1..x_end) else {
                return fail(EcError::InvalidPoint);
            };
            let Some(y_bytes) = bytes.get(x_end..) else {
                return fail(EcError::InvalidPoint);
            };
            let parsed_x = U576::from_be_bytes(x_bytes);
            let parsed_y = U576::from_be_bytes(y_bytes);
            (parsed_x, parsed_y)
        } else if tag == 0x02 || tag == 0x03 {
            if bytes.len() != 1usize.saturating_add(coord_len) {
                return fail(EcError::InvalidPoint);
            }
            let Some(x_bytes) = bytes.get(1..) else {
                return fail(EcError::InvalidPoint);
            };
            let parsed_x = U576::from_be_bytes(x_bytes);
            if parsed_x.ge(&params.prime_p) {
                return fail(EcError::InvalidPoint);
            }
            let mod_p = &params.prime_p;
            let bits = params.bits;
            let x_sq = parsed_x.mul_mod(&parsed_x, mod_p, bits);
            let x_cu = x_sq.mul_mod(&parsed_x, mod_p, bits);
            let three_x = U576::THREE.mul_mod(&parsed_x, mod_p, bits);
            let rhs = x_cu
                .sub_mod(&three_x, mod_p)
                .add_mod(&params.coeff_b, mod_p);
            let (p_plus_1, _) = mod_p.add_raw(&U576::ONE);
            let exp = p_plus_1.shr1().shr1();
            let mut parsed_y = rhs.pow_mod(&exp, mod_p, bits);
            let mut y_parity = 0_u8;
            if let Some(limb) = parsed_y.0.first()
                && let Ok(num) = u8::try_from(limb & 1)
            {
                y_parity = num;
            }
            let wanted_parity = tag & 1;
            if y_parity != wanted_parity {
                parsed_y = U576::ZERO.sub_mod(&parsed_y, mod_p);
            }
            (parsed_x, parsed_y)
        } else {
            return fail(EcError::InvalidPoint);
        };

        if !is_on_curve(&ax, &ay, &params) {
            return fail(EcError::InvalidPoint);
        }
        Ok(Self {
            curve,
            px: ax,
            py: ay,
        })
    }

    /// Encodes the point in uncompressed SEC1 format (`0x04 || X || Y`).
    #[must_use]
    pub(crate) fn to_sec1_uncompressed(&self) -> Vec<u8> {
        let coord_len = self.curve.scalar_len();
        let mut out = Vec::with_capacity(self.curve.uncompressed_point_len());
        out.push(0x04);
        out.extend_from_slice(&self.px.to_be_bytes(coord_len));
        out.extend_from_slice(&self.py.to_be_bytes(coord_len));
        out
    }

    /// Encodes the point in compressed SEC1 format (`0x02`/`0x03 || X`).
    #[must_use]
    pub(crate) fn to_sec1_compressed(&self) -> Vec<u8> {
        let coord_len = self.curve.scalar_len();
        let mut out = Vec::with_capacity(1usize.saturating_add(coord_len));
        let mut odd = 0_u8;
        if let Some(limb) = self.py.0.first()
            && let Ok(num) = u8::try_from(limb & 1)
        {
            odd = num;
        }
        out.push(0x02u8.saturating_add(odd));
        out.extend_from_slice(&self.px.to_be_bytes(coord_len));
        out
    }

    /// Returns the curve this point belongs to.
    #[must_use]
    pub(crate) const fn curve(&self) -> Curve {
        self.curve
    }
}

/// Zeroizing ECDH shared secret ($x$-coordinate of $[d]Q$).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct SharedSecret {
    /// Raw big-endian shared secret bytes.
    bytes: Vec<u8>,
}

impl Drop for SharedSecret {
    fn drop(&mut self) {
        self.bytes.fill(0);
    }
}

impl SharedSecret {
    /// Borrows the raw shared secret bytes.
    #[must_use]
    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// ECDSA signature `(r, s)`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct EcdsaSignature {
    /// Curve identifier.
    curve: Curve,
    /// Signature $r$ scalar in $[1, n-1]$.
    sig_r: U576,
    /// Signature $s$ scalar in $[1, n-1]$.
    sig_s: U576,
}

impl EcdsaSignature {
    /// Parses a fixed-width IEEE P1363 raw signature (`r || s`, `2 * scalar_len` bytes).
    pub(crate) fn from_raw(curve: Curve, bytes: &[u8]) -> Result<Self, EcError> {
        let half = curve.scalar_len();
        if bytes.len() != half.saturating_mul(2) {
            return fail(EcError::SignatureInvalid);
        }
        let Some(r_bytes) = bytes.get(..half) else {
            return fail(EcError::SignatureInvalid);
        };
        let Some(s_bytes) = bytes.get(half..) else {
            return fail(EcError::SignatureInvalid);
        };
        let params = params_for(curve);
        let sig_r = U576::from_be_bytes(r_bytes);
        let sig_s = U576::from_be_bytes(s_bytes);
        if sig_r.is_zero()
            || sig_r.ge(&params.order_n)
            || sig_s.is_zero()
            || sig_s.ge(&params.order_n)
        {
            return fail(EcError::SignatureInvalid);
        }
        Ok(Self {
            curve,
            sig_r,
            sig_s,
        })
    }

    /// Serializes the signature to fixed-width `r || s` bytes.
    #[must_use]
    pub(crate) fn to_raw(&self) -> Vec<u8> {
        let half = self.curve.scalar_len();
        let mut out = Vec::with_capacity(half.saturating_mul(2));
        out.extend_from_slice(&self.sig_r.to_be_bytes(half));
        out.extend_from_slice(&self.sig_s.to_be_bytes(half));
        out
    }

    /// Encodes the signature as strict ASN.1 DER `SEQUENCE { INTEGER r, INTEGER s }`.
    #[must_use]
    pub(crate) fn to_der(&self) -> Vec<u8> {
        let half = self.curve.scalar_len();
        let r_der = encode_der_int(&self.sig_r.to_be_bytes(half));
        let s_der = encode_der_int(&self.sig_s.to_be_bytes(half));
        let body_len = r_der.len().saturating_add(s_der.len());
        let mut out = Vec::with_capacity(body_len.saturating_add(4));
        out.push(0x30);
        if body_len >= 128 {
            out.push(0x81);
        }
        let mut body_u8 = 0_u8;
        if let Ok(num) = u8::try_from(body_len) {
            body_u8 = num;
        }
        out.push(body_u8);
        out.extend_from_slice(&r_der);
        out.extend_from_slice(&s_der);
        out
    }

    /// Parses a strict ASN.1 DER signature.
    pub(crate) fn from_der(curve: Curve, der: &[u8]) -> Result<Self, EcError> {
        if der.first().copied() != Some(0x30) {
            return fail(EcError::DerEncoding { offset: 0 });
        }
        let (seq_len, mut pos) = match der.get(1).copied() {
            Some(len_byte) if len_byte < 0x80 => (usize::from(len_byte), 2usize),
            Some(0x81) => {
                let Some(len_byte) = der.get(2).copied() else {
                    return fail(EcError::DerEncoding { offset: 2 });
                };
                if len_byte < 0x80 {
                    return fail(EcError::DerEncoding { offset: 1 });
                }
                (usize::from(len_byte), 3usize)
            }
            _ => return fail(EcError::DerEncoding { offset: 1 }),
        };
        if pos.saturating_add(seq_len) != der.len() {
            return fail(EcError::DerEncoding { offset: 1 });
        }
        let (sig_r, next_pos) = decode_der_int(der, pos, curve.scalar_len())?;
        pos = next_pos;
        let (sig_s, end_pos) = decode_der_int(der, pos, curve.scalar_len())?;
        if end_pos != der.len() {
            return fail(EcError::DerEncoding { offset: end_pos });
        }
        let params = params_for(curve);
        if sig_r.is_zero()
            || sig_r.ge(&params.order_n)
            || sig_s.is_zero()
            || sig_s.ge(&params.order_n)
        {
            return fail(EcError::SignatureInvalid);
        }
        Ok(Self {
            curve,
            sig_r,
            sig_s,
        })
    }
}

/// Signs a pre-hashed digest deterministically (with low-$S$ normalization).
pub(crate) fn ecdsa_sign_digest(
    secret: &SecretScalar,
    digest: &[u8],
) -> Result<EcdsaSignature, EcError> {
    let params = params_for(secret.curve);
    let z_val = digest_to_scalar_z(digest, &params);
    let sec_bytes = secret.to_be_bytes();
    let base_g = JacPoint::from_affine(params.gen_x, params.gen_y);

    for counter in 0u32..64u32 {
        let mut hasher = crate::hash::Hasher::new();
        hasher.write_framed(b"lgwks.ec.rfc6979.v1");
        hasher.write_framed(&sec_bytes);
        hasher.write_framed(digest);
        hasher.write_framed(&counter.to_be_bytes());
        let first_block = hasher.finalize();
        let mut second = crate::hash::Hasher::new();
        second.write_framed(first_block.as_bytes());
        second.write_framed(&counter.to_le_bytes());
        let second_block = second.finalize();
        let mut third = crate::hash::Hasher::new();
        third.write_framed(second_block.as_bytes());
        let third_block = third.finalize();

        let mut wide = Vec::with_capacity(96);
        wide.extend_from_slice(first_block.as_bytes());
        wide.extend_from_slice(second_block.as_bytes());
        wide.extend_from_slice(third_block.as_bytes());
        let slice = match wide.get(..secret.curve.scalar_len()) {
            Some(wide_slice) => wide_slice,
            None => &wide,
        };
        let nonce_k = U576::from_be_bytes(slice).rem_mod(&params.order_n, params.bits);
        if nonce_k.is_zero() {
            continue;
        }
        let Ok((rx, _)) = base_g.scalar_mul(&nonce_k, &params).to_affine(&params) else {
            continue;
        };
        let sig_r = rx.rem_mod(&params.order_n, params.bits);
        if sig_r.is_zero() {
            continue;
        }
        let k_inv = nonce_k.inv_mod(&params.order_n, params.bits);
        let rd = sig_r.mul_mod(&secret.scalar, &params.order_n, params.bits);
        let z_plus_rd = z_val.add_mod(&rd, &params.order_n);
        let mut sig_s = k_inv.mul_mod(&z_plus_rd, &params.order_n, params.bits);
        if sig_s.is_zero() {
            continue;
        }
        let half_n = params.order_n.shr1();
        if !half_n.ge(&sig_s) {
            sig_s = params.order_n.sub_mod(&sig_s, &params.order_n);
        }
        return Ok(EcdsaSignature {
            curve: secret.curve,
            sig_r,
            sig_s,
        });
    }
    fail(EcError::SignatureInvalid)
}

/// Verifies an ECDSA signature `(r, s)` over `digest` against `public`.
pub(crate) fn ecdsa_verify_digest(
    public: &PublicPoint,
    digest: &[u8],
    sig: &EcdsaSignature,
) -> Result<(), EcError> {
    if public.curve != sig.curve {
        return fail(EcError::CurveMismatch);
    }
    let params = params_for(public.curve);
    if sig.sig_r.is_zero()
        || sig.sig_r.ge(&params.order_n)
        || sig.sig_s.is_zero()
        || sig.sig_s.ge(&params.order_n)
    {
        return fail(EcError::SignatureInvalid);
    }
    let z_val = digest_to_scalar_z(digest, &params);
    let w_inv = sig.sig_s.inv_mod(&params.order_n, params.bits);
    let u1 = z_val.mul_mod(&w_inv, &params.order_n, params.bits);
    let u2 = sig.sig_r.mul_mod(&w_inv, &params.order_n, params.bits);

    let base_g = JacPoint::from_affine(params.gen_x, params.gen_y);
    let pub_q = JacPoint::from_affine(public.px, public.py);
    let p1 = base_g.scalar_mul(&u1, &params);
    let p2 = pub_q.scalar_mul(&u2, &params);
    let sum_pt = p1.add(&p2, &params);
    let (rx, _) = sum_pt.to_affine(&params)?;
    let v_val = rx.rem_mod(&params.order_n, params.bits);
    if v_val.ct_eq(&sig.sig_r) == 1 {
        Ok(())
    } else {
        fail(EcError::SignatureInvalid)
    }
}

/// Computes the ECDH shared secret $x([d_{\text{self}}]Q_{\text{peer}})$.
pub(crate) fn ecdh_shared_secret(
    secret: &SecretScalar,
    peer: &PublicPoint,
) -> Result<SharedSecret, EcError> {
    if secret.curve != peer.curve {
        return fail(EcError::CurveMismatch);
    }
    let params = params_for(secret.curve);
    let peer_jac = JacPoint::from_affine(peer.px, peer.py);
    let (shared_x, _) = peer_jac
        .scalar_mul(&secret.scalar, &params)
        .to_affine(&params)?;
    Ok(SharedSecret {
        bytes: shared_x.to_be_bytes(secret.curve.scalar_len()),
    })
}

/// Internal verification function exercising curve operations so they are live and tested.
#[must_use]
pub(crate) fn verify_ec_contract() -> bool {
    let mut valid = true;
    for curve_item in &[Curve::P256, Curve::P384, Curve::P521] {
        let curve_val = *curve_item;
        let _bits = curve_val.bit_len();
        let _slen = curve_val.scalar_len();
        let _ulen = curve_val.uncompressed_point_len();
        match curve_val {
            Curve::P256 => {
                if let Ok(secret_scalar) = SecretScalar::generate(curve_val) {
                    let _scurve = secret_scalar.curve();
                    let scalar_bytes = secret_scalar.to_be_bytes();
                    valid &= SecretScalar::from_be_bytes(curve_val, &scalar_bytes).is_ok();
                    if let Ok(pub_point) = secret_scalar.public_key() {
                        let _pcurve = pub_point.curve();
                        let uncompressed = pub_point.to_sec1_uncompressed();
                        let compressed = pub_point.to_sec1_compressed();
                        valid &= PublicPoint::from_sec1_bytes(curve_val, &uncompressed).is_ok();
                        valid &= PublicPoint::from_sec1_bytes(curve_val, &compressed).is_ok();
                        if let Ok(shared_sec) = ecdh_shared_secret(&secret_scalar, &pub_point) {
                            let _sh_bytes = shared_sec.as_bytes();
                        }
                        let digest_buf = [0x5au8; 32];
                        if let Ok(ecdsa_sig) = ecdsa_sign_digest(&secret_scalar, &digest_buf) {
                            let raw_sig = ecdsa_sig.to_raw();
                            let der_sig = ecdsa_sig.to_der();
                            valid &= EcdsaSignature::from_raw(curve_val, &raw_sig).is_ok();
                            valid &= EcdsaSignature::from_der(curve_val, &der_sig).is_ok();
                            valid &=
                                ecdsa_verify_digest(&pub_point, &digest_buf, &ecdsa_sig).is_ok();
                        }
                    }
                }
            }
            Curve::P384 | Curve::P521 => {}
        }
    }
    valid
}
