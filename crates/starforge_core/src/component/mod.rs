mod meta;
mod registry;

pub use meta::{ComponentKind, ComponentMeta};
pub use registry::{ComponentKey, ComponentRegistry, Error};

pub trait Component: 'static + Send + Sync {}
