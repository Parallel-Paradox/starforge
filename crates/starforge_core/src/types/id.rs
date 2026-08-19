use std::any::Any;
use std::any::TypeId as StdTypeId;

/// Uniquely identifies a native Rust type or a script-defined type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TypeId {
    /// A type known to the Rust type system.
    Native(StdTypeId),
    /// A type defined by a script, identified by an opaque script-assigned id.
    Script(usize),
}

impl TypeId {
    /// Returns the `TypeId` for a native Rust type `T`.
    pub fn of<T: Any + 'static>() -> Self {
        let std_type_id = StdTypeId::of::<T>();
        Self::Native(std_type_id)
    }

    /// Returns the `TypeId` for a script-defined type identified by `script_id`.
    pub const fn of_script(script_id: usize) -> Self {
        Self::Script(script_id)
    }
}
