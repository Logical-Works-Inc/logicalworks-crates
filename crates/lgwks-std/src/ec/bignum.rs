//! Fixed-width 576-bit unsigned integer arithmetic for NIST curves (#840).

/// Fixed-width 576-bit unsigned integer (9 little-endian 64-bit limbs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct U576(pub(super) [u64; 9]);

impl U576 {
    /// Additive identity `0`.
    pub(super) const ZERO: Self = Self([0; 9]);
    /// Multiplicative identity `1`.
    pub(super) const ONE: Self = Self([1, 0, 0, 0, 0, 0, 0, 0, 0]);
    /// Constant `2`.
    pub(super) const TWO: Self = Self([2, 0, 0, 0, 0, 0, 0, 0, 0]);
    /// Constant `3`.
    pub(super) const THREE: Self = Self([3, 0, 0, 0, 0, 0, 0, 0, 0]);

    /// Parses big-endian bytes into `U576`.
    pub(super) fn from_be_bytes(bytes: &[u8]) -> Self {
        let mut limbs = [0u64; 9];
        let total = bytes.len();
        for (idx, byte_ref) in bytes.iter().rev().enumerate() {
            let limb_idx = idx >> 3;
            let byte_shift = (idx & 7).saturating_mul(8);
            if let Some(slot) = limbs.get_mut(limb_idx) {
                *slot |= u64::from(*byte_ref) << byte_shift;
            }
        }
        let _ = total;
        Self(limbs)
    }

    /// Serializes the lowest `out_len` bytes in big-endian order.
    pub(super) fn to_be_bytes(self, out_len: usize) -> Vec<u8> {
        let mut out = vec![0u8; out_len];
        for idx in 0..out_len {
            let limb_idx = idx >> 3;
            let byte_shift = (idx & 7).saturating_mul(8);
            let limb_val = match self.0.get(limb_idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let mut byte_val = 0_u8;
            if let Ok(num) = u8::try_from((limb_val >> byte_shift) & 0xff) {
                byte_val = num;
            }
            let rev_idx = out_len.saturating_sub(1).saturating_sub(idx);
            if let Some(slot) = out.get_mut(rev_idx) {
                *slot = byte_val;
            }
        }
        out
    }

    /// Constant-time check for zero (`1` if zero, `0` otherwise).
    pub(super) fn ct_is_zero(&self) -> u64 {
        let mut acc = 0u64;
        for limb_ref in &self.0 {
            acc |= *limb_ref;
        }
        let non_zero = (acc | acc.wrapping_neg()) >> 63;
        non_zero ^ 1
    }

    /// Returns `true` if `self == 0`.
    pub(super) fn is_zero(&self) -> bool {
        self.ct_is_zero() == 1
    }

    /// Constant-time equality (`1` if equal, `0` otherwise).
    pub(super) fn ct_eq(&self, rhs: &Self) -> u64 {
        let mut diff = 0u64;
        for idx in 0..9 {
            let lhs_w = match self.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let rhs_w = match rhs.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            };
            diff |= lhs_w ^ rhs_w;
        }
        let non_zero = (diff | diff.wrapping_neg()) >> 63;
        non_zero ^ 1
    }

    /// Constant-time conditional select: returns `lhs` if `choice == 0`, `rhs` if `choice == 1`.
    pub(super) fn ct_select(lhs: &Self, rhs: &Self, choice: u64) -> Self {
        let mask = 0u64.wrapping_sub(choice & 1);
        let mut out = [0u64; 9];
        for idx in 0..9 {
            let lw = match lhs.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let rw = match rhs.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            };
            if let Some(slot) = out.get_mut(idx) {
                *slot = lw ^ (mask & (lw ^ rw));
            }
        }
        Self(out)
    }

    /// Raw addition returning `(sum, carry)`.
    pub(super) fn add_raw(&self, rhs: &Self) -> (Self, u64) {
        let mut out = [0u64; 9];
        let mut carry = 0u128;
        for idx in 0..9 {
            let lw = u128::from(match self.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            });
            let rw = u128::from(match rhs.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            });
            let total = lw.wrapping_add(rw).wrapping_add(carry);
            if let Some(slot) = out.get_mut(idx) {
                let mut limb = 0_u64;
                if let Ok(num) = u64::try_from(total & 0xffff_ffff_ffff_ffff) {
                    limb = num;
                }
                *slot = limb;
            }
            carry = total >> 64;
        }
        let mut carry_u64 = 0_u64;
        if let Ok(num) = u64::try_from(carry) {
            carry_u64 = num;
        }
        (Self(out), carry_u64)
    }

    /// Raw subtraction returning `(diff, borrow)` where `borrow == 1` iff `self < rhs`.
    pub(super) fn sub_raw(&self, rhs: &Self) -> (Self, u64) {
        let mut out = [0u64; 9];
        let mut borrow = 0u128;
        for idx in 0..9 {
            let lw = u128::from(match self.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            });
            let rw = u128::from(match rhs.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            })
            .wrapping_add(borrow);
            let diff = lw.wrapping_sub(rw);
            if let Some(slot) = out.get_mut(idx) {
                let mut limb = 0_u64;
                if let Ok(num) = u64::try_from(diff & 0xffff_ffff_ffff_ffff) {
                    limb = num;
                }
                *slot = limb;
            }
            borrow = (diff >> 127) & 1;
        }
        let mut borrow_u64 = 0_u64;
        if let Ok(num) = u64::try_from(borrow) {
            borrow_u64 = num;
        }
        (Self(out), borrow_u64)
    }

    /// Returns `true` if `self >= rhs`.
    pub(super) fn ge(&self, rhs: &Self) -> bool {
        let (_, borrow) = self.sub_raw(rhs);
        borrow == 0
    }

    /// Right-shifts `self` by 1 bit.
    pub(super) fn shr1(&self) -> Self {
        let mut out = [0u64; 9];
        let mut carry = 0u64;
        for idx in (0..9).rev() {
            let word = match self.0.get(idx) {
                Some(limb) => *limb,
                None => 0,
            };
            if let Some(slot) = out.get_mut(idx) {
                *slot = (word >> 1) | (carry << 63);
            }
            carry = word & 1;
        }
        Self(out)
    }

    /// Constant-time modular addition `(self + rhs) mod modulus`.
    pub(super) fn add_mod(&self, rhs: &Self, modulus: &Self) -> Self {
        let (sum, _) = self.add_raw(rhs);
        let (reduced, borrow) = sum.sub_raw(modulus);
        Self::ct_select(&reduced, &sum, borrow)
    }

    /// Constant-time modular subtraction `(self - rhs) mod modulus`.
    pub(super) fn sub_mod(&self, rhs: &Self, modulus: &Self) -> Self {
        let (diff, borrow) = self.sub_raw(rhs);
        let (wrapped, _) = diff.add_raw(modulus);
        Self::ct_select(&diff, &wrapped, borrow)
    }

    /// Constant-time modular multiplication `(self * rhs) mod modulus` over `bits` bits.
    pub(super) fn mul_mod(&self, rhs: &Self, modulus: &Self, bits: usize) -> Self {
        let mut acc = Self::ZERO;
        let mut bit_idx = bits;
        while bit_idx > 0 {
            bit_idx = bit_idx.saturating_sub(1);
            acc = acc.add_mod(&acc, modulus);
            let limb_idx = bit_idx >> 6;
            let shift = bit_idx & 63;
            let limb_val = match rhs.0.get(limb_idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let bit = (limb_val >> shift) & 1;
            let with_lhs = acc.add_mod(self, modulus);
            acc = Self::ct_select(&acc, &with_lhs, bit);
        }
        acc
    }

    /// Constant-time modular exponentiation `self^exp mod modulus` over `bits` bits.
    pub(super) fn pow_mod(&self, exp: &Self, modulus: &Self, bits: usize) -> Self {
        let mut acc = Self::ONE;
        let mut bit_idx = bits;
        while bit_idx > 0 {
            bit_idx = bit_idx.saturating_sub(1);
            acc = acc.mul_mod(&acc, modulus, bits);
            let limb_idx = bit_idx >> 6;
            let shift = bit_idx & 63;
            let limb_val = match exp.0.get(limb_idx) {
                Some(limb) => *limb,
                None => 0,
            };
            let bit = (limb_val >> shift) & 1;
            let prod = acc.mul_mod(self, modulus, bits);
            acc = Self::ct_select(&acc, &prod, bit);
        }
        acc
    }

    /// Constant-time Fermat modular inverse `self^(modulus - 2) mod modulus`.
    pub(super) fn inv_mod(&self, modulus: &Self, bits: usize) -> Self {
        let (exp, _) = modulus.sub_raw(&Self::TWO);
        self.pow_mod(&exp, modulus, bits)
    }

    /// Reduces an arbitrary `U576` modulo `modulus`.
    pub(super) fn rem_mod(&self, modulus: &Self, bits: usize) -> Self {
        Self::ONE.mul_mod(self, modulus, bits.max(528))
    }
}
