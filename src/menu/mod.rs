//! Menu framework model and generic render surfaces.
//!
//! Content providers register menu data through the core types. The render
//! modules stay content-agnostic so new menus can be added without changing the
//! generic menu surface.

pub mod availability;
#[allow(dead_code)]
pub mod multi_select_view;
pub mod providers;
pub mod registry;
#[allow(dead_code)]
pub mod render;
#[allow(dead_code)]
pub mod selection_view;
pub mod types;

pub use availability::*;
pub use registry::*;
pub use types::*;
