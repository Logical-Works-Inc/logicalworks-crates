//! Spring parameter bounds.
//!
//! Counterpart of `motion-core/tests/spring_bounds.rs` (kernel#850 row 15):
//! non-finite parameters are refused per field, and the zero-tension,
//! zero-friction unit mass spring is valid.

use drishti::motion::node::spring::SpringConfig;

/// Non-finite tension, friction, and mass are each refused; zeros are valid.
#[test]
fn spring_parameters_refuse_non_finite_values() -> Result<(), String> {
    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        if SpringConfig::new(invalid, 1.0, 1.0).is_ok() {
            return Err(String::from("non-finite tension must be refused"));
        }
        if SpringConfig::new(1.0, invalid, 1.0).is_ok() {
            return Err(String::from("non-finite friction must be refused"));
        }
        if SpringConfig::new(1.0, 1.0, invalid).is_ok() {
            return Err(String::from("non-finite mass must be refused"));
        }
    }
    SpringConfig::new(0.0, 0.0, 1.0).map_err(|error| format!("zeros must be valid: {error}"))?;
    let config = SpringConfig::new(170.0, 26.0, 1.0)
        .map_err(|error| format!("valid spring rejected: {error}"))?;
    if config.tension().to_bits() != 170.0_f32.to_bits()
        || config.friction().to_bits() != 26.0_f32.to_bits()
        || config.mass().to_bits() != 1.0_f32.to_bits()
    {
        return Err(String::from("spring accessors must round-trip"));
    }
    Ok(())
}
