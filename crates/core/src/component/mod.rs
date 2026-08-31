mod registry;

use std::any::Any;

pub use registry::{ComponentGeneration, ComponentIndex, ComponentKey, ComponentRegistry, Error};

/// Marker trait for component types. Use `#[derive(Component)]` as convenience access.
pub trait Component: Any + Send + Sync {}
