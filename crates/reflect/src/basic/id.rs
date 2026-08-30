use std::any::{Any, TypeId as StdTypeId};

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
    pub fn of<T: Any>() -> Self {
        let std_type_id = StdTypeId::of::<T>();
        Self::Native(std_type_id)
    }

    /// Returns the `TypeId` for a script-defined type identified by `script_id`.
    pub const fn of_script(script_id: usize) -> Self {
        Self::Script(script_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_id_is_derived_from_rust_type_id() {
        assert_eq!(TypeId::of::<u8>(), TypeId::Native(StdTypeId::of::<u8>()));
    }

    #[test]
    fn script_id_preserves_script_identifier() {
        assert_eq!(TypeId::of_script(42), TypeId::Script(42));
    }
}
