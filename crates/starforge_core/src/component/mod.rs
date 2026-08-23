mod meta;
mod registry;

pub use meta::{ComponentKind, ComponentMeta};
pub use registry::{ComponentGeneration, ComponentIndex, ComponentKey, ComponentRegistry, Error};

pub trait Component: 'static + Send + Sync {}
