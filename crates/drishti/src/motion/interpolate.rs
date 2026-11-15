//! Interpolator sub-modules. Each module handles one value domain.

pub mod clip_path;
pub mod color_oklab;
pub mod matrix;
pub mod path_morph;
pub mod scalar;
pub mod vec;

pub use color_oklab::interp_oklab;
pub use scalar::lerp_scalar;
