//! CPU reference painter with cached shaped text.
//!
//! Absorbed from `primitives/canvas-native/crates/preview/src/paint.rs`.
//! Font and driver RSS is not bounded by the pixel limit. Available only with
//! the `desktop` feature.

use lgwks_deps::cosmic_text::{Attrs, Buffer, Color, FontSystem, Metrics, Shaping, SwashCache};

use super::Failure;
use crate::canvas_scene::{Frame, View};

/// Reference painter: gradient material plus cached text sections.
pub(crate) struct Painter {
    /// Font system for shaping.
    fonts: FontSystem,
    /// Glyph raster cache, rebuilt when the text signature changes.
    cache: SwashCache,
    /// Shaped buffers: title, three views, three nav labels.
    buffers: Vec<Buffer>,
    /// Cache key for the shaped text.
    signature: Option<(u32, u32, u64, &'static str, [usize; 3])>,
}

impl Painter {
    /// Empty painter with a fresh font system.
    pub(crate) fn new() -> Self {
        Self {
            fonts: FontSystem::new(),
            cache: SwashCache::new(),
            buffers: Vec::new(),
            signature: None,
        }
    }

    /// Shape one text block.
    fn text(&mut self, text: &str, width: f32, height: f32, size: f32) -> Buffer {
        let mut buffer = Buffer::new(&mut self.fonts, Metrics::new(size, size * 1.5));
        {
            let mut borrowed = buffer.borrow_with(&mut self.fonts);
            borrowed.set_size(Some(width.max(1.0)), Some(height.max(1.0)));
            borrowed.set_text(text, &Attrs::new(), Shaping::Advanced);
            borrowed.shape_until_scroll(true);
        }
        buffer
    }

    /// Reshape cached text when the frame signature changes.
    fn prepare_text(&mut self, frame: &Frame) {
        let signature = (
            frame.width(),
            frame.height(),
            frame.artifact().revision(),
            frame.status_label(),
            frame.scroll(),
        );
        if self.signature == Some(signature) {
            return;
        }
        self.buffers.clear();
        self.cache = SwashCache::new();
        let title = format!(
            "CANVAS / READ-ONLY / {}\n{}",
            frame.status_label().to_uppercase(),
            frame.artifact().key()
        );
        let title_buffer = self.text(
            &title,
            f32_from_u32(frame.width().saturating_sub(64)),
            70.0,
            16.0,
        );
        self.buffers.push(title_buffer);
        for view in View::ALL {
            let section = format!(
                "{}{}\n\n{}",
                view.label(),
                if view != View::Source && frame.artifact().supplied(view) {
                    ": supplied text, not verified"
                } else {
                    ""
                },
                frame
                    .artifact()
                    .text(view)
                    .lines()
                    .skip(frame.scroll()[view.index()])
                    .take(80)
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            let buffer = self.text(
                &section,
                f32_from_u32(frame.width().saturating_sub(64)),
                f32_from_u32(frame.height().saturating_sub(170)),
                15.0,
            );
            self.buffers.push(buffer);
        }
        for view in View::ALL {
            let buffer = self.text(
                &format!("{}  {}", view.index().wrapping_add(1), view.label()),
                f32_from_u32(frame.width()) / 3.0 - 12.0,
                40.0,
                16.0,
            );
            self.buffers.push(buffer);
        }
        self.signature = Some(signature);
    }

    /// Rasterize one frame into a row-major `u32` pixel buffer.
    ///
    /// # Errors
    ///
    /// Returns a failure when the buffer length mismatches the viewport or a
    /// dimension conversion overflows.
    pub(crate) fn draw(&mut self, frame: &Frame, pixels: &mut [u32]) -> Result<(), Failure> {
        let width = usize::try_from(frame.width())?;
        let height = usize::try_from(frame.height())?;
        let pixel_count = width.checked_mul(height).ok_or("pixel size overflow")?;
        require_input(pixels.len() == pixel_count, "wrong surface buffer length")?;
        if width == 0 || height == 0 {
            return Ok(());
        }
        self.prepare_text(frame);
        // Low-frequency material, deterministic and static once motion settles.
        let width_float = f64_from_usize(width)?;
        let height_float = f64_from_usize(height)?;
        for (index, pixel) in pixels.iter_mut().enumerate() {
            let column = index.checked_rem(width).ok_or("zero-width surface")?;
            let row = index.checked_div(width).ok_or("zero-width surface")?;
            let horizontal = f64_from_usize(column)? / width_float;
            let vertical = f64_from_usize(row)? / height_float;
            let anchor = 0.2 + frame.position().clamp(0.0, 2.0) * 0.3;
            let strength = (1.0 - (horizontal - anchor).abs() * 1.4 - (vertical - 0.5).abs() * 0.5)
                .clamp(0.0, 1.0);
            let red = pixel_channel(239.0 - strength * 25.0);
            let green = pixel_channel(235.0 - strength * 9.0);
            let blue = pixel_channel(223.0 + strength * 11.0);
            *pixel = red.wrapping_shl(16) | green.wrapping_shl(8) | blue;
        }
        for (index, buffer) in self.buffers.iter_mut().enumerate() {
            let (left, top, clip_top, clip_bottom) = match index {
                0 => (32, 18, 0, 95),
                1..=3 => {
                    let scrolled = i32_from_f64_saturating(
                        (f64_from_usize(index.wrapping_sub(1))? - frame.position()) * width_float,
                    )
                    .wrapping_add(32);
                    (
                        scrolled,
                        104,
                        96,
                        i32_from_usize_saturating(height.wrapping_sub(64)),
                    )
                }
                _ => (
                    18_i32.wrapping_add(i32_from_usize_saturating(
                        index.wrapping_sub(4).wrapping_mul(width).wrapping_div(3),
                    )),
                    i32_from_usize_saturating(height.wrapping_sub(42)),
                    i32_from_usize_saturating(height.wrapping_sub(56)),
                    i32_from_usize_saturating(height),
                ),
            };
            let mut borrowed = buffer.borrow_with(&mut self.fonts);
            let width_limit = i32_from_usize_saturating(width);
            let height_limit = i32_from_usize_saturating(height);
            borrowed.draw(
                &mut self.cache,
                Color::rgb(30, 42, 46),
                |glyph_x, glyph_y, glyph_w, glyph_h, color| {
                    let alpha = u32::from(color.a());
                    let row_start = (top.wrapping_add(glyph_y)).max(clip_top).max(0);
                    let row_end = (top
                        .wrapping_add(glyph_y)
                        .wrapping_add(i32_from_u32_saturating(glyph_h)))
                    .min(clip_bottom)
                    .min(height_limit);
                    for row in row_start..row_end {
                        let column_start = (left.wrapping_add(glyph_x)).max(0);
                        let column_end = (left
                            .wrapping_add(glyph_x)
                            .wrapping_add(i32_from_u32_saturating(glyph_w)))
                        .min(width_limit);
                        for column in column_start..column_end {
                            let offset = i32_from_usize_saturating(width)
                                .wrapping_mul(row)
                                .wrapping_add(column);
                            let Some(pixel) =
                                usize_from_i32(offset).and_then(|at| pixels.get_mut(at))
                            else {
                                continue;
                            };
                            let background = *pixel;
                            let blend = |foreground: u8, shift: u32| -> u32 {
                                (u32::from(foreground).wrapping_mul(alpha).wrapping_add(
                                    (background.wrapping_shr(shift) & 255)
                                        .wrapping_mul(255_u32.wrapping_sub(alpha)),
                                ))
                                .wrapping_div(255)
                            };
                            *pixel = (blend(color.r(), 16).wrapping_shl(16))
                                | (blend(color.g(), 8).wrapping_shl(8))
                                | blend(color.b(), 0);
                        }
                    }
                },
            );
        }
        // A stable target indicator, not a container around every component.
        let indicator_start = frame.target().index().wrapping_mul(width).wrapping_div(3);
        let indicator_end = frame
            .target()
            .index()
            .wrapping_add(1)
            .wrapping_mul(width)
            .wrapping_div(3);
        for row in height.saturating_sub(4)..height {
            for column in indicator_start..indicator_end {
                let at = row.wrapping_mul(width).wrapping_add(column);
                if let Some(pixel) = pixels.get_mut(at) {
                    *pixel = 0x25494D;
                }
            }
        }
        Ok(())
    }
}

/// Enforce a painter precondition without adding an effect.
fn require_input(valid: bool, message: &'static str) -> Result<(), Failure> {
    if valid { Ok(()) } else { Err(message.into()) }
}

/// Truncate a painter channel in [128, 256) to `u32` without an `as` cast.
///
/// All three gradient channels stay in this range by construction
/// (red [214, 239], green [226, 235], blue [223, 234]). The input is clamped
/// first as a backstop; the binary exponent is fixed across the range, so the
/// integer part is the leading one plus the top seven mantissa bits.
fn pixel_channel(value: f64) -> u32 {
    let clamped = value.clamp(128.0, 255.0);
    let bits = clamped.to_bits();
    // The low byte of the shifted bits, masked to the seven mantissa bits.
    let [low, ..] = bits.wrapping_shr(45).to_le_bytes();
    128_u32.wrapping_add(u32::from(low & 0x7f))
}

/// Convert a `u32` viewport extent to `f32` without an `as` cast: the two
/// big-endian 16-bit halves are exact, so the only rounding is the `f32`
/// mantissa's own.
fn f32_from_u32(value: u32) -> f32 {
    let [b3, b2, b1, b0] = value.to_be_bytes();
    let high = u16::from_be_bytes([b3, b2]);
    let low = u16::from_be_bytes([b1, b0]);
    f32::from(high) * 65_536.0 + f32::from(low)
}

/// Convert a `usize` pixel count to `f64` for gradient math.
fn f64_from_usize(value: usize) -> Result<f64, Failure> {
    match u32::try_from(value) {
        Ok(narrow) => Ok(f64::from(narrow)),
        Err(_) => Err("pixel dimension overflow".into()),
    }
}

/// Convert a `usize` pixel count to `i32`, saturating on overflow.
fn i32_from_usize_saturating(value: usize) -> i32 {
    // Clamped to i32::MAX first, so the low four little-endian bytes hold the
    // whole value on every 32- and 64-bit target.
    let [b0, b1, b2, b3, ..] = value.min(0x7fff_ffff).to_le_bytes();
    i32::from_le_bytes([b0, b1, b2, b3])
}

/// Convert a `u32` glyph extent to `i32`, saturating on overflow.
fn i32_from_u32_saturating(value: u32) -> i32 {
    i32::from_be_bytes(value.min(0x7fff_ffff).to_be_bytes())
}

/// Convert a non-negative `i32` pixel coordinate to `usize`, or `None`.
fn usize_from_i32(value: i32) -> Option<usize> {
    if value < 0 {
        return None;
    }
    usize::try_from(value).ok()
}

/// Round an `f64` pixel offset to `i32` with saturation, without an `as`
/// cast. NaN maps to zero (matching `as`); out-of-range values saturate.
/// Callers pass `round()`ed values, so the loop below is exact binary
/// place-value reconstruction (31 iterations, no allocation).
fn i32_from_f64_saturating(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    if value >= f64::from(i32::MAX) {
        return i32::MAX;
    }
    if value <= f64::from(i32::MIN) {
        return i32::MIN;
    }
    let negative = value < 0.0;
    let mut magnitude = if negative { -value } else { value };
    let mut whole: i32 = 0;
    let mut bit = 1_i32 << 30;
    while bit > 0 {
        let bit_value = f64::from(bit);
        if magnitude >= bit_value {
            magnitude -= bit_value;
            whole = whole.wrapping_add(bit);
        }
        bit >>= 1;
    }
    if negative {
        whole.saturating_neg()
    } else {
        whole
    }
}

#[cfg(test)]
mod tests {
    use super::{f32_from_u32, i32_from_f64_saturating, pixel_channel};

    /// Viewport extents convert exactly across the 16-bit half boundary.
    #[test]
    fn extents_convert_to_f32_without_casts() -> Result<(), String> {
        for (value, want) in [
            (0_u32, 0.0_f32),
            (1, 1.0),
            (65_535, 65_535.0),
            (65_536, 65_536.0),
            (1_048_577, 1_048_577.0),
        ] {
            let got = f32_from_u32(value);
            if got.to_bits() != want.to_bits() {
                return Err(format!("f32_from_u32({value}) = {got}, want {want}"));
            }
        }
        Ok(())
    }

    /// Assert one channel truncation.
    fn assert_channel(value: f64, want: u32) -> Result<(), String> {
        let got = pixel_channel(value);
        if got != want {
            return Err(format!("pixel_channel({value}) = {got}, want {want}"));
        }
        Ok(())
    }

    /// Gradient channels truncate exactly across their constructed ranges.
    #[test]
    fn pixel_channels_truncate_without_casts() -> Result<(), String> {
        assert_channel(239.0, 239)?;
        assert_channel(214.0, 214)?;
        assert_channel(238.7, 238)?;
        assert_channel(226.0, 226)?;
        assert_channel(235.0, 235)?;
        assert_channel(223.0, 223)?;
        assert_channel(234.0, 234)?;
        assert_channel(128.0, 128)?;
        assert_channel(255.0, 255)?;
        Ok(())
    }

    /// Pixel offsets round and saturate exactly.
    #[test]
    fn offsets_convert_without_casts() -> Result<(), String> {
        if i32_from_f64_saturating(12.6_f64.round()) != 13 {
            return Err(String::from("rounded offset must convert"));
        }
        if i32_from_f64_saturating(-12.6_f64.round()) != -13 {
            return Err(String::from("negative rounded offset must convert"));
        }
        if i32_from_f64_saturating(f64::NAN) != 0 {
            return Err(String::from("NaN must map to zero"));
        }
        if i32_from_f64_saturating(1.0e300) != i32::MAX {
            return Err(String::from("overflow must saturate"));
        }
        Ok(())
    }
}
