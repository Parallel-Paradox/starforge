use std::any::Any;
use std::any::TypeId as StdTypeId;
use std::sync::Arc;

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

/// A human-readable name for a native Rust type or a script-defined type.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum TypeName {
    /// The name reported by Rust for a native type.
    Native(&'static str),
    /// The name assigned to a script-defined type.
    Script(Arc<str>),
}

impl TypeName {
    /// Returns the Rust type name for a native type `T`.
    pub fn of<T: Any + 'static>() -> Self {
        Self::Native(std::any::type_name::<T>())
    }

    /// Returns the name of a script-defined type.
    pub fn of_script(name: impl AsRef<str>) -> Self {
        Self::Script(Arc::from(name.as_ref()))
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

    #[test]
    fn native_name_is_derived_from_rust_type_name() {
        assert_eq!(TypeName::of::<u8>(), TypeName::Native("u8"));
    }

    #[test]
    fn script_name_accepts_string_like_values() {
        assert_eq!(TypeName::of_script("Player"), TypeName::Script(Arc::from("Player")));
    }
}
