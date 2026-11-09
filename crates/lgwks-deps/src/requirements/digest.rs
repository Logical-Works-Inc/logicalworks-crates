//! The lock's digest: SHA-256 (FIPS 180-4), written here so the check adds no
//! dependency to the gate's first lane, and cut to twelve hex digits.

use super::{CheckError, fail};

/// SHA-256 round constants (FIPS 180-4, section 4.2.2).
const ROUND: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// SHA-256 initial hash value (FIPS 180-4, section 5.3.3).
const INITIAL: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// Mixes one 64-byte block into the running state.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    let mut schedule = [0_u32; 64];
    let (words, _) = block.as_slice().as_chunks::<4>();
    for (slot, word) in schedule.iter_mut().zip(words) {
        *slot = u32::from_be_bytes(*word);
    }
    for round in 16..64_usize {
        let early = schedule[round.saturating_sub(15)];
        let late = schedule[round.saturating_sub(2)];
        let low = early.rotate_right(7) ^ early.rotate_right(18) ^ early.wrapping_shr(3);
        let high = late.rotate_right(17) ^ late.rotate_right(19) ^ late.wrapping_shr(10);
        schedule[round] = schedule[round.saturating_sub(16)]
            .wrapping_add(low)
            .wrapping_add(schedule[round.saturating_sub(7)])
            .wrapping_add(high);
    }
    let [
        mut aa,
        mut bb,
        mut cc,
        mut dd,
        mut ee,
        mut ff,
        mut gg,
        mut hh,
    ] = *state;
    for (constant, word) in ROUND.iter().zip(schedule.iter()) {
        let sum_e = ee.rotate_right(6) ^ ee.rotate_right(11) ^ ee.rotate_right(25);
        let choose = (ee & ff) ^ (!ee & gg);
        let first = hh
            .wrapping_add(sum_e)
            .wrapping_add(choose)
            .wrapping_add(*constant)
            .wrapping_add(*word);
        let sum_a = aa.rotate_right(2) ^ aa.rotate_right(13) ^ aa.rotate_right(22);
        let majority = (aa & bb) ^ (aa & cc) ^ (bb & cc);
        let second = sum_a.wrapping_add(majority);
        hh = gg;
        gg = ff;
        ff = ee;
        ee = dd.wrapping_add(first);
        dd = cc;
        cc = bb;
        bb = aa;
        aa = first.wrapping_add(second);
    }
    for (slot, added) in state.iter_mut().zip([aa, bb, cc, dd, ee, ff, gg, hh]) {
        *slot = slot.wrapping_add(added);
    }
}

/// The SHA-256 of `data`, as eight state words.
fn sha256(data: &[u8]) -> Result<[u32; 8], CheckError> {
    let length = match u64::try_from(data.len()) {
        Ok(length) => length,
        Err(_) => return fail(CheckError::TooLong, "sha256"),
    };
    let Some(bits) = length.checked_mul(8) else {
        return fail(CheckError::TooLong, "sha256");
    };
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() & 63 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    let mut state = INITIAL;
    let (blocks, _) = message.as_chunks::<64>();
    for block in blocks {
        compress(&mut state, block);
    }
    Ok(state)
}

/// The first twelve hex digits of `sentence`'s SHA-256: the lock's digest.
pub(super) fn digest_of(sentence: &str) -> Result<String, CheckError> {
    let state = sha256(sentence.as_bytes())?;
    Ok(format!("{:08x}{:04x}", state[0], state[1].wrapping_shr(16)))
}
