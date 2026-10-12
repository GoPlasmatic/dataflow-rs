//! The built-in functions' configs.
//!
//! Public, but `#[doc(hidden)]`, so code outside the crate can compile a
//! built-in's config: today `tests/map_vs_v3.rs`, and the v4 corpus driver
//! next. It becomes crate-private when phase 3's `Compiler::check` replaces
//! that use.

mod map;

pub use map::{MapConfig, MapMode, Mapping, OnNull};
