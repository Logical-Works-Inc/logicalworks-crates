//! Interpolator coverage: every value domain evaluates its endpoints.
//!
//! No kernel counterpart file; the kernel covered these domains through the
//! gamut and tolerance suites. This pins each interpolator's contract
//! directly so no public interpolator ships without a caller.

use drishti::motion::interpolate::clip_path::{Inset, interp_inset};
use drishti::motion::interpolate::color_oklab::{OkLab, lerp_oklab};
use drishti::motion::interpolate::matrix::{Transform2D, interp_transform};
use drishti::motion::interpolate::path_morph::{Point2, interp_path};
use drishti::motion::interpolate::scalar::lerp_scalar;
use drishti::motion::interpolate::vec::{Vec2, Vec3, lerp_vec2, lerp_vec3};
use drishti::motion::node::tween::AnimValue;

/// Scalars hit both endpoints exactly.
#[test]
fn scalars_hit_endpoints() -> Result<(), String> {
    if lerp_scalar(2.0, 8.0, 0.0).to_bits() != 2.0_f32.to_bits() {
        return Err(String::from("scalar must start at from"));
    }
    if lerp_scalar(2.0, 8.0, 1.0).to_bits() != 8.0_f32.to_bits() {
        return Err(String::from("scalar must end at to"));
    }
    Ok(())
}

/// Vectors blend per channel.
#[test]
fn vectors_blend_per_channel() -> Result<(), String> {
    let blended = lerp_vec2(Vec2::new(0.0, 10.0), Vec2::new(10.0, 20.0), 0.5);
    if blended.x().to_bits() != 5.0_f32.to_bits() || blended.y().to_bits() != 15.0_f32.to_bits() {
        return Err(String::from("vec2 must blend per channel"));
    }
    let blended = lerp_vec3(Vec3::new(0.0, 0.0, 0.0), Vec3::new(2.0, 4.0, 6.0), 0.5);
    if blended.x().to_bits() != 1.0_f32.to_bits()
        || blended.y().to_bits() != 2.0_f32.to_bits()
        || blended.z().to_bits() != 3.0_f32.to_bits()
    {
        return Err(String::from("vec3 must blend per channel"));
    }
    Ok(())
}

/// Transforms lerp every decomposed channel independently.
#[test]
fn transforms_lerp_channels() -> Result<(), String> {
    let from = Transform2D::new(0.0, 0.0, 1.0, 1.0, 0.0);
    let to = Transform2D::new(10.0, 20.0, 2.0, 2.0, 1.0);
    let mid = interp_transform(from, to, 0.5);
    if mid.translate_x().to_bits() != 5.0_f32.to_bits()
        || mid.translate_y().to_bits() != 10.0_f32.to_bits()
        || mid.scale_x().to_bits() != 1.5_f32.to_bits()
        || mid.scale_y().to_bits() != 1.5_f32.to_bits()
        || mid.rotation().to_bits() != 0.5_f32.to_bits()
    {
        return Err(String::from("transform must lerp every channel"));
    }
    Ok(())
}

/// Insets blend per side.
#[test]
fn insets_blend_per_side() -> Result<(), String> {
    let blended = interp_inset(
        Inset::new(0.0, 0.0, 0.0, 0.0),
        Inset::new(4.0, 8.0, 12.0, 16.0),
        0.5,
    );
    if blended.top().to_bits() != 2.0_f32.to_bits()
        || blended.right().to_bits() != 4.0_f32.to_bits()
        || blended.bottom().to_bits() != 6.0_f32.to_bits()
        || blended.left().to_bits() != 8.0_f32.to_bits()
    {
        return Err(String::from("inset must blend per side"));
    }
    Ok(())
}

/// Equal-length paths interpolate pointwise; mismatches report the shared
/// prefix length.
#[test]
fn paths_interpolate_pointwise() -> Result<(), String> {
    let from = [Point2::new(0.0, 0.0), Point2::new(10.0, 10.0)];
    let to = [Point2::new(0.0, 10.0), Point2::new(10.0, 20.0)];
    let mut out = [Point2::new(0.0, 0.0), Point2::new(0.0, 0.0)];
    let written = interp_path(&from, &to, 0.5, &mut out);
    if written != 2 {
        return Err(String::from("equal paths must write fully"));
    }
    if out[0].x().to_bits() != 0.0_f32.to_bits() || out[0].y().to_bits() != 5.0_f32.to_bits() {
        return Err(String::from("path must interpolate pointwise"));
    }
    let written = interp_path(&from, &to[..1], 0.5, &mut out[..1]);
    if written != 1 {
        return Err(String::from(
            "mismatched paths must report the shared prefix",
        ));
    }
    Ok(())
}

/// OKLab lerps channel-wise without gamut mapping.
#[test]
fn oklab_lerps_channels() -> Result<(), String> {
    let blended = lerp_oklab(OkLab::new(0.0, 0.0, 0.0), OkLab::new(1.0, 2.0, 4.0), 0.5);
    if blended.lightness().to_bits() != 0.5_f32.to_bits()
        || blended.axis_a().to_bits() != 1.0_f32.to_bits()
        || blended.axis_b().to_bits() != 2.0_f32.to_bits()
    {
        return Err(String::from("oklab must lerp every channel"));
    }
    let value = AnimValue::new(3.0);
    if value.value().to_bits() != 3.0_f32.to_bits() {
        return Err(String::from("anim value must round-trip"));
    }
    Ok(())
}
