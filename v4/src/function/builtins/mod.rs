//! The built-in functions' configs.
//!
//! Public but `#[doc(hidden)]` only so the v4 corpus driver can compile `map`
//! configs. It becomes crate-private when phase 3's `Compiler::check` replaces
//! that use.

mod map;

pub use map::{MapConfig, MapMode, Mapping, OnNull};
