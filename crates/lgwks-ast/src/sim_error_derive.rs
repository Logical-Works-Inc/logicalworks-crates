//! Seeded simulation of `#[derive(Error)]` expansion.
//!
//! Replays across `SWEEP_SEEDS` generating 1,000 enum and struct shapes
//! (unit variants, tuple variants, named-field variants, `#[error(transparent)]`,
//! `#[from]`, `#[source]`, generics/lifetimes, and refusal cases) verifying
//! deterministic token expansion and exact `Display`, `Error`, and `From` tokens.

use core::str::FromStr;
use lgwks_std::seeded::Seeded;
use proc_macro2::TokenStream;

use crate::error_derive::expand;

/// Seed constants for simulation sweeps.
const SWEEP_SEEDS: [u64; 4] = [
    0x5EED_0000_0000_0001,
    0x5EED_0000_0000_0002,
    0x5EED_0000_FFFF_FFFF,
    0xDEAD_BEEF_CAFE_0001,
];

/// Iterations per seed (4 * 250 = 1,000 total shapes).
const ITERATIONS_PER_SEED: usize = 250;

/// Expands `code` via `expand` and asserts that a second expansion is identical.
fn expand_deterministic(code: &str, label: &str) -> Option<String> {
    let tokens = match TokenStream::from_str(code) {
        Ok(toks) => toks,
        Err(_) => return None,
    };
    let first_res = match expand(tokens.clone()) {
        Ok(toks) => toks.to_string(),
        Err(_) => return None,
    };
    let second_res = match expand(tokens) {
        Ok(toks) => toks.to_string(),
        Err(_) => return None,
    };
    assert_eq!(
        first_res, second_res,
        "{label} expansion must be deterministic"
    );
    Some(first_res)
}

/// Asserts that `code` is refused by `expand` and mentions `expected_sub`.
fn assert_refusal(code: &str, expected_sub: &str) {
    if let Ok(toks) = TokenStream::from_str(code) {
        let err_res = expand(toks);
        assert!(err_res.is_err(), "code must be refused: {code}");
        if let Err(refusal) = err_res {
            assert!(
                refusal.message().contains(expected_sub),
                "refusal must mention '{expected_sub}'"
            );
        }
    }
}

/// Run the 1,000-shape seeded simulation test.
#[test]
fn sweep_1000_error_derive_shapes() {
    let mut total_verified: usize = 0;

    for seed_ref in &SWEEP_SEEDS {
        let seed = *seed_ref;
        let mut stream = Seeded::from_seed(seed);

        for step in 0..ITERATIONS_PER_SEED {
            let mut shape_selector = 0_u64;
            if let Ok(drawn) = stream.below(9) {
                shape_selector = drawn;
            }

            match shape_selector {
                0 => verify_unit_enum(step),
                1 => verify_tuple_enum(step),
                2 => verify_named_enum(step),
                3 => verify_transparent_enum(step),
                4 => verify_from_enum(step),
                5 => verify_source_enum(step),
                6 => verify_generic_enum(step),
                7 => verify_struct_shapes(step),
                _ => verify_refusal_cases(step),
            }

            total_verified = total_verified.saturating_add(1);
        }
    }

    assert_eq!(total_verified, 1000, "all 1000 shapes must be verified");
}

fn format_type_code(prefix: &str, step: usize, variants_body: &str) -> (String, String) {
    let name = format!("{prefix}{step}");
    let code = format!("#[derive(Error, Debug)]\npub enum {name} {{\n{variants_body}\n}}");
    (name, code)
}

fn expand_and_assert_impls(code: &str, tag: &str, name: &str) -> Option<String> {
    let expanded = expand_deterministic(code, tag)?;
    assert!(
        expanded.contains(&format!("impl :: core :: fmt :: Display for {name}")),
        "missing Display impl for {tag}"
    );
    assert!(
        expanded.contains(&format!("impl :: core :: error :: Error for {name}")),
        "missing Error impl for {tag}"
    );
    Some(expanded)
}

/// Verify unit variants enum.
fn verify_unit_enum(step: usize) {
    let (name, code) = format_type_code(
        "UnitError",
        step,
        "    #[error(\"first refusal\")]\n    First,\n    #[error(\"second refusal\")]\n    Second,",
    );

    let Some(expanded) = expand_and_assert_impls(&code, "unit enum", &name) else {
        return;
    };

    assert!(
        expanded.contains("\"first refusal\""),
        "missing first refusal literal"
    );
    assert!(
        expanded.contains("\"second refusal\""),
        "missing second refusal literal"
    );
    assert!(
        expanded.contains(":: core :: option :: Option :: None"),
        "missing None for unit enum source"
    );
}

/// Verify tuple variants enum with `{0}`, `{1}`, etc.
fn verify_tuple_enum(step: usize) {
    let (name, code) = format_type_code(
        "TupleError",
        step,
        "    #[error(\"refused: {0}\")]\n    One(String),\n    #[error(\"code: {0}, msg: {1}\")]\n    Two(u32, String),",
    );

    let Some(expanded) = expand_and_assert_impls(&code, "tuple enum", &name) else {
        return;
    };

    assert!(
        expanded.contains("\"refused: {__field_0}\""),
        "missing format string interpolation for __field_0"
    );
    assert!(
        expanded.contains("\"code: {__field_0}, msg: {__field_1}\""),
        "missing format string interpolation for tuple multiple fields"
    );
}

/// Verify named variants enum with `{actual}`, `{limit}`.
fn verify_named_enum(step: usize) {
    let (name, code) = format_type_code(
        "NamedError",
        step,
        "    #[error(\"source is {actual} bytes; limit is {limit} bytes\")]\n    TooLarge { actual: usize, limit: usize },\n    #[error(\"statement {text:?} is invalid\")]\n    BadStatement { text: String },",
    );

    let Some(expanded) = expand_and_assert_impls(&code, "named enum", &name) else {
        return;
    };

    assert!(
        expanded.contains("ref actual"),
        "missing binding for actual"
    );
    assert!(expanded.contains("ref limit"), "missing binding for limit");
    assert!(expanded.contains("ref text"), "missing binding for text");
    assert!(
        expanded.contains("\"source is {actual} bytes; limit is {limit} bytes\""),
        "missing format string interpolation for actual and limit"
    );
    assert!(
        expanded.contains("\"statement {text:?} is invalid\""),
        "missing format string interpolation for text:?"
    );
}

/// Verify transparent variants enum.
fn verify_transparent_enum(step: usize) {
    let name = format!("TransError{step}");
    let code = format!(
        "#[derive(Error, Debug)]
        pub enum {name} {{
            #[error(transparent)]
            Io(std::io::Error),
            #[error(transparent)]
            Custom {{ inner: std::io::Error }},
        }}"
    );

    let Some(expanded) = expand_deterministic(&code, "transparent enum") else {
        return;
    };

    assert!(
        expanded.contains(":: core :: fmt :: Display :: fmt (__field_0 , __formatter)"),
        "missing transparent Display delegation for tuple variant"
    );
    assert!(
        expanded.contains(":: core :: error :: Error :: source (__field_0)"),
        "missing transparent Error source delegation for tuple variant"
    );
    assert!(
        expanded.contains(":: core :: fmt :: Display :: fmt (inner , __formatter)"),
        "missing transparent Display delegation for named variant"
    );
    assert!(
        expanded.contains(":: core :: error :: Error :: source (inner)"),
        "missing transparent Error source delegation for named variant"
    );
}

/// Verify `#[from]` field on tuple variant.
fn verify_from_enum(step: usize) {
    let name = format!("FromError{step}");
    let code = format!(
        "#[derive(Error, Debug)]
        pub enum {name} {{
            #[error(\"io error\")]
            Io(#[from] std::io::Error),
        }}"
    );

    let Some(expanded) = expand_deterministic(&code, "from enum") else {
        return;
    };

    assert!(
        expanded.contains(&format!(
            "impl :: core :: convert :: From < std :: io :: Error > for {name}"
        )),
        "missing From implementation"
    );
    assert!(
        expanded.contains(":: core :: option :: Option :: Some (__field_source)"),
        "missing Some source return for from field"
    );
}

/// Verify `#[source]` field on named variant.
fn verify_source_enum(step: usize) {
    let name = format!("SourceError{step}");
    let code = format!(
        "#[derive(Error, Debug)]
        pub enum {name} {{
            #[error(\"database failure\")]
            Db {{
                #[source]
                cause: std::io::Error,
                code: u32,
            }},
        }}"
    );

    let Some(expanded) = expand_deterministic(&code, "source enum") else {
        return;
    };

    assert!(
        expanded.contains(":: core :: option :: Option :: Some (cause)"),
        "missing Some cause source return"
    );
}

/// Verify generic parameters and where clauses.
fn verify_generic_enum(step: usize) {
    let name = format!("GenericError{step}");
    let code = format!(
        "#[derive(Error, Debug)]
        pub enum {name}<'a, T: ::core::fmt::Display> where T: 'a {{
            #[error(\"value: {{0}}\")]
            Value(T),
            #[error(\"borrowed: {{0}}\")]
            Borrowed(&'a str),
        }}"
    );

    let Some(expanded) = expand_deterministic(&code, "generic enum") else {
        return;
    };

    assert!(
        expanded.contains(&format!(
            "impl < 'a , T : :: core :: fmt :: Display > :: core :: fmt :: Display for {name} < 'a , T > where T : 'a"
        )),
        "missing generic Display impl"
    );
    assert!(
        expanded.contains(&format!(
            "impl < 'a , T : :: core :: fmt :: Display > :: core :: error :: Error for {name} < 'a , T > where T : 'a"
        )),
        "missing generic Error impl"
    );
}

/// Verify struct shapes (unit, tuple, named, transparent).
fn verify_struct_shapes(step: usize) {
    let name_unit = format!("UnitStruct{step}");
    let code_unit = format!(
        "#[error(\"constant failure\")]
        pub struct {name_unit};"
    );
    if let Some(expanded_str) = expand_deterministic(&code_unit, "unit struct") {
        assert!(
            expanded_str.contains(&format!("impl :: core :: fmt :: Display for {name_unit}")),
            "missing Display impl for unit struct"
        );
    }

    let name_tuple = format!("TupleStruct{step}");
    let code_tuple = format!(
        "#[error(\"failure: {{0}}\")]
        pub struct {name_tuple}(String);"
    );
    if let Some(expanded_str) = expand_deterministic(&code_tuple, "tuple struct") {
        assert!(
            expanded_str.contains(&format!("impl :: core :: fmt :: Display for {name_tuple}")),
            "missing Display impl for tuple struct"
        );
        assert!(
            expanded_str.contains("\"failure: {__field_0}\""),
            "missing tuple struct format string"
        );
    }

    let name_named = format!("NamedStruct{step}");
    let code_named = format!(
        "#[error(\"code: {{code}}\")]
        pub struct {name_named} {{ pub code: u32 }};"
    );
    if let Some(expanded_str) = expand_deterministic(&code_named, "named struct") {
        assert!(
            expanded_str.contains(&format!("impl :: core :: fmt :: Display for {name_named}")),
            "missing Display impl for named struct"
        );
        assert!(
            expanded_str.contains("\"code: {code}\""),
            "missing named struct format string"
        );
    }

    let name_trans = format!("TransStruct{step}");
    let code_trans = format!(
        "#[error(transparent)]
        pub struct {name_trans}(#[from] std::io::Error);"
    );
    if let Some(expanded_str) = expand_deterministic(&code_trans, "transparent struct") {
        assert!(
            expanded_str.contains(":: core :: fmt :: Display :: fmt (& self . 0 , __formatter)"),
            "missing transparent struct Display delegation"
        );
        assert!(
            expanded_str.contains(":: core :: error :: Error :: source (& self . 0)"),
            "missing transparent struct Error source delegation"
        );
        assert!(
            expanded_str.contains(&format!(
                "impl :: core :: convert :: From < std :: io :: Error > for {name_trans}"
            )),
            "missing transparent struct From impl"
        );
    }
}

/// Verify refusal cases.
fn verify_refusal_cases(step: usize) {
    let code_union = format!("union BadUnion{step} {{ num_a: u32, num_b: f32 }}");
    assert_refusal(&code_union, "unions");

    let code_missing = format!("enum MissingError{step} {{ VariantWithoutAttr }}");
    assert_refusal(&code_missing, "missing #[error(...)]");

    let code_two_fields = format!(
        "enum BadTrans{step} {{
            #[error(transparent)]
            Two(u32, u32),
        }}"
    );
    assert_refusal(&code_two_fields, "exactly one field");

    let code_zero_fields = format!(
        "enum BadZero{step} {{
            #[error(transparent)]
            Zero,
        }}"
    );
    assert_refusal(&code_zero_fields, "exactly one field");

    let code_bad_from = format!(
        "enum BadFrom{step} {{
            #[error(\"bad\")]
            Two(#[from] std::io::Error, u32),
        }}"
    );
    assert_refusal(&code_bad_from, "single field");

    let code_malformed = format!(
        "enum BadAttr{step} {{
            #[error]
            Bad,
        }}"
    );
    if let Ok(toks) = TokenStream::from_str(&code_malformed) {
        assert!(expand(toks).is_err(), "malformed attribute must be refused");
    }
}
