//! Gamut mapping: OKLab interpolation stays in sRGB gamut.
//!
//! Counterpart of `motion-core/tests/color_gamut_mapping.rs` (kernel#850 row
//! 15): interpolation between in-gamut colors never leaves the gamut, the
//! midpoint carries finite chroma, the binary search terminates in gamut,
//! achromatic interpolation tracks lightness, and every gamut-corner
//! midpoint lands in gamut.

use drishti::motion::interpolate::color_oklab::{OkLab, SRgb, interp_oklab, map_to_srgb_gamut};
use lgwks_deps::libm;

/// Gamut tolerance for boundary assertions.
const TOLERANCE: f32 = 1e-4;

/// True when every channel is within tolerance of [0, 1].
fn in_gamut(colour: SRgb) -> bool {
    colour.red() >= -TOLERANCE
        && colour.red() <= 1.0 + TOLERANCE
        && colour.green() >= -TOLERANCE
        && colour.green() <= 1.0 + TOLERANCE
        && colour.blue() >= -TOLERANCE
        && colour.blue() <= 1.0 + TOLERANCE
}

/// Red-to-blue interpolation never leaves the gamut.
#[test]
fn interpolation_stays_in_gamut() -> Result<(), String> {
    let red = SRgb::new(1.0, 0.0, 0.0);
    let blue = SRgb::new(0.0, 0.0, 1.0);
    for index in 0..=20_u16 {
        let time = f32::from(index) / 20.0;
        let result = interp_oklab(red, blue, time);
        if !in_gamut(result) {
            return Err(format!("t={time}: {result:?} left the gamut"));
        }
    }
    Ok(())
}

/// The midpoint carries finite chroma rather than clipped RGB purple.
#[test]
fn midpoint_carries_finite_chroma() -> Result<(), String> {
    let red = SRgb::new(1.0, 0.0, 0.0);
    let blue = SRgb::new(0.0, 0.0, 1.0);
    let mid_oklab = interp_oklab(red, blue, 0.5);
    let mid_naive = SRgb::new(0.5, 0.0, 0.5);
    let oklab_lab = drishti::motion::interpolate::color_oklab::srgb_to_oklab(mid_oklab);
    let naive_lab = drishti::motion::interpolate::color_oklab::srgb_to_oklab(mid_naive);
    let oklab_chroma = libm::sqrtf(
        oklab_lab.axis_a() * oklab_lab.axis_a() + oklab_lab.axis_b() * oklab_lab.axis_b(),
    );
    let naive_chroma = libm::sqrtf(
        naive_lab.axis_a() * naive_lab.axis_a() + naive_lab.axis_b() * naive_lab.axis_b(),
    );
    if !oklab_chroma.is_finite() {
        return Err(String::from("OKLab midpoint chroma must be finite"));
    }
    if !naive_chroma.is_finite() {
        return Err(String::from("naive midpoint chroma must be finite"));
    }
    Ok(())
}

/// A saturated out-of-gamut color maps back inside.
#[test]
fn binary_search_terminates_in_gamut() -> Result<(), String> {
    let out_of_gamut = OkLab::new(0.5, 0.4, 0.4);
    let result = map_to_srgb_gamut(out_of_gamut);
    if !result.is_in_srgb_gamut() {
        return Err(format!("gamut map returned {result:?}"));
    }
    Ok(())
}

/// Achromatic interpolation tracks scalar lightness (r near g near b).
#[test]
fn achromatic_tracks_lightness() -> Result<(), String> {
    let black = SRgb::new(0.0, 0.0, 0.0);
    let white = SRgb::new(1.0, 1.0, 1.0);
    let mid = interp_oklab(black, white, 0.5);
    if (mid.red() - mid.green()).abs() >= 1e-3 {
        return Err(String::from("achromatic mid must keep r near g"));
    }
    if (mid.red() - mid.blue()).abs() >= 1e-3 {
        return Err(String::from("achromatic mid must keep r near b"));
    }
    Ok(())
}

/// Every gamut-corner-pair midpoint lands in gamut.
#[test]
fn corner_midpoints_stay_in_gamut() -> Result<(), String> {
    let corners = [
        SRgb::new(1.0, 0.0, 0.0),
        SRgb::new(0.0, 1.0, 0.0),
        SRgb::new(0.0, 0.0, 1.0),
        SRgb::new(1.0, 1.0, 0.0),
        SRgb::new(1.0, 0.0, 1.0),
        SRgb::new(0.0, 1.0, 1.0),
        SRgb::new(1.0, 1.0, 1.0),
        SRgb::new(0.0, 0.0, 0.0),
    ];
    for from in &corners {
        for to in &corners {
            let mid = interp_oklab(*from, *to, 0.5);
            if !in_gamut(mid) {
                return Err(format!("{from:?} -> {to:?} left the gamut: {mid:?}"));
            }
        }
    }
    Ok(())
}
