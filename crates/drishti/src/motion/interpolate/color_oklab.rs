//! OKLab perceptual colour interpolation with CSS Color 4 gamut mapping.
//!
//! Absorbed from the kernel motion core. Naive RGB clamping shifts hue, so
//! out-of-gamut colours are pulled back by chroma-reduction binary search
//! (CSS Color Module Level 4 section 10.9 strategy (c), the normative
//! standard). All transcendental calls go through `libm` for
//! cross-architecture determinism.
//!
//! The OKLab matrices below are Bjorn Ottosson's published 10-significant-
//! digit constants. They are written as `f32::from_bits` hex with exactly the
//! bits the kernel's `as f32` narrowing produced (verified by an `as f32`
//! probe script; the hex is the auditable value, not a fresh decimal
//! rounding). The `cone_light`/`cone_medium`/`cone_short` names track the LMS
//! long/medium/short cone-response rows of his matrix M1; renaming one row
//! without the others would break the audit correspondence to the paper,
//! which is the only way to check these constants.

use lgwks_deps::libm;

/// OKLab matrix M1 row for the long cone response (Ottosson 2020).
const M1_LONG_RED: f32 = f32::from_bits(0x3ED3_0EB1);
/// OKLab matrix M1 row for the long cone response, green channel.
const M1_LONG_GREEN: f32 = f32::from_bits(0x3F09_4D17);
/// OKLab matrix M1 row for the long cone response, blue channel.
const M1_LONG_BLUE: f32 = f32::from_bits(0x3D52_B909);
/// OKLab matrix M1 row for the medium cone response, red channel.
const M1_MEDIUM_RED: f32 = f32::from_bits(0x3E58_FD3B);
/// OKLab matrix M1 row for the medium cone response, green channel.
const M1_MEDIUM_GREEN: f32 = f32::from_bits(0x3F2E_4253);
/// OKLab matrix M1 row for the medium cone response, blue channel.
const M1_MEDIUM_BLUE: f32 = f32::from_bits(0x3DDB_F2F0);
/// OKLab matrix M1 row for the short cone response, red channel.
const M1_SHORT_RED: f32 = f32::from_bits(0x3DB4_D7EC);
/// OKLab matrix M1 row for the short cone response, green channel.
const M1_SHORT_GREEN: f32 = f32::from_bits(0x3E90_3D74);
/// OKLab matrix M1 row for the short cone response, blue channel.
const M1_SHORT_BLUE: f32 = f32::from_bits(0x3F21_4649);
/// OKLab matrix M2 lightness row, long channel.
const M2_LIGHT_LONG: f32 = f32::from_bits(0x3E57_8152);
/// OKLab matrix M2 lightness row, medium channel.
const M2_LIGHT_MEDIUM: f32 = f32::from_bits(0x3F4B_2A89);
/// OKLab matrix M2 lightness row, short channel.
const M2_LIGHT_SHORT: f32 = f32::from_bits(0x3B85_6ECE);
/// OKLab matrix M2 green-red row, long channel.
const M2_AXIS_LONG: f32 = f32::from_bits(0x3FFD_2F0E);
/// OKLab matrix M2 green-red row, medium channel.
const M2_AXIS_MEDIUM: f32 = f32::from_bits(0x401B_6E0E);
/// OKLab matrix M2 green-red row, short channel.
const M2_AXIS_SHORT: f32 = f32::from_bits(0x3EE6_B438);
/// OKLab matrix M2 blue-yellow row, long channel.
const M2_BLUE_LONG: f32 = f32::from_bits(0x3CD4_34B4);
/// OKLab matrix M2 blue-yellow row, medium channel.
const M2_BLUE_MEDIUM: f32 = f32::from_bits(0x3F48_63BB);
/// OKLab matrix M2 blue-yellow row, short channel.
const M2_BLUE_SHORT: f32 = f32::from_bits(0x3F4F_0560);
/// OKLab inverse M2 long row, lightness channel.
const INV_M2_LONG_LIGHT: f32 = f32::from_bits(0x3ECA_ECC9);
/// OKLab inverse M2 long row, green-red channel.
const INV_M2_LONG_AXIS: f32 = f32::from_bits(0x3E5C_FBA9);
/// OKLab inverse M2 medium row, lightness channel.
const INV_M2_MEDIUM_LIGHT: f32 = f32::from_bits(0x3DD8_308C);
/// OKLab inverse M2 medium row, green-red channel.
const INV_M2_MEDIUM_AXIS: f32 = f32::from_bits(0x3D82_C5FA);
/// OKLab inverse M2 short row, lightness channel.
const INV_M2_SHORT_LIGHT: f32 = f32::from_bits(0x3DB7_437B);
/// OKLab inverse M2 short row, blue-yellow channel.
const INV_M2_SHORT_BLUE: f32 = f32::from_bits(0x3FA5_4F66);
/// OKLab inverse M1 red row, long channel.
const INV_M1_RED_LONG: f32 = f32::from_bits(0x4082_74AB);
/// OKLab inverse M1 red row, medium channel.
const INV_M1_RED_MEDIUM: f32 = f32::from_bits(0x4053_B18C);
/// OKLab inverse M1 red row, short channel.
const INV_M1_RED_SHORT: f32 = f32::from_bits(0x3E6C_8362);
/// OKLab inverse M1 green row, long channel.
const INV_M1_GREEN_LONG: f32 = f32::from_bits(0x3FA2_5C2D);
/// OKLab inverse M1 green row, medium channel.
const INV_M1_GREEN_MEDIUM: f32 = f32::from_bits(0x4027_0644);
/// OKLab inverse M1 green row, short channel.
const INV_M1_GREEN_SHORT: f32 = f32::from_bits(0x3EAE_C16A);
/// OKLab inverse M1 blue row, long channel.
const INV_M1_BLUE_LONG: f32 = f32::from_bits(0x3B89_7F53);
/// OKLab inverse M1 blue row, medium channel.
const INV_M1_BLUE_MEDIUM: f32 = f32::from_bits(0x3F34_133E);
/// OKLab inverse M1 blue row, short channel.
const INV_M1_BLUE_SHORT: f32 = f32::from_bits(0x3FDA_931E);

/// OKLab colour with lightness in [0, 1].
///
/// Fields are private: construction runs through [`OkLab::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::OkLab;
///
/// let colour = OkLab::new(0.5, 0.01, -0.01);
/// assert_eq!(colour.lightness(), 0.5);
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct OkLab {
    /// Lightness.
    lightness: f32,
    /// Green-red axis.
    axis_a: f32,
    /// Blue-yellow axis.
    axis_b: f32,
}

impl OkLab {
    /// Assemble an OKLab colour from lightness and the two opponent axes.
    /// Nothing is clamped, so an out-of-range colour can be built and later
    /// pulled into gamut by [`map_to_srgb_gamut`].
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::OkLab;
    ///
    /// let colour = OkLab::new(0.7, 0.3, 0.3);
    /// assert_eq!((colour.lightness(), colour.axis_a(), colour.axis_b()), (0.7, 0.3, 0.3));
    /// ```
    #[must_use]
    pub const fn new(lightness: f32, axis_a: f32, axis_b: f32) -> Self {
        Self {
            lightness,
            axis_a,
            axis_b,
        }
    }

    /// Perceptual lightness: 0 is black and 1 is the sRGB white point.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::OkLab;
    ///
    /// assert_eq!(OkLab::new(0.25, 0.0, 0.0).lightness(), 0.25);
    /// ```
    #[must_use]
    pub const fn lightness(&self) -> f32 {
        self.lightness
    }

    /// Position on the green-red axis: negative toward green, positive toward
    /// red.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::OkLab;
    ///
    /// assert_eq!(OkLab::new(0.5, -0.1, 0.2).axis_a(), -0.1);
    /// ```
    #[must_use]
    pub const fn axis_a(&self) -> f32 {
        self.axis_a
    }

    /// Position on the blue-yellow axis: negative toward blue, positive toward
    /// yellow.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::OkLab;
    ///
    /// assert_eq!(OkLab::new(0.5, -0.1, 0.2).axis_b(), 0.2);
    /// ```
    #[must_use]
    pub const fn axis_b(&self) -> f32 {
        self.axis_b
    }
}

/// sRGB colour with channels in [0, 1] (may exceed gamut before mapping).
///
/// Fields are private: construction runs through [`SRgb::new`] and reads go
/// through the accessors.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::SRgb;
///
/// let colour = SRgb::new(1.0, 0.0, 0.0);
/// assert!(colour.is_in_srgb_gamut());
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SRgb {
    /// Red channel.
    red: f32,
    /// Green channel.
    green: f32,
    /// Blue channel.
    blue: f32,
}

impl SRgb {
    /// Assemble an sRGB colour from gamma-encoded channels. Values outside
    /// [0, 1] are kept so the gamut helpers can see them.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// let colour = SRgb::new(0.2, 0.4, 0.6);
    /// assert_eq!((colour.red(), colour.green(), colour.blue()), (0.2, 0.4, 0.6));
    /// ```
    #[must_use]
    pub const fn new(red: f32, green: f32, blue: f32) -> Self {
        Self { red, green, blue }
    }

    /// Gamma-encoded red intensity, 1.0 at full output.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// assert_eq!(SRgb::new(0.2, 0.4, 0.6).red(), 0.2);
    /// ```
    #[must_use]
    pub const fn red(&self) -> f32 {
        self.red
    }

    /// Gamma-encoded green intensity, 1.0 at full output.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// assert_eq!(SRgb::new(0.2, 0.4, 0.6).green(), 0.4);
    /// ```
    #[must_use]
    pub const fn green(&self) -> f32 {
        self.green
    }

    /// Gamma-encoded blue intensity, 1.0 at full output.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// assert_eq!(SRgb::new(0.2, 0.4, 0.6).blue(), 0.6);
    /// ```
    #[must_use]
    pub const fn blue(&self) -> f32 {
        self.blue
    }

    /// True when all channels are in [0, 1]. A NaN channel is never in gamut.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// assert!(SRgb::new(0.2, 0.4, 0.6).is_in_srgb_gamut());
    /// assert!(!SRgb::new(1.2, 0.0, 0.0).is_in_srgb_gamut());
    /// ```
    #[must_use]
    pub const fn is_in_srgb_gamut(self) -> bool {
        self.red >= 0.0
            && self.red <= 1.0
            && self.green >= 0.0
            && self.green <= 1.0
            && self.blue >= 0.0
            && self.blue <= 1.0
    }

    /// Clamp every channel to [0, 1] independently. This shifts hue for a
    /// strongly out-of-gamut colour; [`map_to_srgb_gamut`] preserves hue
    /// instead.
    ///
    /// # Examples
    ///
    /// ```
    /// use drishti::motion::interpolate::color_oklab::SRgb;
    ///
    /// let clamped = SRgb::new(2.0, -1.0, 0.5).clamp();
    /// assert_eq!(clamped, SRgb::new(1.0, 0.0, 0.5));
    /// ```
    #[must_use]
    pub fn clamp(self) -> Self {
        Self::new(
            self.red.clamp(0.0, 1.0),
            self.green.clamp(0.0, 1.0),
            self.blue.clamp(0.0, 1.0),
        )
    }
}

/// Interpolate two sRGB colours in OKLab space, then map back to gamut.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::{SRgb, interp_oklab};
///
/// let black = SRgb::new(0.0, 0.0, 0.0);
/// let white = SRgb::new(1.0, 1.0, 1.0);
/// let grey = interp_oklab(black, white, 0.5);
/// assert!(grey.is_in_srgb_gamut());
/// // Half of OKLab lightness is darker than half of sRGB intensity.
/// assert!((grey.red() - 0.3886).abs() < 0.01);
/// assert!((grey.red() - grey.green()).abs() < 1e-3);
/// ```
#[must_use]
pub fn interp_oklab(from: SRgb, to: SRgb, mix: f32) -> SRgb {
    let from_lab = srgb_to_oklab(from);
    let to_lab = srgb_to_oklab(to);
    map_to_srgb_gamut(lerp_oklab(from_lab, to_lab, mix))
}

/// CSS Color 4 section 10.9: binary-search chroma reduction, terminating
/// within eight iterations. The result is always inside the sRGB gamut.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::{OkLab, map_to_srgb_gamut};
///
/// // A vivid OKLab colour no sRGB display can show is pulled back inside.
/// let mapped = map_to_srgb_gamut(OkLab::new(0.7, 0.3, 0.3));
/// assert!(mapped.is_in_srgb_gamut());
///
/// // An achromatic colour has no chroma to reduce: the channels stay equal.
/// let grey = map_to_srgb_gamut(OkLab::new(0.5, 0.0, 0.0));
/// assert!((grey.red() - grey.blue()).abs() < 1e-3);
/// ```
#[must_use]
pub fn map_to_srgb_gamut(colour: OkLab) -> SRgb {
    let candidate = oklab_to_srgb(colour);
    if candidate.is_in_srgb_gamut() {
        return candidate.clamp();
    }
    // Achromatic fallback: chroma already near zero, so just clamp lightness.
    let chroma = libm::sqrtf(colour.axis_a * colour.axis_a + colour.axis_b * colour.axis_b);
    if chroma < 1e-6 {
        let lightness_clamped = colour.lightness.clamp(0.0, 1.0);
        return oklab_to_srgb(OkLab::new(lightness_clamped, 0.0, 0.0)).clamp();
    }
    // Binary search: reduce chroma until the candidate is in gamut.
    let hue_a = colour.axis_a / chroma;
    let hue_b = colour.axis_b / chroma;
    let mut low: f32 = 0.0;
    let mut high: f32 = chroma;
    for _ in 0..8 {
        let mid = (low + high) / 2.0;
        let trial = OkLab::new(colour.lightness, hue_a * mid, hue_b * mid);
        if oklab_to_srgb(trial).is_in_srgb_gamut() {
            low = mid;
        } else {
            high = mid;
        }
    }
    oklab_to_srgb(OkLab::new(colour.lightness, hue_a * low, hue_b * low)).clamp()
}

/// Linear interpolation between two OKLab colours, axis by axis. The result is
/// not gamut-mapped.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::{OkLab, lerp_oklab};
///
/// let mixed = lerp_oklab(OkLab::new(0.0, 0.0, 0.0), OkLab::new(1.0, 0.2, -0.2), 0.5);
/// assert_eq!(mixed.lightness(), 0.5);
/// assert!((mixed.axis_a() - 0.1).abs() < 1e-6);
/// assert!((mixed.axis_b() + 0.1).abs() < 1e-6);
/// ```
#[must_use]
pub const fn lerp_oklab(from: OkLab, to: OkLab, mix: f32) -> OkLab {
    OkLab::new(
        from.lightness + (to.lightness - from.lightness) * mix,
        from.axis_a + (to.axis_a - from.axis_a) * mix,
        from.axis_b + (to.axis_b - from.axis_b) * mix,
    )
}

/// sRGB into linear sRGB into OKLab. Uses `libm` for cube roots.
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::{SRgb, srgb_to_oklab};
///
/// let white = srgb_to_oklab(SRgb::new(1.0, 1.0, 1.0));
/// assert!((white.lightness() - 1.0).abs() < 1e-3);
/// assert!(white.axis_a().abs() < 1e-3 && white.axis_b().abs() < 1e-3);
///
/// let black = srgb_to_oklab(SRgb::new(0.0, 0.0, 0.0));
/// assert!(black.lightness().abs() < 1e-6);
/// ```
#[must_use]
pub fn srgb_to_oklab(colour: SRgb) -> OkLab {
    let red = srgb_channel_to_linear(colour.red);
    let green = srgb_channel_to_linear(colour.green);
    let blue = srgb_channel_to_linear(colour.blue);

    // OKLab matrix M1 (Ottosson 2020).
    let cone_light = M1_LONG_RED * red + M1_LONG_GREEN * green + M1_LONG_BLUE * blue;
    let cone_medium = M1_MEDIUM_RED * red + M1_MEDIUM_GREEN * green + M1_MEDIUM_BLUE * blue;
    let cone_short = M1_SHORT_RED * red + M1_SHORT_GREEN * green + M1_SHORT_BLUE * blue;

    let cone_light_nonlinear = libm::cbrtf(cone_light);
    let cone_medium_nonlinear = libm::cbrtf(cone_medium);
    let cone_short_nonlinear = libm::cbrtf(cone_short);

    OkLab::new(
        M2_LIGHT_LONG * cone_light_nonlinear + M2_LIGHT_MEDIUM * cone_medium_nonlinear
            - M2_LIGHT_SHORT * cone_short_nonlinear,
        M2_AXIS_LONG * cone_light_nonlinear - M2_AXIS_MEDIUM * cone_medium_nonlinear
            + M2_AXIS_SHORT * cone_short_nonlinear,
        M2_BLUE_LONG * cone_light_nonlinear + M2_BLUE_MEDIUM * cone_medium_nonlinear
            - M2_BLUE_SHORT * cone_short_nonlinear,
    )
}

/// OKLab into linear sRGB into sRGB. Uses `libm` for cube roots. The result
/// is NOT gamut-mapped: an out-of-gamut colour yields channels outside [0, 1].
///
/// # Examples
///
/// ```
/// use drishti::motion::interpolate::color_oklab::{SRgb, oklab_to_srgb, srgb_to_oklab};
///
/// let original = SRgb::new(0.2, 0.4, 0.6);
/// let round_trip = oklab_to_srgb(srgb_to_oklab(original));
/// assert!((round_trip.red() - original.red()).abs() < 1e-3);
/// assert!((round_trip.green() - original.green()).abs() < 1e-3);
/// assert!((round_trip.blue() - original.blue()).abs() < 1e-3);
/// ```
#[must_use]
pub fn oklab_to_srgb(colour: OkLab) -> SRgb {
    // OKLab inverse M2.
    let cone_light_nonlinear =
        colour.lightness + INV_M2_LONG_LIGHT * colour.axis_a + INV_M2_LONG_AXIS * colour.axis_b;
    let cone_medium_nonlinear =
        colour.lightness - INV_M2_MEDIUM_LIGHT * colour.axis_a - INV_M2_MEDIUM_AXIS * colour.axis_b;
    let cone_short_nonlinear =
        colour.lightness - INV_M2_SHORT_LIGHT * colour.axis_a - INV_M2_SHORT_BLUE * colour.axis_b;

    let cone_light = cone_light_nonlinear * cone_light_nonlinear * cone_light_nonlinear;
    let cone_medium = cone_medium_nonlinear * cone_medium_nonlinear * cone_medium_nonlinear;
    let cone_short = cone_short_nonlinear * cone_short_nonlinear * cone_short_nonlinear;

    // OKLab inverse M1.
    let red_linear = INV_M1_RED_LONG * cone_light - INV_M1_RED_MEDIUM * cone_medium
        + INV_M1_RED_SHORT * cone_short;
    let green_linear = -INV_M1_GREEN_LONG * cone_light + INV_M1_GREEN_MEDIUM * cone_medium
        - INV_M1_GREEN_SHORT * cone_short;
    let blue_linear = -INV_M1_BLUE_LONG * cone_light - INV_M1_BLUE_MEDIUM * cone_medium
        + INV_M1_BLUE_SHORT * cone_short;

    SRgb::new(
        linear_to_srgb_channel(red_linear),
        linear_to_srgb_channel(green_linear),
        linear_to_srgb_channel(blue_linear),
    )
}

/// One sRGB channel into linear light.
fn srgb_channel_to_linear(channel: f32) -> f32 {
    if channel <= 0.04045 {
        channel / 12.92
    } else {
        libm::powf((channel + 0.055) / 1.055, 2.4)
    }
}

/// One linear-light channel into sRGB.
fn linear_to_srgb_channel(channel: f32) -> f32 {
    if channel <= 0.0031308 {
        channel * 12.92
    } else {
        1.055 * libm::powf(channel, 1.0 / 2.4) - 0.055
    }
}
