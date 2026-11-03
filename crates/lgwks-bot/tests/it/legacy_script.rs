//! The legacy codecs reached in one line from a `script!` flow.
//!
//! Each flow below names a `lgwks_std::legacy` codec function directly, so
//! the estate's migration path is one call from orchestration: decode or
//! encode on one line, with the typed codec error mapped into the flow's
//! failure. The EBCDIC flow is the receipt the brief asks for.

#![cfg(all(feature = "script", feature = "legacy"))]

use std::error::Error;

use lgwks_bot::script::{FlowError, Scope, Tenant};

/// What every fallible test here returns.
type TestResult = Result<(), Box<dyn Error>>;

lgwks_bot::script! {
    /// Decode EBCDIC then re-encode it, one codec call per line.
    flow legacy_ebcdic_passthrough(bytes: Vec<u8>) -> Vec<u8>:
        let text = lgwks_std::legacy::ebcdic::decode(lgwks_std::legacy::ebcdic::CodePage::Cp037, &bytes, &lgwks_std::legacy::limits::Limits::default()).map_err(|error| FlowError::Failed { at: std::sync::Arc::from("legacy"), reason: format!("{error}") })?
        give back lgwks_std::legacy::ebcdic::encode(lgwks_std::legacy::ebcdic::CodePage::Cp037, &text, &lgwks_std::legacy::limits::Limits::default()).map_err(|error| FlowError::Failed { at: std::sync::Arc::from("legacy"), reason: format!("{error}") })?

    /// Decode a packed decimal then re-encode it, one codec call per line.
    flow legacy_decimal_passthrough(bytes: Vec<u8>) -> Vec<u8>:
        let value = lgwks_std::legacy::decimal::decode_packed(&bytes, 7, 2, true, &lgwks_std::legacy::limits::Limits::default()).map_err(|error| FlowError::Failed { at: std::sync::Arc::from("legacy"), reason: format!("{error}") })?
        give back lgwks_std::legacy::decimal::encode_packed(value, 7, true, &lgwks_std::legacy::limits::Limits::default()).map_err(|error| FlowError::Failed { at: std::sync::Arc::from("legacy"), reason: format!("{error}") })?
}

#[test]
fn a_script_flow_calls_the_ebcdic_codec_in_one_line() -> TestResult {
    let scope = Scope::root(Tenant::new("acme")?);
    let bytes = lgwks_std::legacy::ebcdic::encode(
        lgwks_std::legacy::ebcdic::CodePage::Cp037,
        "HELLO",
        &lgwks_std::legacy::limits::Limits::default(),
    )?;
    let back = lgwks_bot::block_on(legacy_ebcdic_passthrough(&scope, bytes.clone()))?;
    assert_eq!(
        back, bytes,
        "decode then encode through the flow must return the same bytes"
    );
    Ok(())
}

#[test]
fn a_script_flow_calls_the_decimal_codec_in_one_line() -> TestResult {
    let scope = Scope::root(Tenant::new("acme")?);
    let value = lgwks_std::legacy::decimal::Decimal::new(1_234_567, 2);
    let bytes = lgwks_std::legacy::decimal::encode_packed(
        value,
        7,
        true,
        &lgwks_std::legacy::limits::Limits::default(),
    )?;
    let back = lgwks_bot::block_on(legacy_decimal_passthrough(&scope, bytes.clone()))?;
    assert_eq!(
        back, bytes,
        "decode then encode through the flow must return the same bytes"
    );
    Ok(())
}
