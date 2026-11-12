//! The structural DER walk behind [`TlsOptions`](super::TlsOptions): one
//! certificate shape, three private-key shapes, and the public half each one
//! carries, so the parent can prove a key matches its certificate.
//!
//! Nothing here is a trust decision. Signatures are never verified, expiry is
//! the handshake's to report, and extensions walk opaquely; what the walk
//! proves is that the bytes are shaped like the certificate or key the
//! comparison needs. Every refusal goes through the parent's `refused`, so no
//! key or certificate byte is ever echoed into an error.

use super::refused;
use crate::http::Error;

/// One DER type-length-value: its tag, its body, and the whole span.
struct Tlv<'bytes> {
    /// The tag byte as read.
    tag: u8,
    /// The value bytes.
    body: &'bytes [u8],
    /// The full encoding, tag and length included, for byte comparisons.
    span: &'bytes [u8],
}

/// Read one TLV from the front of `input`, returning it and the rest.
///
/// Lengths use definite form only, at most four length bytes, minimal
/// encoding enforced: `0x81 0x05` is refused where `0x05` was required, so
/// two encodings of one value never compare differently downstream.
fn read_tlv(input: &[u8]) -> Result<(Tlv<'_>, &[u8]), Error> {
    let Some((&tag, after_tag)) = input.split_first() else {
        return refused("DER ends inside a tag");
    };
    let Some((&first_len, after_len)) = after_tag.split_first() else {
        return refused("DER ends inside a length");
    };
    let (body_len, header_len) = if first_len < 0x80 {
        (u64::from(first_len), 2_usize)
    } else {
        let count = first_len ^ 0x80;
        if count == 0 || count > 4 {
            return refused("DER length is indefinite or too wide");
        }
        let count_usize = usize::from(count);
        if after_len.len() < count_usize {
            return refused("DER ends inside a long length");
        }
        let mut length = 0_u64;
        for byte in after_len.iter().take(count_usize) {
            length = length.saturating_mul(256).saturating_add(u64::from(*byte));
        }
        let minimal: u64 = if length < 0x100 {
            1
        } else if length < 0x1_0000 {
            2
        } else if length < 0x0100_0000 {
            3
        } else {
            4
        };
        if u64::from(count) != minimal {
            return refused("DER length is not minimally encoded");
        }
        let header_len = 2_usize.saturating_add(count_usize);
        (length, header_len)
    };
    let body_len_usize = match usize::try_from(body_len) {
        Ok(fits) => fits,
        Err(int_error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(?int_error, "DER length does not fit this target");
            let _ = int_error;
            return refused("DER length does not fit this target");
        }
    };
    let total_len = match header_len.checked_add(body_len_usize) {
        Some(total) => total,
        None => return refused("DER length overflows"),
    };
    if input.len() < total_len {
        return refused("DER ends inside a value");
    }
    let (span, rest) = match input.split_at_checked(total_len) {
        Some(split) => split,
        None => return refused("DER ends inside a value"),
    };
    let body = match span.get(header_len..total_len) {
        Some(body) => body,
        None => return refused("DER header overruns its value"),
    };
    Ok((Tlv { tag, body, span }, rest))
}

/// Read exactly the children of a constructed `parent`, refusing trailing bytes.
fn read_children(body: &[u8]) -> Result<Vec<Tlv<'_>>, Error> {
    let mut children = Vec::new();
    let mut rest = body;
    while !rest.is_empty() {
        let (child, next) = read_tlv(rest)?;
        children.push(child);
        rest = next;
    }
    Ok(children)
}

/// The longest RSA modulus admitted, in magnitude bytes: 8,192 bits, the
/// ceiling ring verifies. DER prepends a zero byte when the top bit is set,
/// so a 4,096-bit modulus is 513 content bytes; the old bound of 512 content
/// bytes refused every RSA-4096 certificate and key (#913 item 3).
const RSA_MODULUS_MAX_BYTES: usize = 1_024;

/// Read one INTEGER child and return its value bytes.
///
/// `max_len` states what is being read: versions are one byte, serials at
/// most twenty, keys hundreds. The caller names the limit because only the
/// caller knows which field it asked for.
fn read_integer<'bytes>(
    child: &Tlv<'bytes>,
    what: &'static str,
    max_len: usize,
) -> Result<&'bytes [u8], Error> {
    if child.tag != 0x02 || child.body.is_empty() || child.body.len() > max_len {
        return refused(what);
    }
    Ok(child.body)
}

/// Check a UTCTime/GeneralizedTime body: digits and one trailing `Z`.
fn read_time(child: &Tlv<'_>) -> Result<(), Error> {
    let well_formed = match child.tag {
        0x17 => child.body.len() == 13,
        0x18 => child.body.len() == 15,
        _ => false,
    };
    let Some((&last, digits)) = child.body.split_last() else {
        return refused("certificate validity is not a time");
    };
    if !well_formed || last != b'Z' || !digits.iter().all(u8::is_ascii_digit) {
        return refused("certificate validity is not a time");
    }
    Ok(())
}

// ── OIDs ────────────────────────────────────────────────────────────────────

/// Raw OID bodies this loader admits, compared byte for byte.
mod oid {
    /// 1.2.840.113549.1.1.1, RSA encryption.
    pub(super) const RSA_ENCRYPTION: &[u8] =
        &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x01];
    /// 1.2.840.10045.2.1, EC public key.
    pub(super) const EC_PUBLIC_KEY: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x02, 0x01];
    /// 1.2.840.10045.3.1.7, P-256.
    pub(super) const PRIME256V1: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
    /// 1.3.132.0.34, P-384.
    pub(super) const SECP384R1: &[u8] = &[0x2B, 0x81, 0x04, 0x00, 0x22];
    /// 1.3.132.0.35, P-521.
    pub(super) const SECP521R1: &[u8] = &[0x2B, 0x81, 0x04, 0x00, 0x23];
    /// 1.2.840.113549.1.1.11, SHA-256 with RSA.
    pub(super) const SHA256_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0B];
    /// 1.2.840.113549.1.1.12, SHA-384 with RSA.
    pub(super) const SHA384_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0C];
    /// 1.2.840.113549.1.1.13, SHA-512 with RSA.
    pub(super) const SHA512_RSA: &[u8] = &[0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x01, 0x0D];
    /// 1.2.840.10045.4.3.2, ECDSA with SHA-256.
    pub(super) const ECDSA_SHA256: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x02];
    /// 1.2.840.10045.4.3.3, ECDSA with SHA-384.
    pub(super) const ECDSA_SHA384: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x03];
    /// 1.2.840.10045.4.3.4, ECDSA with SHA-512.
    pub(super) const ECDSA_SHA512: &[u8] = &[0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x04, 0x03, 0x04];
}

/// Whether `body` names an admitted signature scheme.
fn admitted_signature(body: &[u8]) -> bool {
    [
        oid::SHA256_RSA,
        oid::SHA384_RSA,
        oid::SHA512_RSA,
        oid::ECDSA_SHA256,
        oid::ECDSA_SHA384,
        oid::ECDSA_SHA512,
    ]
    .contains(&body)
}

/// Whether `body` names an admitted curve.
fn admitted_curve(body: &[u8]) -> bool {
    [oid::PRIME256V1, oid::SECP384R1, oid::SECP521R1].contains(&body)
}

// ── Certificates ────────────────────────────────────────────────────────────

/// The public half of an end-entity certificate, as encoded.
pub(super) enum CertifiedPublic {
    /// RSA modulus and exponent TLV spans, in order.
    Rsa { modulus: Vec<u8>, exponent: Vec<u8> },
    /// Curve OID body and point BIT STRING span.
    Ec { curve: Vec<u8>, point: Vec<u8> },
}

/// Walk `der` as one X.509 certificate and return its public half.
///
/// The walk is structural, not a trust decision: signatures are never
/// verified here, expiry is the handshake's to report, and extensions walk
/// opaquely. What it proves is that the bytes are shaped like the
/// certificate the key comparison needs, so a malformed file is refused
/// here rather than deeper in the stack.
pub(super) fn validate_certificate(der: &[u8]) -> Result<(), Error> {
    parse_certificate(der).map(|_| ())
}

/// Parse one certificate and return its public half.
pub(super) fn parse_certificate(der: &[u8]) -> Result<CertifiedPublic, Error> {
    let (outer, trailing) = read_tlv(der)?;
    if outer.tag != 0x30 || !trailing.is_empty() {
        return refused("certificate is not one DER sequence");
    }
    let outer_children = read_children(outer.body)?;
    if outer_children.len() != 3 {
        return refused("certificate is not three parts");
    }
    let (tbs, signature, signature_value) =
        (&outer_children[0], &outer_children[1], &outer_children[2]);
    if tbs.tag != 0x30 || signature.tag != 0x30 || signature_value.tag != 0x03 {
        return refused("certificate parts are mistagged");
    }
    let signature_oid = signature_oid_of(signature)?;
    let tbs_children = read_children(tbs.body)?;
    let mut at = 0_usize;
    if tbs_children.first().is_some_and(|child| child.tag == 0xA0) {
        let version_children = read_children(tbs_children[0].body)?;
        if version_children.len() != 1 {
            return refused("certificate version is not one integer");
        }
        let version = read_integer(&version_children[0], "certificate version is not v1..v3", 1)?;
        if version.len() != 1 || version[0] > 2 {
            return refused("certificate version is not v1..v3");
        }
        at = at.saturating_add(1);
    }
    let serial_at = at;
    let signature_at = at.saturating_add(1);
    let issuer_at = at.saturating_add(2);
    let validity_at = at.saturating_add(3);
    let subject_at = at.saturating_add(4);
    let spki_at = at.saturating_add(5);
    if tbs_children.len() < spki_at.saturating_add(1) {
        return refused("certificate tbs is too short");
    }
    read_integer(
        &tbs_children[serial_at],
        "certificate serial is not an integer",
        20,
    )?;
    let inner_signature = &tbs_children[signature_at];
    if inner_signature.tag != 0x30 || inner_signature.span != signature.span {
        return refused("certificate signatures disagree");
    }
    if signature_oid_of(inner_signature)? != signature_oid {
        return refused("certificate signatures disagree");
    }
    for name_at in [issuer_at, subject_at] {
        if tbs_children[name_at].tag != 0x30 {
            return refused("certificate issuer or subject is mistagged");
        }
    }
    let validity_children = read_children(tbs_children[validity_at].body)?;
    if tbs_children[validity_at].tag != 0x30 || validity_children.len() != 2 {
        return refused("certificate validity is not two times");
    }
    read_time(&validity_children[0])?;
    read_time(&validity_children[1])?;
    parse_public(&tbs_children[spki_at])
}

/// Read the OID out of a signature AlgorithmIdentifier, checking its shape.
fn signature_oid_of<'bytes>(identifier: &Tlv<'bytes>) -> Result<&'bytes [u8], Error> {
    let children = read_children(identifier.body)?;
    if children.is_empty() || children[0].tag != 0x06 {
        return refused("certificate signature is not an OID");
    }
    if !admitted_signature(children[0].body) {
        return refused("certificate signature scheme is not admitted");
    }
    let is_rsa = children[0].body.starts_with(&oid::SHA256_RSA[..8]);
    if is_rsa {
        if children.len() != 2 || children[1].span != [0x05, 0x00] {
            return refused("RSA signature parameters are not NULL");
        }
    } else if children.len() != 1 {
        return refused("ECDSA signature takes no parameters");
    }
    Ok(children[0].body)
}

/// Parse a SubjectPublicKeyInfo into its comparable public half.
fn parse_public(spki: &Tlv<'_>) -> Result<CertifiedPublic, Error> {
    if spki.tag != 0x30 {
        return refused("subject key is not a sequence");
    }
    let children = read_children(spki.body)?;
    if children.len() != 2 || children[0].tag != 0x30 || children[1].tag != 0x03 {
        return refused("subject key is not an algorithm and a bit string");
    }
    let algorithm = read_children(children[0].body)?;
    if algorithm.is_empty() || algorithm[0].tag != 0x06 {
        return refused("subject key algorithm is not an OID");
    }
    if algorithm[0].body == oid::RSA_ENCRYPTION {
        if algorithm.len() != 2 || algorithm[1].span != [0x05, 0x00] {
            return refused("RSA parameters are not NULL");
        }
        let key_bytes = match children[1].body.get(1..) {
            Some(key_bytes) => key_bytes,
            None => return refused("subject key bit string is empty"),
        };
        let (key_seq, key_rest) = read_tlv(key_bytes)?;
        if key_seq.tag != 0x30 || !key_rest.is_empty() {
            return refused("RSA subject key is not one sequence");
        }
        return rsa_public(&read_children(key_seq.body)?, 0)
            .map(|(modulus, exponent)| CertifiedPublic::Rsa { modulus, exponent });
    }
    if algorithm[0].body == oid::EC_PUBLIC_KEY {
        if algorithm.len() != 2 || algorithm[1].tag != 0x06 {
            return refused("EC parameters are not a curve");
        }
        if !admitted_curve(algorithm[1].body) {
            return refused("EC curve is not admitted");
        }
        let point = children[1].body;
        if point.is_empty() || point[0] != 0x00 {
            return refused("EC point is not a byte string");
        }
        return Ok(CertifiedPublic::Ec {
            curve: algorithm[1].body.to_vec(),
            point: point.to_vec(),
        });
    }
    refused("subject key algorithm is not admitted")
}

/// The RSA modulus and public exponent from `children`, starting at `at`:
/// the one reader both the certificate's `RSAPublicKey` (at 0) and the
/// key's `RSAPrivateKey` (at 1, after its version) go through, so the two
/// sides of the correspondence proof can never disagree on the bound.
fn rsa_public(children: &[Tlv<'_>], at: usize) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let (Some(modulus), Some(exponent)) = (children.get(at), children.get(at.saturating_add(1)))
    else {
        return refused("RSA key is not a modulus and an exponent");
    };
    if at == 0 && children.len() != 2 {
        return refused("RSA subject key is not two integers");
    }
    let modulus = read_integer(
        modulus,
        "RSA modulus is not an integer",
        RSA_MODULUS_MAX_BYTES.saturating_add(1),
    )?;
    let magnitude = match modulus.split_first() {
        Some((&0, rest)) => rest,
        Some(_) | None => modulus,
    };
    if magnitude.len() > RSA_MODULUS_MAX_BYTES {
        return refused("RSA modulus is longer than 8,192 bits");
    }
    let exponent = read_integer(exponent, "RSA exponent is not an integer", 16)?;
    Ok((modulus.to_vec(), exponent.to_vec()))
}

// ── Private keys ────────────────────────────────────────────────────────────

/// The public half of a private key, in the same shape as the certificate's.
pub(super) enum KeyPublic {
    /// RSA modulus and exponent value bytes, in order.
    Rsa { modulus: Vec<u8>, exponent: Vec<u8> },
    /// Curve OID body and point BIT STRING body.
    Ec { curve: Vec<u8>, point: Vec<u8> },
}

/// Parse a private key DER and return its comparable public half.
///
/// PKCS#1, SEC1 and PKCS#8 are admitted; anything else — including Ed25519,
/// whose correspondence needs curve arithmetic this loader does not own — is
/// refused with a typed error rather than handed to the TLS stack unproven.
pub(super) fn parse_private_key(der: &[u8]) -> Result<KeyPublic, Error> {
    let (outer, trailing) = read_tlv(der)?;
    if outer.tag != 0x30 || !trailing.is_empty() {
        return refused("private key is not one DER sequence");
    }
    let children = read_children(outer.body)?;
    if children.is_empty() || children[0].tag != 0x02 {
        return refused("private key has no version");
    }
    if children.len() >= 2 && children[1].tag == 0x30 {
        return parse_pkcs8(&children);
    }
    let version = read_integer(&children[0], "private key version is not an integer", 1)?;
    if version == [1] {
        return parse_sec1(&children, None);
    }
    parse_pkcs1(&children)
}

/// Parse a PKCS#1 `RSAPrivateKey`.
fn parse_pkcs1(children: &[Tlv<'_>]) -> Result<KeyPublic, Error> {
    if children.len() < 3 {
        return refused("RSA key is too short");
    }
    let version = read_integer(&children[0], "RSA version is not an integer", 1)?;
    if version != [0] {
        return refused("RSA version is not zero");
    }
    let (modulus, exponent) = rsa_public(children, 1)?;
    Ok(KeyPublic::Rsa { modulus, exponent })
}

/// Parse a SEC1 `ECPrivateKey`, with the curve from `outer_curve` when the
/// key itself omits its parameters (PKCS#8 carries the curve instead).
fn parse_sec1(children: &[Tlv<'_>], outer_curve: Option<&[u8]>) -> Result<KeyPublic, Error> {
    if children.len() < 2 {
        return refused("EC key is too short");
    }
    let version = read_integer(&children[0], "EC version is not an integer", 1)?;
    if version != [1] {
        return refused("EC version is not one");
    }
    if children[1].tag != 0x04 || children[1].body.is_empty() {
        return refused("EC key has no private value");
    }
    let mut curve = outer_curve;
    let mut point: Option<&[u8]> = None;
    for child in children.iter().skip(2) {
        if child.tag == 0xA0 {
            let parameters = read_children(child.body)?;
            if parameters.len() != 1 || parameters[0].tag != 0x06 {
                return refused("EC parameters are not a curve");
            }
            curve = Some(parameters[0].body);
        } else if child.tag == 0xA1 {
            let keys = read_children(child.body)?;
            if keys.len() != 1 || keys[0].tag != 0x03 {
                return refused("EC public key is not a bit string");
            }
            point = Some(keys[0].body);
        }
    }
    let Some(curve) = curve else {
        return refused("EC key names no curve");
    };
    if !admitted_curve(curve) {
        return refused("EC curve is not admitted");
    }
    let Some(point) = point else {
        return refused("EC key carries no public point");
    };
    if point.is_empty() || point[0] != 0x00 {
        return refused("EC point is not a byte string");
    }
    Ok(KeyPublic::Ec {
        curve: curve.to_vec(),
        point: point.to_vec(),
    })
}

/// Parse a PKCS#8 `PrivateKeyInfo`, dispatching on its algorithm.
fn parse_pkcs8(children: &[Tlv<'_>]) -> Result<KeyPublic, Error> {
    if children.len() != 3 {
        return refused("PKCS#8 key is not three parts");
    }
    let version = read_integer(&children[0], "PKCS#8 version is not an integer", 1)?;
    if version.len() != 1 || version[0] > 1 {
        return refused("PKCS#8 version is not zero or one");
    }
    if children[1].tag != 0x30 {
        return refused("PKCS#8 algorithm is not a sequence");
    }
    let algorithm = read_children(children[1].body)?;
    if algorithm.is_empty() || algorithm[0].tag != 0x06 {
        return refused("PKCS#8 algorithm is not an OID");
    }
    if children[2].tag != 0x04 || children[2].body.is_empty() {
        return refused("PKCS#8 key has no private value");
    }
    if algorithm[0].body == oid::RSA_ENCRYPTION {
        let (inner, trailing) = read_tlv(children[2].body)?;
        if inner.tag != 0x30 || !trailing.is_empty() {
            return refused("PKCS#8 RSA key is not one sequence");
        }
        return parse_pkcs1(&read_children(inner.body)?);
    }
    if algorithm[0].body == oid::EC_PUBLIC_KEY {
        let curve = if algorithm.len() > 1 {
            if algorithm[1].tag != 0x06 {
                return refused("PKCS#8 EC parameters are not a curve");
            }
            Some(algorithm[1].body)
        } else {
            None
        };
        let (inner, trailing) = read_tlv(children[2].body)?;
        if inner.tag != 0x30 || !trailing.is_empty() {
            return refused("PKCS#8 EC key is not one sequence");
        }
        return parse_sec1(&read_children(inner.body)?, curve);
    }
    refused("PKCS#8 algorithm is not admitted")
}

#[cfg(test)]
mod sim_rsa_bounds;
