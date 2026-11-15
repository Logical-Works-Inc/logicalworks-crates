//! Scrub sources: constants repeat, maps resolve, misses stay misses.
//!
//! No kernel counterpart file; the kernel exercised scrub maps only through
//! frame tests. This pins the source contracts directly.

use drishti::motion::{ConstantScrub, EmptyScrubMap, HashScrubMap, ScrubSource, ScrubSourceMap};

/// Constant sources repeat their value; the empty map always misses.
#[test]
fn constants_repeat_and_empty_misses() -> Result<(), String> {
    let constant = ConstantScrub::new(0.25);
    if constant.value().to_bits() != 0.25_f32.to_bits() {
        return Err(String::from("constant must repeat its value"));
    }
    let empty = EmptyScrubMap::new();
    if empty.get("anything").is_some() {
        return Err(String::from("empty map must miss"));
    }
    Ok(())
}

/// Hash maps resolve inserts and miss unknown ids.
#[test]
fn maps_resolve_inserts() -> Result<(), String> {
    let mut map = HashScrubMap::new();
    map.insert(
        String::from("pointer.x"),
        Box::new(ConstantScrub::new(0.75)),
    );
    let Some(source) = map.get("pointer.x") else {
        return Err(String::from("inserted source must resolve"));
    };
    if source.value().to_bits() != 0.75_f32.to_bits() {
        return Err(String::from("resolved source must repeat"));
    }
    if map.get("pointer.y").is_some() {
        return Err(String::from("unknown id must miss"));
    }
    Ok(())
}
